//! The library half of a mix's rules, checked here on each candidate, and
//! the listening half handed to the store in its own format; then the
//! selection and its order.

use std::collections::{HashMap, HashSet};

use aoide_integrations::openrouter::mix_plan::match_genres;
use aoide_integrations::openrouter::smart_rules::{
    Match, RuleField, RuleOperator, RuleValue, SmartRule, SmartRules, SortDirection,
};
use aoide_jellyfin::{ListQuery, NameId, Sort};

use crate::playback::tracks::TrackMeta;

/// Candidates asked of Jellyfin by the rules.
const CANDIDATES: u32 = 300;

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
        field if store_answers(field) => return None,
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
    }
    query
}

/// The rules only the store can answer: the listening history, and liked
/// (its own like table, so a mix and the smart playlist saved from it agree).
fn store_answers(field: RuleField) -> bool {
    field.is_history() || field == RuleField::Liked
}

/// The listening rules alone, as the store evaluates them (the phone's
/// JSON, read by aoide-store).
pub fn history_rules(rules: &SmartRules) -> Option<aoide_store::SmartRules> {
    let history: Vec<SmartRule> = rules
        .rules
        .iter()
        .filter(|r| store_answers(r.field))
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
pub(super) fn sorted(
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
