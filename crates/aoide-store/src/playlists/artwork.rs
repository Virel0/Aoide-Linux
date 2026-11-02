//! What a playlist's cover is drawn from.

use std::collections::HashMap;

use rusqlite::params_from_iter;

use crate::Millis;
use crate::error::Result;
use crate::models::Playlist;
use crate::record::placeholders;
use crate::smart::Calendar;
use crate::store::Store;

/// How many matched tracks are looked up at a time while looking for a smart
/// playlist's albums: the first few usually have them all.
const ALBUM_LOOKUP: usize = 50;

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

    /// Up to `limit` distinct albums of what a smart playlist's rules match as of
    /// `now`, in the rules' order, from the track cache: what its cover is drawn
    /// from. Tracks the cache has no album for are passed over, and the matches
    /// are looked up a few at a time, stopping once `limit` are found.
    ///
    /// # Errors
    ///
    /// As [`Store::resolve_smart`], and when the database fails.
    pub fn smart_cover_albums(
        &self,
        playlist: &Playlist,
        now: Millis,
        calendar: Calendar,
        limit: usize,
    ) -> Result<Vec<String>> {
        let matched = self.resolve_smart(playlist, now, calendar)?;
        let mut albums: Vec<String> = Vec::new();
        for chunk in matched.chunks(ALBUM_LOOKUP) {
            if albums.len() >= limit {
                break;
            }
            let cached = self.cached_tracks(chunk)?;
            for id in chunk {
                let Some(album) = cached.get(id).and_then(|t| t.album_id.as_ref()) else {
                    continue;
                };
                if albums.len() < limit && !albums.contains(album) {
                    albums.push(album.clone());
                }
            }
        }
        Ok(albums)
    }
}
