//! A playlist's entries: reading, adding, moving and removing them.

use super::{PlaylistError, TrackLink, last_key};
use crate::Millis;
use crate::error::Result;
use crate::fractional_index::FractionalIndex;
use crate::models::PlaylistItem;
use crate::record::Record;
use crate::store::Store;

impl Store {
    /// Live entries of a playlist, in order.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn items(&self, playlist_id: &str) -> Result<Vec<PlaylistItem>> {
        self.read(|conn| {
            PlaylistItem::fetch_where(
                conn,
                "playlistId = ? AND deleted = 0 ORDER BY position, rowid",
                [playlist_id],
            )
        })
    }

    /// Counted in SQLite: a library view shows a count beside every playlist.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn item_count(&self, playlist_id: &str) -> Result<i64> {
        self.read(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM playlistItem WHERE playlistId = ? AND deleted = 0",
                [playlist_id],
                |row| row.get(0),
            )
        })
    }

    /// Appends tracks in the order given. A track already present is added again: a
    /// set list may play the same song twice.
    ///
    /// # Errors
    ///
    /// [`PlaylistError::NoSuchPlaylist`] when there is no live playlist, or a database
    /// error.
    pub fn add_tracks(
        &self,
        links: &[TrackLink],
        playlist_id: &str,
        now: Millis,
    ) -> Result<Vec<PlaylistItem>> {
        self.require_playlist(playlist_id)?;
        let last = self.last_position(playlist_id)?;
        self.insert_items(links, playlist_id, last.as_deref(), None, now)
    }

    /// Inserts tracks so the first of them ends up at `index`.
    ///
    /// # Errors
    ///
    /// [`PlaylistError::NoSuchPlaylist`] when there is no live playlist, or a database
    /// error.
    pub fn insert_tracks(
        &self,
        links: &[TrackLink],
        playlist_id: &str,
        index: usize,
        now: Millis,
    ) -> Result<Vec<PlaylistItem>> {
        self.require_playlist(playlist_id)?;
        let existing = self.items(playlist_id)?;
        let target = index.min(existing.len());
        let lower = (target > 0).then(|| existing[target - 1].position.clone());
        let upper = existing.get(target).map(|item| item.position.clone());
        self.insert_items(links, playlist_id, lower.as_deref(), upper.as_deref(), now)
    }

    /// Drag-to-reorder. Writes exactly one row. `index` is where the entry ends up,
    /// measured in the list with it already taken out.
    ///
    /// # Errors
    ///
    /// [`PlaylistError::NoSuchItem`] when there is no live entry,
    /// [`PlaylistError::ItemNotInPlaylist`] when it is in another playlist, or a database
    /// error.
    pub fn move_item(
        &self,
        item_id: &str,
        playlist_id: &str,
        index: usize,
        now: Millis,
    ) -> Result<PlaylistItem> {
        let mut item = self.require_item(item_id)?;
        if item.playlist_id != playlist_id {
            return Err(PlaylistError::ItemNotInPlaylist {
                item: item_id.to_owned(),
                playlist: playlist_id.to_owned(),
            }
            .into());
        }
        let others: Vec<PlaylistItem> = self
            .items(playlist_id)?
            .into_iter()
            .filter(|other| other.id != item_id)
            .collect();
        let target = index.min(others.len());
        let lower = (target > 0).then(|| others[target - 1].position.clone());
        let upper = others.get(target).map(|other| other.position.clone());

        // A drag that lands back where it started manufactures no op.
        let already_above = lower.as_ref().is_none_or(|lower| *lower < item.position);
        let already_below = upper.as_ref().is_none_or(|upper| item.position < *upper);
        if already_above && already_below {
            return Ok(item);
        }
        item.position = FractionalIndex::between(lower.as_deref(), upper.as_deref());
        self.record(item, false, now)
    }

    /// Soft-deletes an entry; removing it twice mints nothing.
    ///
    /// # Errors
    ///
    /// [`PlaylistError::NoSuchItem`] when no row has the id, deleted or not, or a database
    /// error.
    pub fn remove_item(&self, item_id: &str, now: Millis) -> Result<()> {
        let item = self
            .read(|conn| PlaylistItem::fetch(conn, item_id))?
            .ok_or_else(|| PlaylistError::NoSuchItem(item_id.to_owned()))?;
        if item.deleted {
            return Ok(());
        }
        self.record(item, true, now)?;
        Ok(())
    }
}

impl Store {
    fn require_item(&self, id: &str) -> Result<PlaylistItem> {
        self.read(|conn| PlaylistItem::fetch(conn, id))?
            .filter(|item| !item.deleted)
            .ok_or_else(|| PlaylistError::NoSuchItem(id.to_owned()).into())
    }

    fn last_position(&self, playlist_id: &str) -> Result<Option<String>> {
        self.read(|conn| {
            last_key(
                conn,
                "SELECT position FROM playlistItem WHERE playlistId = ? AND deleted = 0 ORDER BY position DESC LIMIT 1",
                Some(playlist_id),
            )
        })
    }

    fn insert_items(
        &self,
        links: &[TrackLink],
        playlist_id: &str,
        lower: Option<&str>,
        upper: Option<&str>,
        now: Millis,
    ) -> Result<Vec<PlaylistItem>> {
        let mut created = Vec::with_capacity(links.len());
        for (link, position) in links.iter().zip(positions(links.len(), lower, upper)) {
            created.push(self.record(
                PlaylistItem::new(
                    playlist_id,
                    &link.jellyfin_id,
                    &link.content_key,
                    position,
                    now,
                    self.device_id(),
                ),
                false,
                now,
            )?);
        }
        Ok(created)
    }
}

/// Appending counts upwards; inserting into the middle bisects what is left of the gap.
fn positions(count: usize, lower: Option<&str>, upper: Option<&str>) -> Vec<String> {
    let Some(upper) = upper else {
        return FractionalIndex::sequence(lower, count);
    };
    let mut keys = Vec::with_capacity(count);
    let mut previous = lower.map(str::to_owned);
    for _ in 0..count {
        let key = FractionalIndex::between(previous.as_deref(), Some(upper));
        keys.push(key.clone());
        previous = Some(key);
    }
    keys
}
