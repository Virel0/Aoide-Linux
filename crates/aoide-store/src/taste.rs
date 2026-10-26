//! What this listener has been listening to, as numbers something can sort by.

use std::collections::{HashMap, HashSet};

use rusqlite::{params, params_from_iter};

use crate::Millis;
use crate::error::Result;
use crate::finish_rate::{FinishCounts, FinishRate};
use crate::history::sql_limit;
use crate::record::placeholders;
use crate::store::Store;

/// Built from local history: the server counts a play when audio starts, so its idea of
/// a favourite includes everything ever skipped past. A plain value, so the rules that
/// use it can be exercised against a profile written by hand.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TasteProfile {
    /// Genre (lower-cased) to how strongly it figures, 0…1 with the strongest at 1.
    pub genres: HashMap<String, f64>,
    /// The same for artists.
    pub artists: HashMap<String, f64>,
    /// Heard recently enough that hearing it again would feel like a repeat.
    pub recent: HashSet<String>,
    /// Whose records those were, lower-cased.
    pub recent_artists: HashSet<String>,
}

impl TasteProfile {
    /// Whether there is no genre or artist history to rank by.
    pub fn is_empty(&self) -> bool {
        self.genres.is_empty() && self.artists.is_empty()
    }
}

/// One song being considered, described by the little that matters here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TasteCandidate {
    /// The track's Jellyfin id.
    pub id: String,
    /// The track's artist.
    pub artist: String,
    /// The track's genres, as tagged.
    pub genres: Vec<String>,
}

impl TasteCandidate {
    /// A candidate from its id, artist and genres.
    pub fn new<S: AsRef<str>>(
        id: impl Into<String>,
        artist: impl Into<String>,
        genres: &[S],
    ) -> Self {
        Self {
            id: id.into(),
            artist: artist.into(),
            genres: genres
                .iter()
                .map(|genre| genre.as_ref().to_owned())
                .collect(),
        }
    }
}

/// Puts candidates in the order this listener is most likely to want them. Arithmetic
/// over what the history says, so every position can be explained in one sentence —
/// and held to the sixteen-row parity table the phone and the desktop share.
pub struct TasteRanking;

impl TasteRanking {
    /// What the best-matching genre's weight (0 to 1) is worth.
    pub const GENRE_WEIGHT: f64 = 0.45;
    /// Less than genre, on purpose: a queue should widen out from what is playing.
    pub const ARTIST_WEIGHT: f64 = 0.3;
    /// What a finish rate away from one half is worth, either way.
    pub const FINISH_WEIGHT: f64 = 0.3;
    /// Wider than the rest of the scale, so everything heard lately sits below
    /// everything that has not been.
    pub const RECENT_PENALTY: f64 = 1.4;
    /// Taken off a track whose artist was heard recently.
    pub const SAME_ARTIST_PENALTY: f64 = 0.4;
    /// How much mixability with what is playing is worth when a set is programmed.
    pub const COMPATIBILITY_WEIGHT: f64 = 0.6;

    /// How much this listener is likely to want `candidate` now: higher is better, with no
    /// fixed range. `finish` is the track's finish counts and `compatibility` (0 to 1) how
    /// well it mixes with what is playing, when known.
    pub fn score(
        candidate: &TasteCandidate,
        profile: &TasteProfile,
        finish: Option<FinishCounts>,
        compatibility: Option<f64>,
    ) -> f64 {
        // The best-matching genre rather than the sum: six tags are not six times as
        // relevant as the right one.
        let genre = candidate
            .genres
            .iter()
            .filter_map(|genre| profile.genres.get(&genre.to_lowercase()).copied())
            .fold(None, |best: Option<f64>, weight| {
                Some(best.map_or(weight, |b| b.max(weight)))
            })
            .unwrap_or(0.0);
        let artist_key = candidate.artist.to_lowercase();
        let artist = profile.artists.get(&artist_key).copied().unwrap_or(0.0);
        // Too little history is neither promoted nor punished.
        let finish_bias = finish.and_then(FinishRate::rate).unwrap_or(0.5) - 0.5;

        let mut score = genre * Self::GENRE_WEIGHT
            + artist * Self::ARTIST_WEIGHT
            + finish_bias * 2.0 * Self::FINISH_WEIGHT;
        if profile.recent.contains(&candidate.id) {
            score -= Self::RECENT_PENALTY;
        }
        if profile.recent_artists.contains(&artist_key) {
            score -= Self::SAME_ARTIST_PENALTY;
        }
        if let Some(compatibility) = compatibility {
            score += compatibility.clamp(0.0, 1.0) * Self::COMPATIBILITY_WEIGHT;
        }
        score
    }

    /// The best `limit` candidates, best first; ties break on id so the same history
    /// produces the same queue twice.
    pub fn rank(
        candidates: &[TasteCandidate],
        profile: &TasteProfile,
        finish: &HashMap<String, FinishCounts>,
        compatibility: &HashMap<String, f64>,
        limit: usize,
    ) -> Vec<TasteCandidate> {
        if limit == 0 {
            return Vec::new();
        }
        let mut scored: Vec<(&TasteCandidate, f64)> = candidates
            .iter()
            .map(|candidate| {
                let score = Self::score(
                    candidate,
                    profile,
                    finish.get(&candidate.id).copied(),
                    compatibility.get(&candidate.id).copied(),
                );
                (candidate, score)
            })
            .collect();
        scored.sort_by(|(left, left_score), (right, right_score)| {
            right_score
                .partial_cmp(left_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| left.id.cmp(&right.id))
        });
        scored
            .into_iter()
            .take(limit)
            .map(|(candidate, _)| candidate.clone())
            .collect()
    }
}

impl Store {
    /// Reads a profile out of the history: the latest `depth` finished listens since
    /// `since`, and the `recent_depth` most recent tracks of any outcome as "just heard".
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn taste_profile(
        &self,
        since: Millis,
        depth: usize,
        recent_depth: usize,
    ) -> Result<TasteProfile> {
        let rows: Vec<(Option<String>, Option<String>)> = self.read(|conn| {
            let mut statement = conn.prepare(
                "SELECT track.artist, track.genres FROM playEvent \
                 LEFT JOIN track ON track.jellyfinId = playEvent.jellyfinId \
                 WHERE playEvent.startedAt >= ? AND playEvent.completed = 1 \
                 ORDER BY playEvent.startedAt DESC LIMIT ?",
            )?;
            let rows = statement.query_map(params![since, sql_limit(depth)], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?;
            rows.collect()
        })?;

        let mut genre_counts: HashMap<String, f64> = HashMap::new();
        let mut artist_counts: HashMap<String, f64> = HashMap::new();
        for (artist, genres) in rows {
            if let Some(artist) = artist.filter(|artist| !artist.is_empty()) {
                *artist_counts.entry(artist.to_lowercase()).or_default() += 1.0;
            }
            let Some(decoded) = genres.and_then(|g| serde_json::from_str::<Vec<String>>(&g).ok())
            else {
                continue;
            };
            for genre in decoded.into_iter().filter(|genre| !genre.is_empty()) {
                *genre_counts.entry(genre.to_lowercase()).or_default() += 1.0;
            }
        }

        // Against the strongest rather than the total: a listener with two genres and
        // one with twenty should both have a 1.
        fn normalised(counts: HashMap<String, f64>) -> HashMap<String, f64> {
            let top = counts.values().copied().fold(0.0, f64::max);
            if top <= 0.0 {
                return HashMap::new();
            }
            counts
                .into_iter()
                .map(|(key, count)| (key, count / top))
                .collect()
        }

        // Everything lately, finished or not: a song skipped an hour ago is still a
        // song you have just heard.
        let recent = self.recently_played(recent_depth)?;
        let recent_artists: HashSet<String> = if recent.is_empty() {
            HashSet::new()
        } else {
            self.read(|conn| {
                let mut statement = conn.prepare(&format!(
                    "SELECT artist FROM track WHERE jellyfinId IN ({})",
                    placeholders(recent.len())
                ))?;
                let rows = statement
                    .query_map(params_from_iter(&recent), |row| row.get::<_, String>(0))?;
                rows.collect::<rusqlite::Result<Vec<String>>>()
            })?
            .into_iter()
            .map(|artist| artist.to_lowercase())
            .filter(|artist| !artist.is_empty())
            .collect()
        };

        Ok(TasteProfile {
            genres: normalised(genre_counts),
            artists: normalised(artist_counts),
            recent: recent.into_iter().collect(),
            recent_artists,
        })
    }
}
