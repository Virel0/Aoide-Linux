//! Recording and aggregating listening history. What a play *is* lives in
//! `play_definition`, shared with the smart-playlist evaluator.

use std::collections::HashMap;

use rusqlite::{Connection, params, params_from_iter};

use crate::Millis;
use crate::activity::Activity;
use crate::error::Result;
use crate::finish_rate::FinishCounts;
use crate::json;
use crate::models::{Op, OpKind, PlayEvent, PlaySource};
use crate::play_definition::{PlayClassifier, PlayDefinition};
use crate::playlists::TrackLink;
use crate::record::{Record, placeholders};
use crate::smart::Calendar;
use crate::store::Store;

/// SQLite on older systems caps host parameters at 999, and an id list here comes
/// straight from a screenful of rows; chunking is not an optimisation.
pub(crate) const MAX_PARAMETERS: usize = 400;

/// Every aggregate reads the track cache through this one alias, so the play and skip
/// fragments always have a `t` to take the duration from.
const EVENTS_WITH_TRACK: &str = "FROM playEvent e LEFT JOIN track t ON t.jellyfinId = e.jellyfinId";

/// A single album, keyed. Unit separator cannot appear in a tag, so two real albums
/// never merge on a printable separator.
const ALBUM_KEY: &str = "t.album || char(31) || COALESCE(t.albumArtist, t.artist)";

fn qualifies_as_play() -> String {
    PlayDefinition::counts_as_play("e", "t")
}

fn qualifies_as_skip() -> String {
    PlayDefinition::counts_as_skip("e", "t")
}

/// What is known about one track's listening.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackStats {
    /// The track.
    pub jellyfin_id: String,
    /// Events that qualified as plays.
    pub play_count: i64,
    /// Events that qualified as skips.
    pub skip_count: i64,
    /// Every millisecond heard, skipped fragments included.
    pub total_ms_played: i64,
    /// The first and last events that *qualified as plays* — the figures the smart
    /// fields `first_played` and `last_played` read.
    pub first_played_at: Option<Millis>,
    /// The last of those, in milliseconds since the epoch.
    pub last_played_at: Option<Millis>,
    /// The last event of any kind: when this track was last *on*.
    pub last_started_at: Option<Millis>,
}

impl TrackStats {
    /// A track nothing is known about.
    pub fn empty(jellyfin_id: impl Into<String>) -> Self {
        Self {
            jellyfin_id: jellyfin_id.into(),
            play_count: 0,
            skip_count: 0,
            total_ms_played: 0,
            first_played_at: None,
            last_played_at: None,
            last_started_at: None,
        }
    }

    /// Whether this track has ever been touched — a track skipped four times has a
    /// history.
    pub fn has_history(&self) -> bool {
        self.last_started_at.is_some()
    }
}

/// One track's plays in a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackPlays {
    /// The track.
    pub jellyfin_id: String,
    /// Events that qualified as plays.
    pub play_count: i64,
}

/// One artist's plays in a window, as the track cache names them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistPlays {
    /// The artist.
    pub artist: String,
    /// Events that qualified as plays.
    pub play_count: i64,
}

/// Carries the artist because an album title is not a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumPlays {
    /// The album's title.
    pub album: String,
    /// The album's artist (the track's when it has none).
    pub artist: String,
    /// Events that qualified as plays.
    pub play_count: i64,
}

/// One calendar day's plays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayPlays {
    /// The day, `YYYY-MM-DD`.
    pub day: String,
    /// Events that qualified as plays.
    pub play_count: i64,
}

/// A wrapped-style summary of one window of listening.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recap {
    /// Half-open: `from` included, `to` excluded.
    pub from: Millis,
    /// The window's end, excluded.
    pub to: Millis,
    /// Events that qualified as plays.
    pub total_plays: i64,
    /// Every millisecond listened to, skipped fragments included.
    pub total_ms_played: i64,
    /// Distinct tracks among those plays.
    pub distinct_tracks: i64,
    /// Distinct artists among those plays whose track is in the cache.
    pub distinct_artists: i64,
    /// Distinct albums among those plays whose track is in the cache.
    pub distinct_albums: i64,
    /// The most-played tracks, most first.
    pub top_tracks: Vec<TrackPlays>,
    /// The most-played artists, most first.
    pub top_artists: Vec<ArtistPlays>,
    /// The most-played albums, most first.
    pub top_albums: Vec<AlbumPlays>,
    /// Bucketed in the device's local time zone: the day you listened most is a human
    /// day.
    pub busiest_day: Option<DayPlays>,
    /// Plays whose track is missing from the cache, so in no artist or album figure.
    pub unattributed_plays: i64,
}

// MARK: - Recording

impl Store {
    /// Opens a play. The event lands immediately with `ms_played` 0; `finish_play`
    /// fills in how it went. Written at the start because an app killed mid-track is
    /// normal, and the long listens are the ones that would otherwise be lost.
    ///
    /// # Errors
    ///
    /// Fails when the database does; nothing is written then.
    pub fn begin_play(
        &self,
        jellyfin_id: &str,
        content_key: &str,
        source: PlaySource,
        activity: Option<Activity>,
        now: Millis,
    ) -> Result<PlayEvent> {
        self.append(
            PlayEvent::new(jellyfin_id, content_key, now, self.device_id())
                .with_source(source)
                .with_activity(activity),
        )
    }

    /// Closes a play, classifying it and amending the event `begin_play` wrote.
    ///
    /// The amendment *replaces* this device's unpushed op for the event, under a
    /// **new** op id: an op the server accepted while its acknowledgement was lost is
    /// ignored on re-push, so an amended payload under the old id would vanish. If the
    /// begin op had already synced, this lands after it in sequence order.
    ///
    /// # Errors
    ///
    /// Fails when the event does not encode or the database fails; nothing is written then.
    pub fn finish_play(
        &self,
        event: &PlayEvent,
        ms_played: i64,
        track_duration_ms: Option<i64>,
        ended_at: Millis,
        completed: Option<bool>,
    ) -> Result<PlayEvent> {
        let outcome = PlayClassifier::classify(ms_played, track_duration_ms, completed);
        let mut amended = event.clone();
        amended.ended_at = Some(ended_at);
        // Clamped: a negative figure would poison every SUM over the column.
        amended.ms_played = ms_played.max(0);
        amended.completed = outcome.completed;
        amended.skipped = outcome.skipped;

        let op = Op::new(
            PlayEvent::TABLE,
            &amended.id,
            OpKind::Upsert,
            json::payload(&amended)?,
            ended_at,
        );
        self.transaction(|tx| {
            amended.save(tx)?;
            tx.execute(
                "DELETE FROM \"op\" WHERE \"entity\" = ? AND \"entityId\" = ? AND \"synced\" = 0",
                params![PlayEvent::TABLE, amended.id],
            )?;
            op.insert(tx)?;
            Ok(())
        })?;
        Ok(amended)
    }

    /// Records a play that is already over — an import, or a player that reports once.
    ///
    /// # Errors
    ///
    /// As [`Self::append`].
    pub fn record_play(
        &self,
        track: &TrackLink,
        started_at: Millis,
        ms_played: i64,
        track_duration_ms: Option<i64>,
        source: PlaySource,
        completed: Option<bool>,
    ) -> Result<PlayEvent> {
        let outcome = PlayClassifier::classify(ms_played, track_duration_ms, completed);
        let played = ms_played.max(0);
        self.append(
            PlayEvent::new(
                &track.jellyfin_id,
                &track.content_key,
                started_at,
                self.device_id(),
            )
            .with_source(source)
            .finished(
                Some(started_at + played),
                played,
                outcome.completed,
                outcome.skipped,
            ),
        )
    }

    /// Finish counts split by what the listener was doing, ignoring untagged listens
    /// and words this build does not know.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn finish_counts_by_activity(
        &self,
        jellyfin_ids: &[String],
    ) -> Result<HashMap<Activity, FinishCounts>> {
        let mut by_activity: HashMap<Activity, FinishCounts> = HashMap::new();
        if jellyfin_ids.is_empty() {
            return Ok(by_activity);
        }
        let rows: Vec<(String, FinishCounts)> = self.read(|conn| {
            let mut found = Vec::new();
            for chunk in jellyfin_ids.chunks(MAX_PARAMETERS) {
                let mut statement = conn.prepare(&format!(
                    "SELECT activity, COUNT(*) AS starts, \
                            SUM(CASE WHEN completed = 1 THEN 1 ELSE 0 END) AS completed \
                     FROM playEvent \
                     WHERE endedAt IS NOT NULL AND (completed = 1 OR skipped = 1) \
                       AND activity IS NOT NULL AND jellyfinId IN ({}) \
                     GROUP BY activity",
                    placeholders(chunk.len())
                ))?;
                let mapped = statement.query_map(params_from_iter(chunk), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        FinishCounts::new(row.get(1)?, row.get(2)?),
                    ))
                })?;
                for row in mapped {
                    found.push(row?);
                }
            }
            Ok(found)
        })?;
        for (raw, counts) in rows {
            let Some(activity) = Activity::parse(Some(&raw)) else {
                continue;
            };
            let entry = by_activity.entry(activity).or_default();
            *entry = *entry + counts;
        }
        Ok(by_activity)
    }
}

// MARK: - Aggregates

impl Store {
    /// Counts only events that qualify as plays — unlike a server's play count.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn play_count(&self, jellyfin_id: &str) -> Result<i64> {
        Ok(self.stats(jellyfin_id)?.play_count)
    }

    /// The batched query with one id, so a count on a row and the sheet behind it
    /// cannot disagree.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn stats(&self, jellyfin_id: &str) -> Result<TrackStats> {
        let ids = [jellyfin_id.to_owned()];
        Ok(self
            .stats_for(&ids)?
            .remove(jellyfin_id)
            .unwrap_or_else(|| TrackStats::empty(jellyfin_id)))
    }

    /// Stats for many tracks in one query. Every requested id comes back, zeroed if it
    /// has no history, so a list view has no misses to fall back on.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn stats_for(&self, jellyfin_ids: &[String]) -> Result<HashMap<String, TrackStats>> {
        if jellyfin_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let found = self.read(|conn| {
            let mut found: HashMap<String, TrackStats> = HashMap::new();
            let (play, skip) = (qualifies_as_play(), qualifies_as_skip());
            for chunk in jellyfin_ids.chunks(MAX_PARAMETERS) {
                let sql = format!(
                    "SELECT e.jellyfinId AS jellyfinId, \
                            SUM(CASE WHEN {play} THEN 1 ELSE 0 END) AS playCount, \
                            SUM(CASE WHEN {skip} THEN 1 ELSE 0 END) AS skipCount, \
                            COALESCE(SUM(e.msPlayed), 0) AS totalMsPlayed, \
                            MIN(CASE WHEN {play} THEN e.startedAt END) AS firstPlayedAt, \
                            MAX(CASE WHEN {play} THEN e.startedAt END) AS lastPlayedAt, \
                            MAX(e.startedAt) AS lastStartedAt \
                     {EVENTS_WITH_TRACK} \
                     WHERE e.jellyfinId IN ({}) \
                     GROUP BY e.jellyfinId",
                    placeholders(chunk.len())
                );
                let mut statement = conn.prepare(&sql)?;
                let rows = statement.query_map(params_from_iter(chunk), |row| {
                    Ok(TrackStats {
                        jellyfin_id: row.get("jellyfinId")?,
                        play_count: row.get("playCount")?,
                        skip_count: row.get("skipCount")?,
                        total_ms_played: row.get("totalMsPlayed")?,
                        first_played_at: row.get("firstPlayedAt")?,
                        last_played_at: row.get("lastPlayedAt")?,
                        last_started_at: row.get("lastStartedAt")?,
                    })
                })?;
                for row in rows {
                    let stats = row?;
                    found.insert(stats.jellyfin_id.clone(), stats);
                }
            }
            Ok(found)
        })?;
        Ok(jellyfin_ids
            .iter()
            .map(|id| {
                (
                    id.clone(),
                    found
                        .get(id)
                        .cloned()
                        .unwrap_or_else(|| TrackStats::empty(id)),
                )
            })
            .collect())
    }

    /// The share of decided outcomes that were skips, or `None` below the sample
    /// floor. The neither band is in neither the numerator nor the denominator.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn skip_rate(&self, jellyfin_id: &str) -> Result<Option<f64>> {
        let stats = self.stats(jellyfin_id)?;
        let decided = stats.play_count + stats.skip_count;
        if decided < PlayClassifier::MINIMUM_SKIP_SAMPLE {
            return Ok(None);
        }
        Ok(Some(stats.skip_count as f64 / decided as f64))
    }

    /// The `limit` most-played tracks since `since` (epoch milliseconds), most first,
    /// ties by id.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn top_tracks(&self, since: Millis, limit: usize) -> Result<Vec<TrackPlays>> {
        self.read(|conn| top_tracks(conn, since, Millis::MAX, limit))
    }

    /// The `limit` most-played artists since `since`, most first, ties by name.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn top_artists(&self, since: Millis, limit: usize) -> Result<Vec<ArtistPlays>> {
        self.read(|conn| top_artists(conn, since, Millis::MAX, limit))
    }

    /// The `limit` most-played albums since `since`, most first, ties by title.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn top_albums(&self, since: Millis, limit: usize) -> Result<Vec<AlbumPlays>> {
        self.read(|conn| top_albums(conn, since, Millis::MAX, limit))
    }

    /// Plays in the window whose track is not in the local cache.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn unattributed_plays(&self, since: Millis, to: Millis) -> Result<i64> {
        self.read(|conn| unattributed_plays(conn, since, to))
    }

    /// Distinct tracks, most recently started first — skipped and abandoned included:
    /// "recently played" is a history of what was on, not a leaderboard.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn recently_played(&self, limit: usize) -> Result<Vec<String>> {
        self.read(|conn| {
            let mut statement = conn.prepare(
                "SELECT e.jellyfinId FROM playEvent e GROUP BY e.jellyfinId \
                 ORDER BY MAX(e.startedAt) DESC LIMIT ?",
            )?;
            let rows = statement.query_map([sql_limit(limit)], |row| row.get(0))?;
            rows.collect()
        })
    }

    /// A wrapped-style summary of `[from, to)`, read in one consistent snapshot, with
    /// the busiest day bucketed in this device's time zone.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn recap(&self, from: Millis, to: Millis) -> Result<Recap> {
        self.recap_in(from, to, Calendar::Local)
    }

    /// [`recap`](Self::recap), with the busiest day bucketed in `calendar`.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn recap_in(&self, from: Millis, to: Millis, calendar: Calendar) -> Result<Recap> {
        let zone = match calendar {
            Calendar::Local => ", 'localtime'",
            Calendar::Utc => "",
        };
        self.read(|conn| {
            let play = qualifies_as_play();
            let (total_plays, distinct_tracks, distinct_artists, distinct_albums) = conn.query_row(
                &format!(
                    "SELECT COUNT(*), COUNT(DISTINCT e.jellyfinId), COUNT(DISTINCT t.artist), \
                            COUNT(DISTINCT {ALBUM_KEY}) \
                     {EVENTS_WITH_TRACK} \
                     WHERE e.startedAt >= ? AND e.startedAt < ? AND {play}"
                ),
                params![from, to],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
            let total_ms_played = conn.query_row(
                "SELECT COALESCE(SUM(msPlayed), 0) FROM playEvent WHERE startedAt >= ? AND startedAt < ?",
                params![from, to],
                |row| row.get(0),
            )?;
            let busiest_day = {
                let mut statement = conn.prepare(&format!(
                    "SELECT strftime('%Y-%m-%d', e.startedAt / 1000, 'unixepoch'{zone}) AS day, \
                            COUNT(*) AS playCount \
                     {EVENTS_WITH_TRACK} \
                     WHERE e.startedAt >= ? AND e.startedAt < ? AND {play} \
                     GROUP BY day ORDER BY playCount DESC, day LIMIT 1"
                ))?;
                let mut rows = statement.query_map(params![from, to], |row| {
                    Ok(DayPlays { day: row.get(0)?, play_count: row.get(1)? })
                })?;
                rows.next().transpose()?
            };
            Ok(Recap {
                from,
                to,
                total_plays,
                total_ms_played,
                distinct_tracks,
                distinct_artists,
                distinct_albums,
                top_tracks: top_tracks(conn, from, to, 10)?,
                top_artists: top_artists(conn, from, to, 10)?,
                top_albums: top_albums(conn, from, to, 10)?,
                busiest_day,
                unattributed_plays: unattributed_plays(conn, from, to)?,
            })
        })
    }
}

// Ordering ties break on the name or id so a chart does not reshuffle between runs.

fn top_tracks(
    conn: &Connection,
    from: Millis,
    to: Millis,
    limit: usize,
) -> rusqlite::Result<Vec<TrackPlays>> {
    let mut statement = conn.prepare(&format!(
        "SELECT e.jellyfinId, COUNT(*) AS playCount {EVENTS_WITH_TRACK} \
         WHERE e.startedAt >= ? AND e.startedAt < ? AND {} \
         GROUP BY e.jellyfinId ORDER BY playCount DESC, e.jellyfinId LIMIT ?",
        qualifies_as_play()
    ))?;
    let rows = statement.query_map(params![from, to, sql_limit(limit)], |row| {
        Ok(TrackPlays {
            jellyfin_id: row.get(0)?,
            play_count: row.get(1)?,
        })
    })?;
    rows.collect()
}

/// Grouped on the track artist, not the album artist: "Various Artists" would be the
/// loudest entry in most recaps and say nothing.
fn top_artists(
    conn: &Connection,
    from: Millis,
    to: Millis,
    limit: usize,
) -> rusqlite::Result<Vec<ArtistPlays>> {
    let mut statement = conn.prepare(&format!(
        "SELECT t.artist, COUNT(*) AS playCount \
         FROM playEvent e JOIN track t ON t.jellyfinId = e.jellyfinId \
         WHERE e.startedAt >= ? AND e.startedAt < ? AND {} \
         GROUP BY t.artist ORDER BY playCount DESC, t.artist LIMIT ?",
        qualifies_as_play()
    ))?;
    let rows = statement.query_map(params![from, to, sql_limit(limit)], |row| {
        Ok(ArtistPlays {
            artist: row.get(0)?,
            play_count: row.get(1)?,
        })
    })?;
    rows.collect()
}

fn top_albums(
    conn: &Connection,
    from: Millis,
    to: Millis,
    limit: usize,
) -> rusqlite::Result<Vec<AlbumPlays>> {
    let mut statement = conn.prepare(&format!(
        "SELECT t.album, COALESCE(t.albumArtist, t.artist) AS artist, COUNT(*) AS playCount \
         FROM playEvent e JOIN track t ON t.jellyfinId = e.jellyfinId \
         WHERE e.startedAt >= ? AND e.startedAt < ? AND {} \
         GROUP BY {ALBUM_KEY} ORDER BY playCount DESC, t.album LIMIT ?",
        qualifies_as_play()
    ))?;
    let rows = statement.query_map(params![from, to, sql_limit(limit)], |row| {
        Ok(AlbumPlays {
            album: row.get(0)?,
            artist: row.get(1)?,
            play_count: row.get(2)?,
        })
    })?;
    rows.collect()
}

fn unattributed_plays(conn: &Connection, from: Millis, to: Millis) -> rusqlite::Result<i64> {
    conn.query_row(
        &format!(
            "SELECT COUNT(*) {EVENTS_WITH_TRACK} \
             WHERE e.startedAt >= ? AND e.startedAt < ? AND t.jellyfinId IS NULL AND {}",
            qualifies_as_play()
        ),
        params![from, to],
        |row| row.get(0),
    )
}

pub(crate) fn sql_limit(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
}

// MARK: - Weighted shuffle

/// Turns history into a selection weight: familiarity `1 + 0.5·log₂(1 + plays)`, skips
/// `1 − skips/(plays + skips + 2)`, taste ×1.75 liked, ×0.35 unliked, floored and never
/// zeroed — a weight of zero presents as a song that disappeared from the library.
pub struct PlayWeighting;

impl PlayWeighting {
    /// The least weight a track can have.
    pub const FLOOR: f64 = 0.05;
    /// What a liked track's weight is multiplied by.
    pub const LIKED_FACTOR: f64 = 1.75;
    /// What a disliked track's weight is multiplied by.
    pub const UNLIKED_FACTOR: f64 = 0.35;
    /// Pseudo-counts: one imagined play and one imagined skip.
    pub const SKIP_PRIOR: f64 = 2.0;

    /// The weight for a track with `plays` plays, `skips` skips and a like (`Some(true)`),
    /// dislike (`Some(false)`) or neither.
    pub fn weight(plays: i64, skips: i64, liked: Option<bool>) -> f64 {
        let (plays, skips) = (plays.max(0) as f64, skips.max(0) as f64);
        let familiarity = 1.0 + 0.5 * (1.0 + plays).log2();
        let skip_ratio = skips / (plays + skips + Self::SKIP_PRIOR);
        let taste = liked.map_or(1.0, |liked| {
            if liked {
                Self::LIKED_FACTOR
            } else {
                Self::UNLIKED_FACTOR
            }
        });
        (familiarity * (1.0 - skip_ratio) * taste).max(Self::FLOOR)
    }
}

impl Store {
    /// A selection weight per track, for weighted shuffle. Every requested id gets one.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn play_weights(&self, jellyfin_ids: &[String]) -> Result<HashMap<String, f64>> {
        if jellyfin_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let stats = self.stats_for(jellyfin_ids)?;
        let likes: HashMap<String, bool> = self.read(|conn| {
            let mut found = HashMap::new();
            for chunk in jellyfin_ids.chunks(MAX_PARAMETERS) {
                let mut statement = conn.prepare(&format!(
                    "SELECT jellyfinId, liked FROM \"like\" WHERE deleted = 0 AND jellyfinId IN ({})",
                    placeholders(chunk.len())
                ))?;
                let rows = statement.query_map(params_from_iter(chunk), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
                })?;
                for row in rows {
                    let (id, liked) = row?;
                    found.insert(id, liked);
                }
            }
            Ok(found)
        })?;
        Ok(jellyfin_ids
            .iter()
            .map(|id| {
                let (plays, skips) = stats
                    .get(id)
                    .map_or((0, 0), |s| (s.play_count, s.skip_count));
                (
                    id.clone(),
                    PlayWeighting::weight(plays, skips, likes.get(id).copied()),
                )
            })
            .collect())
    }
}
