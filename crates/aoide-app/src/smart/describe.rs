//! Search by description: a phrase, through the server's AI, to filters,
//! kept to what the phrase says ([`grounded`]), and the filters to an
//! ordinary Melete query, so the songs are always the server's. The orders
//! Melete can't sort songs by (newest, most or last played, random) are put
//! on a pool of what matches, here.

use aoide_melete::{Error, Item, MeleteClient, MusicFilters, MusicOrdering, Sort};

use super::Ai;
use super::grounding::grounded;
use crate::playback::tracks::TrackMeta;
use crate::song_filter::SongFilter;
use crate::store_worker::StoreHandle;

/// How many songs a description finds at most.
pub const DESCRIBED_SONGS: usize = 100;
/// How many matching songs are read to put in an order of the app's own.
const ORDER_POOL: usize = 500;

/// Most years a range is spelled out as, where the server lists none.
const MAX_YEARS: i32 = 150;

/// What a description found.
#[derive(Debug, Clone, PartialEq)]
pub struct Described {
    /// The filters in words, or `None` when the description gave nothing to
    /// filter by (the plain search stands).
    pub understood: Option<String>,
    pub songs: Vec<TrackMeta>,
    /// Why there are no filters, or why nothing could match.
    pub note: Option<String>,
}

/// Why a description could not be searched, or a mix made.
#[derive(Debug)]
pub enum Failure {
    /// The server's AI refused or failed.
    Ai(Error),
    Server(Error),
    Other(String),
}

impl Failure {
    /// The server no longer accepts the token.
    pub fn is_unauthorized(&self) -> bool {
        matches!(
            self,
            Self::Ai(Error::Unauthorized) | Self::Server(Error::Unauthorized)
        )
    }

    /// What the failure says of the server's AI: a 404 is a server without
    /// music AI yet, a 503 one with AI off.
    pub fn availability(&self) -> Option<Ai> {
        match self {
            Self::Ai(Error::NotFound { .. }) => Some(Ai::Missing),
            Self::Ai(Error::Http { status: 503, .. }) => Some(Ai::Off),
            _ => None,
        }
    }
}

/// The filters in words, for "Understood as …".
pub fn understood(query: &MusicFilters) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(term) = &query.search_term {
        parts.push(format!("“{term}”"));
    }
    if !query.genres.is_empty() {
        parts.push(query.genres.join(", "));
    }
    match (query.from_year, query.to_year) {
        (Some(a), Some(b)) if a == b => parts.push(a.to_string()),
        (Some(a), Some(b)) => parts.push(format!("{a}–{b}")),
        (Some(a), None) => parts.push(format!("from {a}")),
        (None, Some(b)) => parts.push(format!("up to {b}")),
        (None, None) => {}
    }
    if query.liked_only {
        parts.push("liked".to_owned());
    }
    let order = match query.ordering {
        MusicOrdering::ByName => None,
        MusicOrdering::MostPlayed => Some("most played first"),
        MusicOrdering::NewestFirst => Some("newest first"),
        MusicOrdering::Random => Some("in random order"),
        MusicOrdering::RecentlyPlayed => Some("recently played first"),
    };
    parts.extend(order.map(str::to_owned));
    parts.join(" · ")
}

/// The years of the library inside the query's range: the server's own list
/// where it gave one, else the range spelled out. `None` for no year filter;
/// an empty list means no year of the library is in range (no years would
/// read as every year).
pub fn years_in_range(
    query: &MusicFilters,
    library_years: &[i32],
    this_year: i32,
) -> Option<Vec<i32>> {
    if query.from_year.is_none() && query.to_year.is_none() {
        return None;
    }
    let from = query.from_year.unwrap_or(i32::MIN);
    let to = query.to_year.unwrap_or(i32::MAX);
    let mut years: Vec<i32> = if library_years.is_empty() {
        let to = to.min(this_year);
        let from = from.max(to - MAX_YEARS + 1);
        (from..=to).collect()
    } else {
        library_years
            .iter()
            .copied()
            .filter(|y| (from..=to).contains(y))
            .collect()
    };
    years.sort_unstable();
    years.dedup();
    Some(years)
}

/// The filter for the server's filters: the named genres as the library
/// spells them, the years, likes, the term. `None` when the years rule every
/// song out.
pub fn song_filter(
    query: &MusicFilters,
    library_genres: &[String],
    library_years: &[i32],
    this_year: i32,
) -> Option<SongFilter> {
    let years = match years_in_range(query, library_years, this_year) {
        Some(years) if years.is_empty() => return None,
        Some(years) => years,
        None => Vec::new(),
    };
    let genres = query
        .genres
        .iter()
        .filter_map(|name| {
            library_genres
                .iter()
                .find(|g| g.eq_ignore_ascii_case(name))
                .cloned()
        })
        .collect();
    Some(SongFilter {
        term: query.search_term.clone(),
        genres,
        years,
        liked_only: query.liked_only,
        sort: match query.ordering {
            MusicOrdering::ByName => Sort::Name,
            _ => Sort::Default,
        },
    })
}

/// The songs in the order asked for, at most [`DESCRIBED_SONGS`]: by name as
/// Melete sent them, else newest, most or last played (this device's
/// history), or shuffled.
async fn ordered(songs: Vec<Item>, ordering: MusicOrdering, store: &StoreHandle) -> Vec<Item> {
    let mut songs = match ordering {
        MusicOrdering::ByName => songs,
        MusicOrdering::Random => crate::screens::home::pick(songs, DESCRIBED_SONGS),
        MusicOrdering::NewestFirst => {
            let mut songs = songs;
            songs.sort_by_key(|song| std::cmp::Reverse(song.year));
            songs
        }
        MusicOrdering::MostPlayed | MusicOrdering::RecentlyPlayed => {
            let ids: Vec<String> = songs.iter().map(|song| song.id.clone()).collect();
            let stats = store
                .call(move |w| w.store.stats_for(&ids).unwrap_or_default())
                .await
                .unwrap_or_default();
            let mut songs = songs;
            songs.sort_by_key(|song| {
                let stat = stats.get(&song.id);
                std::cmp::Reverse(match ordering {
                    MusicOrdering::MostPlayed => stat.map_or(0, |s| s.play_count),
                    _ => stat.and_then(|s| s.last_played_at).unwrap_or(0),
                })
            });
            songs
        }
    };
    songs.truncate(DESCRIBED_SONGS);
    songs
}

fn this_year() -> i32 {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400);
    // Close enough for a bound: 1970 plus whole Julian years.
    1970 + i32::try_from(days * 4 / 1461).unwrap_or(0)
}

/// A phrase, through the server's AI, to Melete's songs.
pub async fn search_by_description(
    client: MeleteClient,
    store: StoreHandle,
    phrase: String,
) -> Result<Described, Failure> {
    let asked = client.music_filters(&phrase).await.map_err(Failure::Ai)?;
    let query = grounded(&asked, &phrase);
    if query.is_empty() {
        return Ok(Described {
            understood: None,
            songs: Vec::new(),
            note: Some(
                "That description gave nothing to filter by, so the plain search stands."
                    .to_owned(),
            ),
        });
    }
    let filters = crate::song_filter::library_filters(&client)
        .await
        .map_err(Failure::Server)?;
    let words = understood(&query);
    let Some(filter) = song_filter(&query, &filters.genres, &filters.years, this_year()) else {
        return Ok(Described {
            understood: Some(words),
            songs: Vec::new(),
            note: Some("Nothing in your library is from those years.".to_owned()),
        });
    };
    let pool = match query.ordering {
        MusicOrdering::ByName => DESCRIBED_SONGS,
        _ => ORDER_POOL,
    };
    let songs = filter
        .songs(&client, Some(pool))
        .await
        .map_err(Failure::Server)?;
    let songs = ordered(songs, query.ordering, &store).await;
    Ok(Described {
        understood: Some(words),
        songs: songs.iter().map(TrackMeta::from_item).collect(),
        note: None,
    })
}

#[cfg(test)]
mod tests;
