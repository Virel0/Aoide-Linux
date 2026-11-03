//! Entries for songs the library does not have yet.

use super::items::positions;
use crate::Millis;
use crate::error::Result;
use crate::models::{PendingSong, PlaylistItem};
use crate::store::Store;

impl Store {
    /// Appends songs that are not in the library yet, in the order given, each as a
    /// waiting entry. Melete swaps the track in once its request brings it, and the
    /// next sync brings that here.
    ///
    /// # Errors
    ///
    /// [`PlaylistError::NoSuchPlaylist`](super::PlaylistError::NoSuchPlaylist) when there
    /// is no live playlist, or a database error.
    pub fn add_waiting(
        &self,
        songs: &[PendingSong],
        playlist_id: &str,
        now: Millis,
    ) -> Result<Vec<PlaylistItem>> {
        self.require_playlist(playlist_id)?;
        let last = self.last_position(playlist_id)?;
        let mut created = Vec::with_capacity(songs.len());
        for (song, position) in songs
            .iter()
            .zip(positions(songs.len(), last.as_deref(), None))
        {
            let mut item = PlaylistItem::new(playlist_id, "", "", position, now, self.device_id());
            item.pending = Some(song.clone());
            created.push(self.record(item, false, now)?);
        }
        Ok(created)
    }
}
