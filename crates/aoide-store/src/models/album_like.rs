//! Likes of albums: a sync record of their own, so a build that predates them sets
//! one aside rather than reading it as a track's like.

use rusqlite::Row;
use rusqlite::types::Value as SqlValue;
use serde::{Deserialize, Serialize};

use crate::Millis;
use crate::json::flag as lenient;
use crate::record::{Record, SyncRecord, flag, int, text};

/// A listener's like of an album. The same row and the same whole-row merge as a
/// track's [`Like`](super::Like), naming the album by `albumId` in place of
/// `jellyfinId`. Liking an album likes none of its tracks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumLike {
    /// The row id.
    pub id: String,
    /// The album's id.
    pub album_id: String,
    /// `artist|album`, folded as a track's [`ContentKey`](crate::ContentKey) is.
    pub content_key: String,
    /// True for a like; Melete and the apps write only likes.
    #[serde(deserialize_with = "lenient::deserialize")]
    pub liked: bool,
    /// When the row last changed, in milliseconds since the epoch.
    pub updated_at: Millis,
    /// Whether the like has been withdrawn (a tombstone).
    #[serde(deserialize_with = "lenient::deserialize")]
    pub deleted: bool,
    /// The device that made the last change.
    pub origin_device: String,
}

impl Record for AlbumLike {
    const TABLE: &'static str = "albumLike";
    const KEY: &'static str = "id";
    const COLUMNS: &'static [&'static str] = &[
        "id",
        "albumId",
        "contentKey",
        "liked",
        "updatedAt",
        "deleted",
        "originDevice",
    ];

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            album_id: row.get("albumId")?,
            content_key: row.get("contentKey")?,
            liked: row.get("liked")?,
            updated_at: row.get("updatedAt")?,
            deleted: row.get("deleted")?,
            origin_device: row.get("originDevice")?,
        })
    }

    fn values(&self) -> Vec<SqlValue> {
        vec![
            text(&self.id),
            text(&self.album_id),
            text(&self.content_key),
            flag(self.liked),
            int(self.updated_at),
            flag(self.deleted),
            text(&self.origin_device),
        ]
    }

    fn key(&self) -> &str {
        &self.id
    }
}

impl SyncRecord for AlbumLike {
    sync_meta!();
}
