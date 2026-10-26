//! Copying a Jellyfin playlist into a local one.

use std::collections::BTreeSet;

use super::TrackLink;
use crate::Millis;
use crate::error::Result;
use crate::models::Playlist;
use crate::record::Record;
use crate::store::Store;

impl Store {
    /// The local copy of a Jellyfin playlist, if one has been imported.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn imported_playlist(&self, source_jellyfin_id: &str) -> Result<Option<Playlist>> {
        Ok(self
            .read(|conn| {
                Playlist::fetch_where(
                    conn,
                    "sourceJellyfinId = ? AND deleted = 0 ORDER BY rowid",
                    [source_jellyfin_id],
                )
            })?
            .into_iter()
            .next())
    }

    /// Which Jellyfin playlists already have a live local copy.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn imported_source_ids(&self) -> Result<BTreeSet<String>> {
        self.read(|conn| {
            let mut statement =
                conn.prepare("SELECT sourceJellyfinId FROM playlist WHERE sourceJellyfinId IS NOT NULL AND deleted = 0")?;
            let rows = statement.query_map([], |row| row.get(0))?;
            rows.collect()
        })
    }

    /// Copies a Jellyfin playlist into a local one, or refreshes the copy that exists —
    /// a snapshot, replaced outright on refresh, with every removal an op so the other
    /// devices drop the old contents too. Never automatic: an automatic read-back of
    /// an exported playlist would loop.
    ///
    /// # Errors
    ///
    /// Fails when the database does; what was written before the failure stays.
    pub fn import_playlist(
        &self,
        name: &str,
        source_jellyfin_id: &str,
        tracks: &[TrackLink],
        now: Millis,
    ) -> Result<Playlist> {
        let playlist = match self.imported_playlist(source_jellyfin_id)? {
            Some(existing) => {
                for item in self.items(&existing.id)? {
                    self.remove_item(&item.id, now)?;
                }
                existing
            }
            None => {
                let mut created = self.create_playlist(name, None, None, now)?;
                created.source_jellyfin_id = Some(source_jellyfin_id.to_owned());
                self.record(created, false, now)?
            }
        };
        self.add_tracks(tracks, &playlist.id, now)?;
        Ok(playlist)
    }
}
