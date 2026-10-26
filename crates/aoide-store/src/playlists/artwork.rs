//! What a playlist's cover is drawn from.

use std::collections::HashMap;

use rusqlite::params_from_iter;

use crate::Millis;
use crate::error::Result;
use crate::models::Playlist;
use crate::record::placeholders;
use crate::store::Store;

impl Store {
    /// Sets or clears the item whose cover stands in for a playlist's. `None` when the
    /// playlist is gone. A chosen picture is cleared with it, as on the phone: left
    /// behind, its hash would keep winning and choosing an album would look like it
    /// did nothing.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn set_artwork(
        &self,
        item_id: Option<&str>,
        playlist_id: &str,
        now: Millis,
    ) -> Result<Option<Playlist>> {
        let Some(mut list) = self.playlist(playlist_id)? else {
            return Ok(None);
        };
        list.artwork_item_id = item_id.map(str::to_owned);
        list.image_hash = None;
        list.image_mime = None;
        Ok(Some(self.record(list, false, now)?))
    }

    /// Up to `limit` distinct albums per playlist, in playing order, for drawing a cover
    /// from what it holds — for every playlist in one query.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn cover_albums(
        &self,
        playlist_ids: &[String],
        limit: usize,
    ) -> Result<HashMap<String, Vec<String>>> {
        if playlist_ids.is_empty() {
            return Ok(HashMap::new());
        }
        self.read(|conn| {
            let mut covers: HashMap<String, Vec<String>> = HashMap::new();
            for chunk in playlist_ids.chunks(crate::history::MAX_PARAMETERS) {
                let mut statement = conn.prepare(&format!(
                    "SELECT i.playlistId, t.albumId FROM playlistItem i \
                     JOIN track t ON t.jellyfinId = i.jellyfinId \
                     WHERE i.deleted = 0 AND t.albumId IS NOT NULL AND i.playlistId IN ({}) \
                     ORDER BY i.playlistId, i.position",
                    placeholders(chunk.len())
                ))?;
                let rows = statement.query_map(params_from_iter(chunk), |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
                for row in rows {
                    let (playlist_id, album_id) = row?;
                    let albums = covers.entry(playlist_id).or_default();
                    if albums.len() < limit && !albums.contains(&album_id) {
                        albums.push(album_id);
                    }
                }
            }
            Ok(covers)
        })
    }
}
