//! Taste flags: "not interested" and "don't count".

use std::collections::BTreeSet;

use rusqlite::params_from_iter;

use crate::Millis;
use crate::error::Result;
use crate::history::MAX_PARAMETERS;
use crate::models::{TrackFlags, TrackRef};
use crate::record::{Record, placeholders};
use crate::store::Store;

/// Which of the two flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Flag {
    /// "Not interested": kept out of mixes, stations, smart playlists and shuffle.
    NotInterested,
    /// "Don't count": new plays are not recorded.
    DontCount,
}

impl Store {
    /// The flags on a track, or `None` when nothing has been said about it.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn flags(&self, jellyfin_id: &str) -> Result<Option<TrackFlags>> {
        Ok(self
            .read(|conn| {
                TrackFlags::fetch_where(conn, "jellyfinId = ? AND deleted = 0", [jellyfin_id])
            })?
            .into_iter()
            .next())
    }

    /// Which of these tracks are marked not interested.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn not_interested(&self, jellyfin_ids: &[String]) -> Result<BTreeSet<String>> {
        if jellyfin_ids.is_empty() {
            return Ok(BTreeSet::new());
        }
        self.read(|conn| {
            let mut found = BTreeSet::new();
            for chunk in jellyfin_ids.chunks(MAX_PARAMETERS) {
                let mut statement = conn.prepare(&format!(
                    "SELECT jellyfinId FROM trackFlags \
                     WHERE deleted = 0 AND notInterested = 1 AND jellyfinId IN ({})",
                    placeholders(chunk.len())
                ))?;
                for row in statement.query_map(params_from_iter(chunk), |row| row.get(0))? {
                    found.insert(row?);
                }
            }
            Ok(found)
        })
    }

    /// Every flagged track, newest first, with what the cache knows of it.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn flagged_tracks(&self) -> Result<Vec<(TrackFlags, Option<TrackRef>)>> {
        self.read(|conn| {
            let rows = TrackFlags::fetch_where(
                conn,
                "deleted = 0 ORDER BY updatedAt DESC, rowid DESC",
                [],
            )?;
            rows.into_iter()
                .map(|row| {
                    let track = TrackRef::fetch(conn, &row.jellyfin_id)?;
                    Ok((row, track))
                })
                .collect()
        })
    }

    /// Sets one flag, minting the row if needed and deleting it when both end up off —
    /// a row that says nothing is not worth syncing. A deleted row coming back keeps
    /// its id, so other devices see one row change rather than a delete and a stranger.
    ///
    /// # Errors
    ///
    /// Fails when the database does; nothing is written then.
    pub fn set_flag(
        &self,
        flag: Flag,
        value: bool,
        jellyfin_id: &str,
        content_key: &str,
        now: Millis,
    ) -> Result<TrackFlags> {
        let existing = self
            .read(|conn| TrackFlags::fetch_where(conn, "jellyfinId = ?", [jellyfin_id]))?
            .into_iter()
            .next();
        let mut row = existing
            .unwrap_or_else(|| TrackFlags::new(jellyfin_id, content_key, now, self.device_id()));
        row.deleted = false;
        match flag {
            Flag::NotInterested => row.not_interested = value,
            Flag::DontCount => row.dont_count = value,
        }
        let empty = row.is_empty();
        self.record(row, empty, now)
    }

    /// Sets or clears "not interested"; see [`Self::set_flag`].
    ///
    /// # Errors
    ///
    /// As [`Self::set_flag`].
    pub fn set_not_interested(
        &self,
        value: bool,
        jellyfin_id: &str,
        content_key: &str,
        now: Millis,
    ) -> Result<TrackFlags> {
        self.set_flag(Flag::NotInterested, value, jellyfin_id, content_key, now)
    }

    /// Sets or clears "don't count"; see [`Self::set_flag`].
    ///
    /// # Errors
    ///
    /// As [`Self::set_flag`].
    pub fn set_dont_count(
        &self,
        value: bool,
        jellyfin_id: &str,
        content_key: &str,
        now: Millis,
    ) -> Result<TrackFlags> {
        self.set_flag(Flag::DontCount, value, jellyfin_id, content_key, now)
    }
}
