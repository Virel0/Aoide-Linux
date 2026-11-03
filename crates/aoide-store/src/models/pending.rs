//! A song a playlist holds before the library has it: a waiting entry.
//!
//! On the wire a waiting entry is a `playlist_items` row with no `jellyfinId` and no
//! `contentKey`, but a `pending` object naming the song and the request that will
//! bring it. Once the song arrives, Melete writes the same row again with the track's
//! id and content key and without `pending`, and the merge takes it as any later edit.

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ValueRef};
use serde::{Deserialize, Serialize};

use super::PlaylistItem;
use crate::Millis;
use crate::json::flag as lenient;

/// The song a waiting entry stands for. Every field is written, `null` when unknown,
/// as Melete writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingSong {
    /// The song request that will bring it; `None` when nothing could be asked for,
    /// and then it waits for good.
    pub request_id: Option<i64>,
    /// Its title.
    pub title: String,
    /// Its artists, comma-separated.
    pub artist: Option<String>,
    /// The album it is on.
    pub album: Option<String>,
    /// Its length in milliseconds.
    pub duration_ms: Option<i64>,
    /// Why it could not be asked for; `None` when it was.
    pub why: Option<String>,
}

impl PendingSong {
    /// A song waiting on `request_id`, known only by its title and artist.
    pub fn requested(request_id: i64, title: impl Into<String>, artist: Option<String>) -> Self {
        Self {
            request_id: Some(request_id),
            title: title.into(),
            artist,
            album: None,
            duration_ms: None,
            why: None,
        }
    }

    /// The column's text: the payload's own JSON, keys sorted.
    pub(crate) fn to_column(&self) -> String {
        crate::json::payload(self).unwrap_or_default()
    }
}

impl FromSql for PendingSong {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let text = value.as_str()?;
        serde_json::from_str(text).map_err(|error| FromSqlError::Other(Box::new(error)))
    }
}

/// A playlist entry as the payload spells it: the track's two fields may be absent,
/// as they are on a waiting entry.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WireItem {
    id: String,
    playlist_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    jellyfin_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content_key: Option<String>,
    position: String,
    updated_at: Millis,
    #[serde(deserialize_with = "lenient::deserialize")]
    deleted: bool,
    origin_device: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending: Option<PendingSong>,
}

impl TryFrom<WireItem> for PlaylistItem {
    type Error = String;

    /// An entry needs a track or a waiting song; one with neither is corrupt, and
    /// failing here quarantines it rather than storing an entry that names nothing.
    fn try_from(wire: WireItem) -> Result<Self, Self::Error> {
        let has_track = wire.jellyfin_id.is_some() && wire.content_key.is_some();
        if !has_track && wire.pending.is_none() {
            return Err("a playlist entry needs jellyfinId and contentKey, or pending".to_owned());
        }
        Ok(Self {
            id: wire.id,
            playlist_id: wire.playlist_id,
            jellyfin_id: wire.jellyfin_id.unwrap_or_default(),
            content_key: wire.content_key.unwrap_or_default(),
            position: wire.position,
            updated_at: wire.updated_at,
            deleted: wire.deleted,
            origin_device: wire.origin_device,
            pending: wire.pending,
        })
    }
}

impl From<PlaylistItem> for WireItem {
    /// A track entry goes out exactly as before; a waiting one without the track's
    /// fields, which it does not have.
    fn from(item: PlaylistItem) -> Self {
        let waiting = item.is_waiting();
        let present = |value: String| (!waiting || !value.is_empty()).then_some(value);
        Self {
            id: item.id,
            playlist_id: item.playlist_id,
            jellyfin_id: present(item.jellyfin_id),
            content_key: present(item.content_key),
            position: item.position,
            updated_at: item.updated_at,
            deleted: item.deleted,
            origin_device: item.origin_device,
            pending: item.pending,
        }
    }
}

impl PlaylistItem {
    /// Whether this entry is a song the library does not have yet: it has a waiting
    /// song and no track. Such an entry cannot be played, and is skipped by relinking.
    pub fn is_waiting(&self) -> bool {
        self.pending.is_some() && self.jellyfin_id.is_empty()
    }
}
