//! Creating, renaming, moving, reordering and deleting playlists.

use super::{PlaylistError, check_gap, last_key};
use crate::Millis;
use crate::error::Result;
use crate::fractional_index::FractionalIndex;
use crate::models::Playlist;
use crate::new_id;
use crate::record::Record;
use crate::store::Store;

impl Store {
    /// Live playlists in display order, across every folder. Sort keys are generated
    /// against all playlists, so a folder's contents are a subsequence of this.
    pub fn playlists(&self) -> Result<Vec<Playlist>> {
        self.read(|conn| Playlist::fetch_where(conn, "deleted = 0 ORDER BY sortIndex, rowid", []))
    }

    /// Live playlists filed directly under `folder_id`; `None` is the root.
    pub fn playlists_in_folder(&self, folder_id: Option<&str>) -> Result<Vec<Playlist>> {
        self.read(|conn| {
            Playlist::fetch_where(
                conn,
                "deleted = 0 AND folderId IS ? ORDER BY sortIndex, rowid",
                [folder_id],
            )
        })
    }

    /// A live playlist; a deleted one is `None`.
    pub fn playlist(&self, id: &str) -> Result<Option<Playlist>> {
        Ok(self
            .read(|conn| Playlist::fetch(conn, id))?
            .filter(|list| !list.deleted))
    }

    /// Whether a row with this id has ever existed here, deleted or not — so a deleted
    /// built-in is not created again under the same id and undeleted everywhere.
    pub fn playlist_row_exists(&self, id: &str) -> Result<bool> {
        Ok(self.read(|conn| Playlist::fetch(conn, id))?.is_some())
    }

    pub fn create_playlist(
        &self,
        name: &str,
        folder_id: Option<&str>,
        notes: Option<&str>,
        now: Millis,
    ) -> Result<Playlist> {
        self.create_playlist_with_id(&new_id(), name, folder_id, notes, now)
    }

    /// A caller-chosen id is for the built-ins, which every device creates under the
    /// same id so they merge rather than duplicate.
    pub fn create_playlist_with_id(
        &self,
        id: &str,
        name: &str,
        folder_id: Option<&str>,
        notes: Option<&str>,
        now: Millis,
    ) -> Result<Playlist> {
        if let Some(folder_id) = folder_id {
            self.require_folder(folder_id)?;
        }
        let mut list = Playlist::new(
            name,
            self.next_playlist_sort_index()?,
            now,
            self.device_id(),
        );
        list.id = id.to_owned();
        list.notes = notes.map(str::to_owned);
        list.folder_id = folder_id.map(str::to_owned);
        self.record(list, false, now)
    }

    /// Renames in place. The whole row goes into the op, and the receiving side merges
    /// per field, so a concurrent reorder elsewhere survives.
    pub fn rename_playlist(&self, id: &str, name: &str, now: Millis) -> Result<Playlist> {
        let mut list = self.require_playlist(id)?;
        list.name = name.to_owned();
        self.record(list, false, now)
    }

    pub fn set_notes(
        &self,
        notes: Option<&str>,
        playlist_id: &str,
        now: Millis,
    ) -> Result<Playlist> {
        let mut list = self.require_playlist(playlist_id)?;
        list.notes = notes.map(str::to_owned);
        self.record(list, false, now)
    }

    /// Files a playlist under a folder, or at the root, landing at the end of it.
    pub fn move_playlist(
        &self,
        id: &str,
        folder_id: Option<&str>,
        now: Millis,
    ) -> Result<Playlist> {
        let mut list = self.require_playlist(id)?;
        if let Some(folder_id) = folder_id {
            self.require_folder(folder_id)?;
        }
        list.folder_id = folder_id.map(str::to_owned);
        list.sort_index = self.next_playlist_sort_index()?;
        self.record(list, false, now)
    }

    /// Soft-deletes a playlist **and its items**, one op each, items first — so a crash
    /// half way leaves a shortened playlist rather than orphans. Deleting twice mints
    /// nothing.
    pub fn delete_playlist(&self, id: &str, now: Millis) -> Result<()> {
        let list = self
            .read(|conn| Playlist::fetch(conn, id))?
            .ok_or_else(|| PlaylistError::NoSuchPlaylist(id.to_owned()))?;
        if list.deleted {
            return Ok(());
        }
        for item in self.items(id)? {
            self.record(item, true, now)?;
        }
        self.record(list, true, now)?;
        Ok(())
    }

    /// Removes a playlist and its entries **from this device only**, writing no op,
    /// along with any of their changes still waiting to be pushed. For a playlist
    /// someone shared with this user whose share is gone: the contract says to drop
    /// it locally, and a delete op would only be refused. Reports whether there was a
    /// row. Never for a playlist this user owns: every other device keeps it, and
    /// this one would not hear of it again.
    pub fn drop_playlist_locally(&self, id: &str) -> Result<bool> {
        self.write_local(|tx| {
            tx.execute(
                "DELETE FROM op WHERE synced = 0 AND ((entity = 'playlist' AND entityId = ?1) \
                 OR (entity = 'playlistItem' AND entityId IN \
                     (SELECT id FROM playlistItem WHERE playlistId = ?1)))",
                [id],
            )?;
            tx.execute("DELETE FROM playlistItem WHERE playlistId = ?", [id])?;
            Ok(tx.execute("DELETE FROM playlist WHERE id = ?", [id])? > 0)
        })
    }

    /// Repositions a playlist between two siblings; `None` on either side is the end.
    /// Passing neither leaves it where it is.
    pub fn reorder_playlist(
        &self,
        id: &str,
        after: Option<&str>,
        before: Option<&str>,
        now: Millis,
    ) -> Result<Playlist> {
        let mut list = self.require_playlist(id)?;
        if after.is_none() && before.is_none() {
            return Ok(list);
        }
        if after == Some(id) || before == Some(id) {
            return Err(PlaylistError::BadNeighbours.into());
        }
        let lower = after
            .map(|n| self.require_playlist(n).map(|p| p.sort_index))
            .transpose()?;
        let upper = before
            .map(|n| self.require_playlist(n).map(|p| p.sort_index))
            .transpose()?;
        check_gap(lower.as_deref(), upper.as_deref())?;
        list.sort_index = FractionalIndex::between(lower.as_deref(), upper.as_deref());
        self.record(list, false, now)
    }
}

impl Store {
    fn next_playlist_sort_index(&self) -> Result<String> {
        let last = self.read(|conn| {
            last_key(
                conn,
                "SELECT sortIndex FROM playlist WHERE deleted = 0 ORDER BY sortIndex DESC LIMIT 1",
                None,
            )
        })?;
        Ok(FractionalIndex::between(last.as_deref(), None))
    }
}
