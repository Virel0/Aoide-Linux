//! Mixes from a mood: a description goes to the model (`describe_mix`),
//! which answers with smart rules and the names the description mentioned;
//! the library and the listening history then pick the tracks.
//!
//! - **Names** ("Mara Vey", "Night Shift") are searched in Jellyfin: the
//!   albums and album artists they name, and songs by that name.
//! - **The library half of the rules** (genre, year, and the text and
//!   length fields) is asked of Jellyfin as a pre-filter where it can be
//!   (genres by id, years) and checked here on every
//!   candidate ([`rules::library_matches`]); the model's genre words are first
//!   resolved to the library's own genres (`match_genres`: "rock" takes Hard
//!   Rock, not Rockabilly).
//! - **The listening half** (play and skip counts, first and last played,
//!   and liked, which is Aoide's own like, not the Jellyfin favourite, as
//!   in a saved smart playlist) only the local store knows: the candidates
//!   go into the store's track
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
use aoide_integrations::openrouter::smart_rules::{Match, SmartRules};
use aoide_jellyfin::{ItemKind, JellyfinClient};
use iced::Task;

use crate::app::{App, Message as AppMessage};
use crate::credentials::CredentialStore;
use crate::playback::tracks::TrackMeta;
use crate::smart::{Failure, read_key};
use crate::store_worker::StoreHandle;

mod rules;

use rules::{candidate_query, history_rules, select, sorted, store_rules};

/// Songs in a mix when the rules give no limit.
pub const DEFAULT_LENGTH: usize = 50;
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

    let mut found = Candidates::default();
    found.by_name(&client, library, &outcome.names).await?;
    if wants_rule_candidates(outcome.rules.as_ref(), &outcome.names)
        && let Some(rules) = &outcome.rules
    {
        let query = candidate_query(rules, &genres, &filters.genres, &filters.years, library);
        let page = client.songs(&query).await.map_err(Failure::Server)?;
        found.by_rules(&page.items);
    }
    let chosen = choose(&store, outcome.rules.as_ref(), &found, &library_genres).await?;
    log::info!(
        "mix: {} names ({} missed), {} candidates, {} chosen",
        outcome.names.len(),
        found.missed.len(),
        found.rule_ids.len(),
        chosen.len()
    );
    Ok(Mix {
        description,
        looked_for,
        tracks: chosen
            .iter()
            .filter_map(|id| found.tracks.get(id).cloned())
            .collect(),
        rules: outcome.rules,
        rejected: outcome.rejected,
        missed: (!found.missed.is_empty()).then(|| missed_names_message(&found.missed)),
    })
}

/// The songs a mix may take, by id: those the names stand for, and those
/// the library rules asked Jellyfin for.
#[derive(Default)]
struct Candidates {
    tracks: HashMap<String, TrackMeta>,
    name_ids: Vec<String>,
    rule_ids: Vec<String>,
    /// Names the library has nothing for.
    missed: Vec<String>,
}

impl Candidates {
    async fn by_name(
        &mut self,
        client: &JellyfinClient,
        library: Option<&str>,
        names: &[String],
    ) -> Result<(), Failure> {
        for name in names {
            let found = name_tracks(client, library, name)
                .await
                .map_err(Failure::Server)?;
            if found.is_empty() {
                self.missed.push(name.clone());
            }
            for track in found {
                self.name_ids.push(track.id.clone());
                self.tracks.entry(track.id.clone()).or_insert(track);
            }
        }
        Ok(())
    }

    fn by_rules(&mut self, items: &[aoide_jellyfin::Item]) {
        for item in items {
            let track = TrackMeta::from_item(item);
            self.rule_ids.push(track.id.clone());
            self.tracks.entry(track.id.clone()).or_insert(track);
        }
    }
}

/// The candidates through the history and the Not Interested flags, sorted,
/// name hits first, cut at the rules' limit.
async fn choose(
    store: &StoreHandle,
    rules: Option<&SmartRules>,
    found: &Candidates,
    library_genres: &[String],
) -> Result<Vec<String>, Failure> {
    // Name hits stand for themselves, not for the library rules (which chose
    // the rest); both go through the history and the Not Interested flags.
    let candidates: Vec<TrackMeta> = order_candidates(&found.name_ids, &found.rule_ids)
        .iter()
        .filter_map(|id| found.tracks.get(id).cloned())
        .collect();
    let named: HashSet<String> = found.name_ids.iter().cloned().collect();
    let (heard, visible) = evaluate(store, &candidates, rules.and_then(history_rules)).await?;
    let visible: HashSet<String> = visible.into_iter().collect();
    let heard_set: Option<HashSet<String>> = heard.as_ref().map(|h| h.iter().cloned().collect());
    let (named_tracks, rest): (Vec<TrackMeta>, Vec<TrackMeta>) =
        candidates.into_iter().partition(|t| named.contains(&t.id));
    let all = rules.is_none_or(|r| r.match_ == Match::All);
    let mut chosen: Vec<String> = named_tracks
        .iter()
        .filter(|t| visible.contains(&t.id))
        .filter(|t| !all || heard_set.as_ref().is_none_or(|h| h.contains(&t.id)))
        .map(|t| t.id.clone())
        .collect();
    chosen.extend(select(
        rules,
        &rest,
        library_genres,
        heard_set.as_ref(),
        &visible,
    ));
    let chosen = sorted(
        chosen,
        rules,
        &found.tracks,
        heard.as_deref().unwrap_or_default(),
    );
    let mut chosen = names_first(&chosen, &named);
    let length = rules
        .and_then(|r| r.limit)
        .map_or(DEFAULT_LENGTH, |l| l as usize);
    chosen.truncate(length);
    Ok(chosen)
}

/// The candidates into the store's track cache, then evaluated there: the
/// listening rules' matches (`None` without any), and every song not marked
/// Not Interested.
async fn evaluate(
    store: &StoreHandle,
    candidates: &[TrackMeta],
    history: Option<aoide_store::SmartRules>,
) -> Result<(Option<Vec<String>>, Vec<String>), Failure> {
    let now = aoide_store::now_millis();
    let rows: Vec<_> = candidates.iter().map(|t| t.track_ref(now)).collect();
    store
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
        .map_err(Failure::Other)
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
        if self.session.is_none() {
            return Task::none();
        }
        match request {
            Request::Compose { page, description } => self.compose_mix(page, description),
            Request::Save { page, mix } => self.save_mix(page, &mix),
        }
    }

    /// A description to a mix, for the Mixes page with key `page`.
    fn compose_mix(&mut self, page: u64, description: String) -> Task<AppMessage> {
        use crate::screens::PageMsg;
        use crate::screens::mixes::Message as M;
        let Some(signed) = &self.session else {
            return Task::none();
        };
        let asked = description.clone();
        let openrouter = match crate::smart::client() {
            Ok(client) => client,
            Err(error) => {
                return Task::done(AppMessage::Page(
                    page,
                    PageMsg::Mixes(M::Composed(asked, Err(error.into()))),
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
                        result.map(Box::new).map_err(crate::failure::Failure::from),
                    )),
                ),
            },
        )
    }

    /// Keeps a mix as a smart playlist named after its description.
    fn save_mix(&mut self, page: u64, mix: &Mix) -> Task<AppMessage> {
        use crate::screens::PageMsg;
        use crate::screens::mixes::Message as M;
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
                        let list = w.store.create_playlist(&name, None, Some(&notes), now)?;
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

#[cfg(test)]
mod tests;
