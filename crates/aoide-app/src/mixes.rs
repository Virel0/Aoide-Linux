//! Mixes from a mood: a description goes to the model (`describe_mix`),
//! which answers with smart rules and the names the description mentioned;
//! the library and the listening history then pick the tracks.
//!
//! - **Names** ("Mara Vey", "Night Shift") are searched in Jellyfin: the
//!   albums and album artists they name, and songs by that name.
//! - **The library half of the rules** (genre, year, favourites, and the
//!   text and length fields) is asked of Jellyfin as a pre-filter where it
//!   can be (genres by id, years, favourites) and checked here on every
//!   candidate ([`library_matches`]); the model's genre words are first
//!   resolved to the library's own genres (`match_genres`: "rock" takes Hard
//!   Rock, not Rockabilly).
//! - **The listening half** (play and skip counts, first and last played)
//!   only the local history knows: the candidates go into the store's track
//!   cache and those rules are evaluated there. Every mix also leaves out
//!   the songs marked Not Interested, as the store's evaluation does.
//!
//! Name hits come first, then the rest, cut at the rules' limit. What was
//! understood is said in one line ("Looked for: …"), the explanation for a
//! mix that comes back empty.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use aoide_integrations::openrouter::OpenRouter;
use aoide_integrations::openrouter::mix_plan::{
    MixPlan, describe_plan, described_genres, match_genres, missed_names_message, names_first,
    order_candidates, wants_rule_candidates,
};
use aoide_integrations::openrouter::smart_rules::{
    Match, RuleField, RuleOperator, RuleValue, SmartRule, SmartRules, SortDirection,
};
use aoide_jellyfin::{ItemKind, JellyfinClient, ListQuery, NameId, Sort};
use iced::Task;

use crate::app::{App, Message as AppMessage};
use crate::credentials::CredentialStore;
use crate::playback::tracks::TrackMeta;
use crate::smart::{Failure, read_key};
use crate::store_worker::StoreHandle;

/// Songs in a mix when the rules give no limit.
pub const DEFAULT_LENGTH: usize = 50;
/// Candidates asked of Jellyfin by the rules.
const CANDIDATES: u32 = 300;
/// Per name: albums and artists followed, songs taken.
const NAME_ALBUMS: usize = 3;
const NAME_ARTISTS: usize = 2;
const ARTIST_ALBUMS: usize = 4;
const NAME_SONGS: u32 = 10;

/// A mix, ready to play, keep or throw away.
#[derive(Debug, Clone, PartialEq)]
pub struct Mix {
    pub description: String,
    /// What was understood, in one line.
    pub looked_for: String,
    pub tracks: Vec<TrackMeta>,
    /// The rules, to keep as a smart playlist.
    pub rules: Option<SmartRules>,
    /// Rules the model got wrong, and why they were left out.
    pub rejected: Vec<String>,
    /// Names the library has nothing for, in a sentence.
    pub missed: Option<String>,
}

impl Mix {
    /// A mix can be kept as a smart playlist only if its rules say
    /// something: names are not rules, and "all of nothing" would keep the
    /// whole library.
    pub fn can_save(&self) -> bool {
        self.rules.as_ref().is_some_and(|r| !r.rules.is_empty())
    }
}

/// Requests from the Mixes screen, handled by the app (it holds the key).
#[derive(Debug, Clone)]
pub enum Request {
    Compose { page: u64, description: String },
    Save { page: u64, mix: Box<Mix> },
}

fn lower(text: &str) -> String {
    text.to_lowercase()
}

fn text_matches(op: RuleOperator, have: &str, want: &str) -> bool {
    let (have, want) = (lower(have), lower(want));
    match op {
        RuleOperator::Is => have == want,
        RuleOperator::IsNot => have != want,
        RuleOperator::Contains => have.contains(&want),
        RuleOperator::DoesNotContain => !have.contains(&want),
        RuleOperator::BeginsWith => have.starts_with(&want),
        _ => false,
    }
}

fn number_matches(op: RuleOperator, have: Option<i64>, want: i64) -> bool {
    match (op, have) {
        (RuleOperator::Is, Some(have)) => have == want,
        // Not knowing a year satisfies "is not 1999", as the store has it.
        (RuleOperator::IsNot, have) => have != Some(want),
        (RuleOperator::GreaterThan, Some(have)) => have > want,
        (RuleOperator::LessThan, Some(have)) => have < want,
        _ => false,
    }
}

/// Whether `track` satisfies a library rule; `None` for a listening rule,
/// which only the store can answer. Genre words mean what [`match_genres`]
/// makes of them in `library_genres`.
pub fn library_matches(
    rule: &SmartRule,
    track: &TrackMeta,
    library_genres: &[String],
) -> Option<bool> {
    let text = match &rule.value {
        RuleValue::Text(t) => Some(t.as_str()),
        _ => None,
    };
    let number = match rule.value {
        RuleValue::Number(n) => Some(n),
        _ => None,
    };
    Some(match rule.field {
        field if field.is_history() => return None,
        RuleField::Title => text.is_some_and(|t| text_matches(rule.op, &track.title, t)),
        RuleField::Album => text.is_some_and(|t| text_matches(rule.op, &track.album, t)),
        RuleField::AlbumArtist => text
            .is_some_and(|t| text_matches(rule.op, track.album_artist.as_deref().unwrap_or(""), t)),
        // Any credited artist, or the credits as written.
        RuleField::Artist => text.is_some_and(|t| {
            let credits: Vec<&str> = track.artist.split(", ").collect();
            let any = |op| {
                text_matches(op, &track.artist, t) || credits.iter().any(|c| text_matches(op, c, t))
            };
            match rule.op {
                RuleOperator::IsNot => !any(RuleOperator::Is),
                RuleOperator::DoesNotContain => !any(RuleOperator::Contains),
                op => any(op),
            }
        }),
        RuleField::Genre => text.is_some_and(|t| {
            let meant: Vec<String> = match_genres(&[t.to_owned()], library_genres);
            let named = |g: &String| meant.iter().any(|m| m.eq_ignore_ascii_case(g));
            match rule.op {
                RuleOperator::Is | RuleOperator::Contains => track
                    .genres
                    .iter()
                    .any(|g| named(g) || text_matches(rule.op, g, t)),
                RuleOperator::IsNot => !track
                    .genres
                    .iter()
                    .any(|g| named(g) || lower(g) == lower(t)),
                RuleOperator::DoesNotContain => !track
                    .genres
                    .iter()
                    .any(|g| named(g) || lower(g).contains(&lower(t))),
                RuleOperator::BeginsWith => track
                    .genres
                    .iter()
                    .any(|g| text_matches(RuleOperator::BeginsWith, g, t)),
                _ => false,
            }
        }),
        RuleField::Year => {
            number.is_some_and(|n| number_matches(rule.op, track.year.map(i64::from), n))
        }
        RuleField::DurationMs => {
            number.is_some_and(|n| number_matches(rule.op, track.duration_ms, n))
        }
        RuleField::Liked => match rule.value {
            RuleValue::Flag(want) => match rule.op {
                RuleOperator::Is => track.favourite == want,
                RuleOperator::IsNot => track.favourite != want,
                _ => false,
            },
            _ => false,
        },
        _ => return None,
    })
}

/// The Jellyfin pre-filter for the rules: only what narrows without ever
/// dropping a track the rules would keep. For "any", nothing but the
/// genres when the genres are all there is.
pub fn candidate_query(
    rules: &SmartRules,
    genres: &[String],
    genre_ids: &[NameId],
    library_years: &[i32],
    library: Option<&str>,
) -> ListQuery {
    let mut query = ListQuery {
        parent_id: library.map(str::to_owned),
        sort: Sort::Random,
        limit: Some(CANDIDATES),
        ..ListQuery::default()
    };
    let only_genres = rules.rules.iter().all(|r| r.field == RuleField::Genre);
    if rules.match_ == Match::Any && !only_genres {
        return query;
    }
    let positive_genre = rules.rules.iter().any(|r| {
        r.field == RuleField::Genre && matches!(r.op, RuleOperator::Is | RuleOperator::Contains)
    });
    if positive_genre && !genres.is_empty() {
        query.genre_ids = genres
            .iter()
            .filter_map(|g| {
                genre_ids
                    .iter()
                    .find(|n| n.name == *g)
                    .map(|n| n.id.clone())
            })
            .collect();
    }
    if rules.match_ == Match::All {
        let years: Vec<i32> = library_years
            .iter()
            .copied()
            .filter(|y| {
                rules
                    .rules
                    .iter()
                    .filter(|r| r.field == RuleField::Year && r.op != RuleOperator::IsNot)
                    .all(|r| match r.value {
                        RuleValue::Number(n) => number_matches(r.op, Some(i64::from(*y)), n),
                        _ => true,
                    })
            })
            .collect();
        let has_year_rule = rules
            .rules
            .iter()
            .any(|r| r.field == RuleField::Year && r.op != RuleOperator::IsNot);
        // An empty list is every year to Jellyfin: only send a real narrowing.
        if has_year_rule && !years.is_empty() && years.len() < library_years.len() {
            query.years = years;
        }
        query.favorites_only = rules.rules.iter().any(|r| {
            r.field == RuleField::Liked
                && r.op == RuleOperator::Is
                && r.value == RuleValue::Flag(true)
        });
    }
    query
}

/// The listening rules alone, as the store evaluates them (the phone's
/// JSON, read by aoide-store).
pub fn history_rules(rules: &SmartRules) -> Option<aoide_store::SmartRules> {
    let history: Vec<SmartRule> = rules
        .rules
        .iter()
        .filter(|r| r.field.is_history())
        .cloned()
        .collect();
    if history.is_empty() {
        return None;
    }
    let only = SmartRules {
        limit: None,
        match_: rules.match_,
        rules: history,
        sort: rules.sort.filter(|s| s.field.is_history()),
    };
    store_rules(&only)
}

/// The rules as the store keeps them, for a smart playlist.
pub fn store_rules(rules: &SmartRules) -> Option<aoide_store::SmartRules> {
    let json = serde_json::to_string(rules).ok()?;
    aoide_store::SmartRules::from_json(&json).ok()
}

/// Which candidates the rules keep, given what the store said: `history`
/// are the ids passing the listening rules (`None` if there are none),
/// `visible` the ids not marked Not Interested.
pub fn select(
    rules: Option<&SmartRules>,
    candidates: &[TrackMeta],
    library_genres: &[String],
    history: Option<&HashSet<String>>,
    visible: &HashSet<String>,
) -> Vec<String> {
    candidates
        .iter()
        .filter(|t| visible.contains(&t.id))
        .filter(|t| {
            let Some(rules) = rules else {
                return true;
            };
            let library: Vec<bool> = rules
                .rules
                .iter()
                .filter_map(|r| library_matches(r, t, library_genres))
                .collect();
            let heard = history.map(|h| h.contains(&t.id));
            match rules.match_ {
                Match::All => library.iter().all(|&b| b) && heard.unwrap_or(true),
                Match::Any => {
                    (rules.rules.is_empty()) || library.iter().any(|&b| b) || heard.unwrap_or(false)
                }
            }
        })
        .map(|t| t.id.clone())
        .collect()
}

/// Orders by a library sort field where the rules ask for one; a listening
/// sort is the store's order, given as `history_order`.
fn sorted(
    mut ids: Vec<String>,
    rules: Option<&SmartRules>,
    tracks: &HashMap<String, TrackMeta>,
    history_order: &[String],
) -> Vec<String> {
    let Some(sort) = rules.and_then(|r| r.sort) else {
        return ids;
    };
    if sort.field.is_history() {
        let rank: HashMap<&str, usize> = history_order
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        ids.sort_by_key(|id| rank.get(id.as_str()).copied().unwrap_or(usize::MAX));
        return ids;
    }
    let key = |id: &String| -> (i64, String) {
        let t = tracks.get(id);
        match sort.field {
            RuleField::Year => (t.and_then(|t| t.year).map_or(0, i64::from), String::new()),
            RuleField::DurationMs => (t.and_then(|t| t.duration_ms).unwrap_or(0), String::new()),
            RuleField::Artist => (0, t.map(|t| lower(&t.artist)).unwrap_or_default()),
            RuleField::Album => (0, t.map(|t| lower(&t.album)).unwrap_or_default()),
            _ => (0, t.map(|t| lower(&t.title)).unwrap_or_default()),
        }
    };
    ids.sort_by_key(key);
    if sort.dir == SortDirection::Desc {
        ids.reverse();
    }
    ids
}

/// The tracks a name stands for: the albums it names, the album artists
/// it names (a few albums each), and songs by that name.
async fn name_tracks(
    client: &JellyfinClient,
    library: Option<&str>,
    name: &str,
) -> Result<Vec<TrackMeta>, aoide_jellyfin::Error> {
    let found = client.search(name, library, NAME_SONGS).await?;
    let mut tracks: Vec<TrackMeta> = Vec::new();
    for album in found.albums.iter().take(NAME_ALBUMS) {
        tracks.extend(
            client
                .album_tracks(&album.id)
                .await?
                .iter()
                .map(TrackMeta::from_item),
        );
    }
    for artist in found.artists.iter().take(NAME_ARTISTS) {
        for album in client
            .artist_albums(&artist.id)
            .await?
            .iter()
            .take(ARTIST_ALBUMS)
        {
            tracks.extend(
                client
                    .album_tracks(&album.id)
                    .await?
                    .iter()
                    .map(TrackMeta::from_item),
            );
        }
    }
    tracks.extend(found.songs.iter().map(TrackMeta::from_item));
    Ok(tracks)
}

/// A description, through the model, the library and the history, to a mix.
pub async fn compose(
    openrouter: OpenRouter,
    keyring: Arc<dyn CredentialStore>,
    model: String,
    client: JellyfinClient,
    store: StoreHandle,
    library: Option<String>,
    description: String,
) -> Result<Mix, Failure> {
    let key = read_key(keyring).await.map_err(Failure::Other)?;
    let library = library.as_deref();
    let filters = client
        .query_filters(library, &[ItemKind::Audio])
        .await
        .map_err(Failure::Server)?;
    let library_genres: Vec<String> = filters.genres.iter().map(|g| g.name.clone()).collect();
    let outcome = openrouter
        .describe_mix(&key, &model, &description, &library_genres)
        .await;
    if let Some(reason) = outcome.reason {
        return Err(Failure::Other(reason));
    }
    let genres = match_genres(&described_genres(outcome.rules.as_ref()), &library_genres);
    let plan = MixPlan {
        genres: genres.clone(),
        names: outcome.names.clone(),
        rules: outcome.rules.clone(),
    };
    let looked_for = describe_plan(&plan);

    let mut tracks: HashMap<String, TrackMeta> = HashMap::new();
    let mut name_ids: Vec<String> = Vec::new();
    let mut missed: Vec<String> = Vec::new();
    for name in &outcome.names {
        let found = name_tracks(&client, library, name)
            .await
            .map_err(Failure::Server)?;
        if found.is_empty() {
            missed.push(name.clone());
        }
        for track in found {
            name_ids.push(track.id.clone());
            tracks.entry(track.id.clone()).or_insert(track);
        }
    }
    let mut rule_ids: Vec<String> = Vec::new();
    if wants_rule_candidates(outcome.rules.as_ref(), &outcome.names)
        && let Some(rules) = &outcome.rules
    {
        let query = candidate_query(rules, &genres, &filters.genres, &filters.years, library);
        let page = client.songs(&query).await.map_err(Failure::Server)?;
        for item in &page.items {
            let track = TrackMeta::from_item(item);
            rule_ids.push(track.id.clone());
            tracks.entry(track.id.clone()).or_insert(track);
        }
    }

    // Name hits stand for themselves, not for the library rules (which chose
    // the rest); both go through the history and the Not Interested flags.
    let candidates: Vec<TrackMeta> = order_candidates(&name_ids, &rule_ids)
        .iter()
        .filter_map(|id| tracks.get(id).cloned())
        .collect();
    let named: HashSet<String> = name_ids.iter().cloned().collect();
    let history = outcome.rules.as_ref().and_then(history_rules);
    let now = aoide_store::now_millis();
    let rows: Vec<_> = candidates.iter().map(|t| t.track_ref(now)).collect();
    let (heard, visible) = store
        .call(move |w| {
            w.store.cache_tracks(&rows).map_err(|e| e.to_string())?;
            let calendar = aoide_store::Calendar::Local;
            let visible = w
                .store
                .evaluate(&aoide_store::SmartRules::everything(), now, calendar)
                .map_err(|e| e.to_string())?;
            let heard = match history {
                Some(rules) => Some(
                    w.store
                        .evaluate(&rules, now, calendar)
                        .map_err(|e| e.to_string())?,
                ),
                None => None,
            };
            Ok::<_, String>((heard, visible))
        })
        .await
        .unwrap_or_else(|| Err("the library is closing".to_owned()))
        .map_err(Failure::Other)?;
    let visible: HashSet<String> = visible.into_iter().collect();
    let heard_set: Option<HashSet<String>> = heard.as_ref().map(|h| h.iter().cloned().collect());
    let (named_tracks, rest): (Vec<TrackMeta>, Vec<TrackMeta>) =
        candidates.into_iter().partition(|t| named.contains(&t.id));
    let all = outcome
        .rules
        .as_ref()
        .is_none_or(|r| r.match_ == Match::All);
    let mut chosen: Vec<String> = named_tracks
        .iter()
        .filter(|t| visible.contains(&t.id))
        .filter(|t| !all || heard_set.as_ref().is_none_or(|h| h.contains(&t.id)))
        .map(|t| t.id.clone())
        .collect();
    chosen.extend(select(
        outcome.rules.as_ref(),
        &rest,
        &library_genres,
        heard_set.as_ref(),
        &visible,
    ));
    let chosen = sorted(
        chosen,
        outcome.rules.as_ref(),
        &tracks,
        heard.as_deref().unwrap_or_default(),
    );
    let mut chosen = names_first(&chosen, &named);
    let length = outcome
        .rules
        .as_ref()
        .and_then(|r| r.limit)
        .map_or(DEFAULT_LENGTH, |l| l as usize);
    chosen.truncate(length);
    log::info!(
        "mix: {} names ({} missed), {} candidates, {} chosen",
        outcome.names.len(),
        missed.len(),
        rule_ids.len(),
        chosen.len()
    );
    Ok(Mix {
        description,
        looked_for,
        tracks: chosen
            .iter()
            .filter_map(|id| tracks.get(id).cloned())
            .collect(),
        rules: outcome.rules,
        rejected: outcome.rejected,
        missed: (!missed.is_empty()).then(|| missed_names_message(&missed)),
    })
}

/// A playlist's name from the description: its first line, at most 60
/// characters.
pub fn playlist_name(description: &str) -> String {
    let line = description.lines().next().unwrap_or("").trim();
    let mut name: String = line.chars().take(60).collect();
    if line.chars().count() > 60 {
        name.push('…');
    }
    if name.is_empty() {
        "Mix".to_owned()
    } else {
        name
    }
}

impl App {
    pub(crate) fn mix_request(&mut self, request: Request) -> Task<AppMessage> {
        use crate::screens::PageMsg;
        use crate::screens::mixes::Message as M;
        let Some(signed) = &self.session else {
            return Task::none();
        };
        match request {
            Request::Compose { page, description } => {
                let asked = description.clone();
                let openrouter = match crate::smart::client() {
                    Ok(client) => client,
                    Err(error) => {
                        return Task::done(AppMessage::Page(
                            page,
                            PageMsg::Mixes(M::Composed(asked, Err(error))),
                        ));
                    }
                };
                Task::perform(
                    compose(
                        openrouter,
                        Arc::clone(&self.keyring),
                        self.config.system.smart_search.model().to_owned(),
                        signed.connection.client.clone(),
                        self.store.clone(),
                        signed.library.clone(),
                        description,
                    ),
                    move |result| match result {
                        Err(Failure::Server(aoide_jellyfin::Error::Unauthorized)) => {
                            AppMessage::TokenRejected
                        }
                        result => AppMessage::Page(
                            page,
                            PageMsg::Mixes(M::Composed(
                                asked.clone(),
                                result.map(Box::new).map_err(|failure| match failure {
                                    Failure::Server(e) => crate::session::describe(&e),
                                    Failure::Other(e) => e,
                                }),
                            )),
                        ),
                    },
                )
            }
            Request::Save { page, mix } => {
                let Some(rules) = mix.rules.as_ref().and_then(store_rules) else {
                    return Task::done(AppMessage::Page(
                        page,
                        PageMsg::Mixes(M::Saved(Err("These rules can't be kept.".to_owned()))),
                    ));
                };
                let name = playlist_name(&mix.description);
                let notes = format!("A mix: {}", mix.description.trim());
                let store = self.store.clone();
                let now = aoide_store::now_millis();
                Task::perform(
                    async move {
                        store
                            .call(move |w| {
                                let list =
                                    w.store.create_playlist(&name, None, Some(&notes), now)?;
                                let smart = list.setting_smart_rules(&rules);
                                w.store.record(smart, false, now).map(|p| p.name)
                            })
                            .await
                            .map(|r| r.map_err(|e| e.to_string()))
                            .unwrap_or_else(|| Err("the library is closing".into()))
                    },
                    move |result| AppMessage::Page(page, PageMsg::Mixes(M::Saved(result))),
                )
                .chain(Task::done(AppMessage::PlaylistsChanged))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoide_integrations::openrouter::smart_rules::RuleSort;

    fn track(id: &str, genre: &str, year: i32, favourite: bool) -> TrackMeta {
        TrackMeta {
            id: id.into(),
            title: format!("Song {id}"),
            artist: "Mara Vey, The Quiet Engines".into(),
            album_artist: Some("Mara Vey".into()),
            album: "Harbour Songs".into(),
            album_id: None,
            artist_id: None,
            artwork: None,
            duration_ms: Some(200_000),
            track_number: None,
            disc_number: None,
            year: Some(year),
            genres: vec![genre.into()],
            favourite,
        }
    }

    fn rule(field: RuleField, op: RuleOperator, value: RuleValue) -> SmartRule {
        SmartRule::new(field, op, value).unwrap()
    }

    fn library() -> Vec<String> {
        ["Folk", "Hard Rock", "Rock", "Rockabilly", "Ambient"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    }

    fn text(t: &str) -> RuleValue {
        RuleValue::Text(t.into())
    }

    #[test]
    fn genre_words_mean_the_librarys_genres() {
        let rock = rule(RuleField::Genre, RuleOperator::Is, text("rock"));
        let g = library();
        assert_eq!(
            library_matches(&rock, &track("1", "Hard Rock", 2000, false), &g),
            Some(true)
        );
        assert_eq!(
            library_matches(&rock, &track("2", "Rock", 2000, false), &g),
            Some(true)
        );
        assert_eq!(
            library_matches(&rock, &track("3", "Rockabilly", 2000, false), &g),
            Some(false)
        );
        let not_rock = rule(RuleField::Genre, RuleOperator::IsNot, text("rock"));
        assert_eq!(
            library_matches(&not_rock, &track("4", "Folk", 2000, false), &g),
            Some(true)
        );
        assert_eq!(
            library_matches(&not_rock, &track("5", "Hard Rock", 2000, false), &g),
            Some(false)
        );
    }

    #[test]
    fn library_fields_are_checked_on_the_track() {
        let t = track("1", "Folk", 2019, true);
        let g = library();
        let check = |r: SmartRule| library_matches(&r, &t, &g);
        assert_eq!(
            check(rule(
                RuleField::Year,
                RuleOperator::GreaterThan,
                RuleValue::Number(2015)
            )),
            Some(true)
        );
        assert_eq!(
            check(rule(
                RuleField::Year,
                RuleOperator::LessThan,
                RuleValue::Number(2015)
            )),
            Some(false)
        );
        assert_eq!(
            check(rule(
                RuleField::Year,
                RuleOperator::IsNot,
                RuleValue::Number(1999)
            )),
            Some(true)
        );
        assert_eq!(
            check(rule(
                RuleField::Liked,
                RuleOperator::Is,
                RuleValue::Flag(true)
            )),
            Some(true)
        );
        assert_eq!(
            check(rule(
                RuleField::Artist,
                RuleOperator::Is,
                text("the quiet engines")
            )),
            Some(true)
        );
        assert_eq!(
            check(rule(
                RuleField::Artist,
                RuleOperator::IsNot,
                text("Mara Vey")
            )),
            Some(false)
        );
        assert_eq!(
            check(rule(
                RuleField::Album,
                RuleOperator::Contains,
                text("harbour")
            )),
            Some(true)
        );
        assert_eq!(
            check(rule(
                RuleField::Title,
                RuleOperator::BeginsWith,
                text("song")
            )),
            Some(true)
        );
        assert_eq!(
            check(rule(
                RuleField::DurationMs,
                RuleOperator::LessThan,
                RuleValue::Number(180_000)
            )),
            Some(false)
        );
        // The listening fields are the store's.
        assert_eq!(
            check(rule(
                RuleField::PlayCount,
                RuleOperator::Is,
                RuleValue::Number(0)
            )),
            None
        );
        assert_eq!(
            check(rule(
                RuleField::LastPlayed,
                RuleOperator::NotInTheLast,
                text("-6m")
            )),
            None
        );
    }

    fn rules(match_: Match, rules: Vec<SmartRule>) -> SmartRules {
        SmartRules {
            limit: None,
            match_,
            rules,
            sort: None,
        }
    }

    fn ids() -> Vec<NameId> {
        library()
            .into_iter()
            .map(|name| NameId {
                id: format!("g-{name}"),
                name,
            })
            .collect()
    }

    #[test]
    fn the_prefilter_narrows_only_safely() {
        let all = rules(
            Match::All,
            vec![
                rule(RuleField::Genre, RuleOperator::Is, text("rock")),
                rule(
                    RuleField::Year,
                    RuleOperator::GreaterThan,
                    RuleValue::Number(2016),
                ),
                rule(RuleField::Liked, RuleOperator::Is, RuleValue::Flag(true)),
            ],
        );
        let genres = vec!["Hard Rock".to_owned(), "Rock".to_owned()];
        let q = candidate_query(&all, &genres, &ids(), &[2023, 2019, 2015], Some("lib1"));
        assert_eq!(q.genre_ids, ["g-Hard Rock", "g-Rock"]);
        assert_eq!(q.years, [2023, 2019]);
        assert!(q.favorites_only);
        assert_eq!(q.sort, Sort::Random);
        assert_eq!(q.parent_id.as_deref(), Some("lib1"));
        // "Any" of a genre and a year: a genre filter would drop the year's
        // tracks, so nothing narrows.
        let any = rules(
            Match::Any,
            vec![
                rule(RuleField::Genre, RuleOperator::Is, text("rock")),
                rule(RuleField::Year, RuleOperator::Is, RuleValue::Number(2019)),
            ],
        );
        let q = candidate_query(&any, &genres, &ids(), &[2019], None);
        assert!(q.genre_ids.is_empty() && q.years.is_empty() && !q.favorites_only);
        // "Any" of genres alone: the genre ids are already "any of".
        let genres_only = rules(
            Match::Any,
            vec![rule(RuleField::Genre, RuleOperator::Is, text("rock"))],
        );
        assert_eq!(
            candidate_query(&genres_only, &genres, &ids(), &[], None)
                .genre_ids
                .len(),
            2
        );
        // A year rule no library year passes sends no years (not "every year").
        let none = rules(
            Match::All,
            vec![rule(
                RuleField::Year,
                RuleOperator::LessThan,
                RuleValue::Number(1900),
            )],
        );
        assert!(
            candidate_query(&none, &[], &ids(), &[2019], None)
                .years
                .is_empty()
        );
    }

    #[test]
    fn selection_combines_the_library_the_history_and_the_flags() {
        let candidates = vec![
            track("folk-heard", "Folk", 2019, false),
            track("folk-new", "Folk", 2019, false),
            track("rock-heard", "Rock", 2019, false),
            track("folk-hidden", "Folk", 2019, false),
        ];
        let visible: HashSet<String> = ["folk-heard", "folk-new", "rock-heard"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let heard: HashSet<String> = ["folk-heard", "rock-heard"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let folk = rule(RuleField::Genre, RuleOperator::Is, text("folk"));
        let forgotten = rule(
            RuleField::LastPlayed,
            RuleOperator::NotInTheLast,
            text("-6m"),
        );
        let all = rules(Match::All, vec![folk.clone(), forgotten.clone()]);
        assert_eq!(
            select(Some(&all), &candidates, &library(), Some(&heard), &visible),
            ["folk-heard"]
        );
        let any = rules(Match::Any, vec![folk, forgotten]);
        assert_eq!(
            select(Some(&any), &candidates, &library(), Some(&heard), &visible),
            ["folk-heard", "folk-new", "rock-heard"]
        );
        // "All of nothing" is everything not hidden.
        let everything = rules(Match::All, vec![]);
        assert_eq!(
            select(Some(&everything), &candidates, &library(), None, &visible).len(),
            3
        );
    }

    #[test]
    fn the_listening_rules_go_to_the_store_in_its_own_format() {
        let mixed = SmartRules {
            limit: Some(20),
            match_: Match::All,
            rules: vec![
                rule(RuleField::Genre, RuleOperator::Is, text("folk")),
                rule(
                    RuleField::PlayCount,
                    RuleOperator::GreaterThan,
                    RuleValue::Number(3),
                ),
            ],
            sort: Some(RuleSort {
                dir: SortDirection::Desc,
                field: RuleField::PlayCount,
            }),
        };
        let history = history_rules(&mixed).unwrap();
        assert_eq!(
            history.to_json(),
            r#"{"match":"all","rules":[{"field":"play_count","op":"greaterThan","value":3}],"sort":{"dir":"desc","field":"play_count"}}"#
        );
        let full = store_rules(&mixed).unwrap();
        assert_eq!(full.rules.len(), 2);
        assert_eq!(full.limit, Some(20));
        assert!(
            history_rules(&rules(
                Match::All,
                vec![rule(RuleField::Genre, RuleOperator::Is, text("folk"))]
            ))
            .is_none()
        );
    }

    #[test]
    fn a_library_sort_is_applied_here_and_a_listening_one_by_the_store() {
        let tracks: HashMap<String, TrackMeta> = [
            track("a", "Folk", 2015, false),
            track("b", "Folk", 2023, false),
            track("c", "Folk", 2019, false),
        ]
        .into_iter()
        .map(|t| (t.id.clone(), t))
        .collect();
        let ids = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let newest = SmartRules {
            sort: Some(RuleSort {
                dir: SortDirection::Desc,
                field: RuleField::Year,
            }),
            ..rules(Match::All, vec![])
        };
        assert_eq!(
            sorted(ids.clone(), Some(&newest), &tracks, &[]),
            ["b", "c", "a"]
        );
        let most_played = SmartRules {
            sort: Some(RuleSort {
                dir: SortDirection::Desc,
                field: RuleField::PlayCount,
            }),
            ..rules(Match::All, vec![])
        };
        let store_order = vec!["c".to_owned(), "a".to_owned()];
        assert_eq!(
            sorted(ids.clone(), Some(&most_played), &tracks, &store_order),
            ["c", "a", "b"]
        );
        assert_eq!(sorted(ids.clone(), None, &tracks, &[]), ids);
    }

    #[test]
    fn a_mix_is_kept_only_with_rules_that_say_something() {
        let mix = |rules| Mix {
            description: "x".into(),
            looked_for: String::new(),
            tracks: Vec::new(),
            rules,
            rejected: Vec::new(),
            missed: None,
        };
        assert!(!mix(None).can_save());
        assert!(!mix(Some(rules(Match::All, vec![]))).can_save());
        assert!(
            mix(Some(rules(
                Match::All,
                vec![rule(RuleField::Genre, RuleOperator::Is, text("folk"))]
            )))
            .can_save()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_mood_becomes_a_mix_with_the_names_first_and_the_hidden_left_out() {
        use crate::credentials::MemoryStore;
        use serde_json::json;
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        let get = |p: &str| Mock::given(method("GET")).and(path(p.to_owned()));
        let items = |items: serde_json::Value| {
            let n = items.as_array().map_or(0, Vec::len);
            ResponseTemplate::new(200).set_body_json(json!({"Items": items, "TotalRecordCount": n}))
        };
        let song = |id: &str, genre: &str, album: &str| {
            json!({"Id": id, "Name": format!("Song {id}"), "Type": "Audio", "Genres": [genre],
                   "Album": album, "AlbumArtist": "Someone", "ProductionYear": 2019})
        };
        get("/Items/Filters2")
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Genres": [{"Id": "g-folk", "Name": "Folk"}, {"Id": "g-rock", "Name": "Rock"}]
            })))
            .mount(&server)
            .await;
        get("/Items/Filters")
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"Years": [2019]})))
            .mount(&server)
            .await;
        // The name: an album artist with one album of two songs.
        get("/Artists/AlbumArtists")
            .and(query_param("searchTerm", "Mara Vey"))
            .respond_with(items(
                json!([{"Id": "ar-mv", "Name": "Mara Vey", "Type": "MusicArtist"}]),
            ))
            .mount(&server)
            .await;
        get("/Items")
            .and(query_param("albumArtistIds", "ar-mv"))
            .respond_with(items(
                json!([{"Id": "al-mv", "Name": "Saltwater", "Type": "MusicAlbum"}]),
            ))
            .mount(&server)
            .await;
        get("/Items")
            .and(query_param("parentId", "al-mv"))
            .respond_with(items(json!([
                song("mv1", "Folk", "Saltwater"),
                song("mv2", "Folk", "Saltwater")
            ])))
            .mount(&server)
            .await;
        get("/Items")
            .and(query_param("searchTerm", "Mara Vey"))
            .respond_with(items(json!([])))
            .mount(&server)
            .await;
        // The rules' candidates: folk, one of them marked Not Interested.
        get("/Items")
            .and(query_param("genreIds", "g-folk"))
            .respond_with(items(json!([
                song("f1", "Folk", "A"),
                song("hidden", "Folk", "B"),
                song("f2", "Folk", "C")
            ])))
            .expect(1)
            .mount(&server)
            .await;
        let reply = json!({"match": "all", "limit": 4, "names": ["Mara Vey"],
            "rules": [{"field": "genre", "op": "is", "value": "folk"},
                      {"field": "last_played", "op": "notInTheLast", "value": "-6m"},
                      {"field": "duration_ms", "op": "beginsWith", "value": 3}]});
        Mock::given(method("POST"))
            .and(path("/api/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{"message": {"content": reply.to_string()}}]
            })))
            .mount(&server)
            .await;

        let store = StoreHandle::in_memory("device-1").unwrap();
        store
            .call(|w| {
                w.store
                    .set_not_interested(true, "hidden", "key-hidden", aoide_store::now_millis())
                    .unwrap();
            })
            .await
            .unwrap();
        let keyring: Arc<dyn CredentialStore> = Arc::new(MemoryStore::default());
        keyring
            .set(aoide_integrations::openrouter::KEYRING_ENTRY, "sk-or-test")
            .unwrap();
        let client = JellyfinClient::new(
            aoide_jellyfin::ServerAddress::parse(&server.uri()).unwrap(),
            aoide_jellyfin::Device::new("Test", "device-1"),
        )
        .unwrap()
        .with_credentials(aoide_jellyfin::Credentials {
            user_id: "u1".into(),
            access_token: "tok".into(),
            username: "tester".into(),
        });
        let mix = compose(
            OpenRouter::with_base_url(&format!("{}/api/v1/", server.uri())).unwrap(),
            keyring,
            "test/model".into(),
            client,
            store.clone(),
            Some("lib1".into()),
            "forgotten folk and Mara Vey".into(),
        )
        .await
        .unwrap();
        let ids: Vec<&str> = mix.tracks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(&ids[..2], ["mv1", "mv2"], "the names come first: {ids:?}");
        assert_eq!(ids.len(), 4, "cut at the rules' limit: {ids:?}");
        assert!(!ids.contains(&"hidden"), "{ids:?}");
        assert_eq!(
            mix.looked_for,
            "Mara Vey · Folk · not played in 6 months · 4 tracks"
        );
        assert_eq!(mix.rejected.len(), 1, "{:?}", mix.rejected);
        assert!(mix.missed.is_none());
        assert!(mix.can_save());
        // The candidates went into the store's track cache for the history.
        let cached = store
            .call(|w| w.store.cached_track_count().unwrap())
            .await
            .unwrap();
        assert_eq!(cached, 5);
    }

    #[test]
    fn playlist_names_come_from_the_description() {
        assert_eq!(
            playlist_name("  rainy Sunday folk \nmore"),
            "rainy Sunday folk"
        );
        assert_eq!(playlist_name("   "), "Mix");
        let long = "a".repeat(80);
        assert_eq!(playlist_name(&long).chars().count(), 61);
    }
}
