//! Reading and filling the local mirror of the library.
//!
//! A smart playlist's whole universe *is* this table, so a track it has never heard of
//! cannot match any rule. Local, rebuilt by each device from its own connection, and
//! never synced.

use std::collections::HashMap;

use rusqlite::params_from_iter;

use crate::Millis;
use crate::error::Result;
use crate::history::MAX_PARAMETERS;
use crate::models::TrackRef;
use crate::record::{Record, placeholders};
use crate::store::{Store, set_state};

const INDEX_KEY: &str = "trackIndexCompletedAt";

impl Store {
    /// Writes library rows into the cache in one transaction — one fsync for a pass,
    /// not thousands.
    pub fn cache_tracks(&self, tracks: &[TrackRef]) -> Result<()> {
        if tracks.is_empty() {
            return Ok(());
        }
        self.write_local(|tx| {
            for track in tracks {
                track.save(tx)?;
            }
            Ok(())
        })
    }

    /// Cached rows for these ids. Ids with nothing cached are simply absent.
    pub fn cached_tracks(&self, jellyfin_ids: &[String]) -> Result<HashMap<String, TrackRef>> {
        if jellyfin_ids.is_empty() {
            return Ok(HashMap::new());
        }
        self.read(|conn| {
            let mut found = HashMap::new();
            for chunk in jellyfin_ids.chunks(MAX_PARAMETERS) {
                let rows = TrackRef::fetch_where(
                    conn,
                    &format!("jellyfinId IN ({})", placeholders(chunk.len())),
                    params_from_iter(chunk),
                )?;
                for row in rows {
                    found.insert(row.jellyfin_id.clone(), row);
                }
            }
            Ok(found)
        })
    }

    /// The size of every smart playlist's universe. Not the way to ask whether the
    /// index has run — playback writes rows here too; see `library_indexed_at`.
    pub fn cached_track_count(&self) -> Result<i64> {
        self.read(TrackRef::count)
    }

    /// Drops cached rows a completed index pass did not see. A hard delete, correct only
    /// because this table is local; the cutoff must come from before the pass began.
    pub fn prune_tracks(&self, not_seen_since: Millis) -> Result<usize> {
        self.write_local(|tx| {
            tx.execute("DELETE FROM track WHERE lastSeenAt < ?", [not_seen_since])
        })
    }

    /// When a full library index last finished here, if one ever has.
    pub fn library_indexed_at(&self) -> Result<Option<Millis>> {
        Ok(self
            .state(INDEX_KEY)?
            .and_then(|stored| stored.parse().ok()))
    }

    /// Records that a full pass completed.
    pub fn record_library_index(&self, now: Millis) -> Result<()> {
        self.write_local(|tx| set_state(tx, INDEX_KEY, &now.to_string()))
    }
}
