//! Likes and the per-track taste flags.

use rusqlite::Row;
use rusqlite::types::Value as SqlValue;
use serde::{Deserialize, Serialize};

use crate::Millis;
use crate::field_stamps::FieldStamps;
use crate::json::flag as lenient;
use crate::new_id;
use crate::record::{Record, SyncRecord, flag, int, opt_text, text};

/// A listener's verdict on a track: a like or a dislike. Merged whole-row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Like {
    /// The row id.
    pub id: String,
    /// The track's id.
    pub jellyfin_id: String,
    /// The track's [`ContentKey`](crate::ContentKey).
    pub content_key: String,
    /// True for a like, false for a dislike.
    #[serde(deserialize_with = "lenient::deserialize")]
    pub liked: bool,
    /// When the row last changed, in milliseconds since the epoch.
    pub updated_at: Millis,
    /// Whether the verdict has been withdrawn (a tombstone).
    #[serde(deserialize_with = "lenient::deserialize")]
    pub deleted: bool,
    /// The device that made the last change.
    pub origin_device: String,
}

impl Like {
    /// A verdict with a fresh id.
    pub fn new(
        jellyfin_id: impl Into<String>,
        content_key: impl Into<String>,
        liked: bool,
        updated_at: Millis,
        origin_device: impl Into<String>,
    ) -> Self {
        Self {
            id: new_id(),
            jellyfin_id: jellyfin_id.into(),
            content_key: content_key.into(),
            liked,
            updated_at,
            deleted: false,
            origin_device: origin_device.into(),
        }
    }
}

impl Record for Like {
    const TABLE: &'static str = "like";
    const KEY: &'static str = "id";
    const COLUMNS: &'static [&'static str] = &[
        "id",
        "jellyfinId",
        "contentKey",
        "liked",
        "updatedAt",
        "deleted",
        "originDevice",
    ];

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            jellyfin_id: row.get("jellyfinId")?,
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
            text(&self.jellyfin_id),
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

impl SyncRecord for Like {
    sync_meta!();
}

/// What a listener has said about a track beyond liking it: "never offer this to me",
/// and "a play of this is not a fact about my taste".
///
/// Its own row rather than fields on `Like`, because a like set on one device and a
/// flag set on another at the same moment must both survive — `Like` merges whole-row,
/// and this merges per field. One row per track; the unique index on `jellyfinId` is
/// what the merge dedupes on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackFlags {
    /// The row id.
    pub id: String,
    /// The track's id; unique among the rows.
    pub jellyfin_id: String,
    /// The track's [`ContentKey`](crate::ContentKey).
    pub content_key: String,
    /// Excluded from mixes, stations, smart-playlist results and shuffle.
    #[serde(deserialize_with = "lenient::deserialize")]
    pub not_interested: bool,
    /// New plays are not recorded; existing history is untouched.
    #[serde(deserialize_with = "lenient::deserialize")]
    pub dont_count: bool,
    /// When the row last changed, in milliseconds since the epoch.
    pub updated_at: Millis,
    /// Whether the row has been deleted (a tombstone).
    #[serde(deserialize_with = "lenient::deserialize")]
    pub deleted: bool,
    /// The device that made the last change.
    pub origin_device: String,
    /// When each flag last changed. Absent on rows written before per-field merging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_updated_at: Option<FieldStamps>,
}

impl TrackFlags {
    /// A row for a track with neither flag set, and a fresh id.
    pub fn new(
        jellyfin_id: impl Into<String>,
        content_key: impl Into<String>,
        updated_at: Millis,
        origin_device: impl Into<String>,
    ) -> Self {
        Self {
            id: new_id(),
            jellyfin_id: jellyfin_id.into(),
            content_key: content_key.into(),
            not_interested: false,
            dont_count: false,
            updated_at,
            deleted: false,
            origin_device: origin_device.into(),
            field_updated_at: None,
        }
    }

    /// A row with neither flag set says nothing; it is deleted rather than kept.
    pub fn is_empty(&self) -> bool {
        !self.not_interested && !self.dont_count
    }
}

impl Record for TrackFlags {
    const TABLE: &'static str = "trackFlags";
    const KEY: &'static str = "id";
    const COLUMNS: &'static [&'static str] = &[
        "id",
        "jellyfinId",
        "contentKey",
        "notInterested",
        "dontCount",
        "updatedAt",
        "deleted",
        "originDevice",
        "fieldUpdatedAt",
    ];

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            jellyfin_id: row.get("jellyfinId")?,
            content_key: row.get("contentKey")?,
            not_interested: row.get("notInterested")?,
            dont_count: row.get("dontCount")?,
            updated_at: row.get("updatedAt")?,
            deleted: row.get("deleted")?,
            origin_device: row.get("originDevice")?,
            field_updated_at: row.get("fieldUpdatedAt")?,
        })
    }

    fn values(&self) -> Vec<SqlValue> {
        vec![
            text(&self.id),
            text(&self.jellyfin_id),
            text(&self.content_key),
            flag(self.not_interested),
            flag(self.dont_count),
            int(self.updated_at),
            flag(self.deleted),
            text(&self.origin_device),
            opt_text(&self.field_updated_at.as_ref().map(FieldStamps::to_column)),
        ]
    }

    fn key(&self) -> &str {
        &self.id
    }
}

impl SyncRecord for TrackFlags {
    const FIELD_STAMPED: bool = true;
    sync_meta!();

    fn field_stamps(&self) -> Option<&FieldStamps> {
        self.field_updated_at.as_ref()
    }

    fn set_field_stamps(&mut self, stamps: Option<FieldStamps>) {
        self.field_updated_at = stamps;
    }
}
