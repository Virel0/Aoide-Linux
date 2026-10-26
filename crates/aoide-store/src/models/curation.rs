//! Folders, playlists and their items: the rows a listener arranges by hand.

use rusqlite::Row;
use rusqlite::types::Value as SqlValue;
use serde::{Deserialize, Serialize};

use crate::Millis;
use crate::field_stamps::FieldStamps;
use crate::json::flag as lenient;
use crate::new_id;
use crate::record::{Record, SyncRecord, flag, int, opt_text, text};

/// A folder in the playlist tree. Merged field by field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    /// The row id.
    pub id: String,
    /// The name shown.
    pub name: String,
    /// The folder it sits in; `None` at the top level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// A `FractionalIndex` key ordering it among all folders.
    pub sort_index: String,
    /// When the row last changed, in milliseconds since the epoch.
    pub updated_at: Millis,
    /// Whether the folder has been deleted (a tombstone).
    #[serde(deserialize_with = "lenient::deserialize")]
    pub deleted: bool,
    /// The device that made the last change.
    pub origin_device: String,
    /// When each field last changed. Absent on rows written before per-field merging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_updated_at: Option<FieldStamps>,
}

impl Folder {
    /// A new top-level folder with a fresh id.
    pub fn new(
        name: impl Into<String>,
        sort_index: impl Into<String>,
        updated_at: Millis,
        origin_device: impl Into<String>,
    ) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            parent_id: None,
            sort_index: sort_index.into(),
            updated_at,
            deleted: false,
            origin_device: origin_device.into(),
            field_updated_at: None,
        }
    }
}

impl Record for Folder {
    const TABLE: &'static str = "folder";
    const KEY: &'static str = "id";
    const COLUMNS: &'static [&'static str] = &[
        "id",
        "name",
        "parentId",
        "sortIndex",
        "updatedAt",
        "deleted",
        "originDevice",
        "fieldUpdatedAt",
    ];

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            name: row.get("name")?,
            parent_id: row.get("parentId")?,
            sort_index: row.get("sortIndex")?,
            updated_at: row.get("updatedAt")?,
            deleted: row.get("deleted")?,
            origin_device: row.get("originDevice")?,
            field_updated_at: row.get("fieldUpdatedAt")?,
        })
    }

    fn values(&self) -> Vec<SqlValue> {
        vec![
            text(&self.id),
            text(&self.name),
            opt_text(&self.parent_id),
            text(&self.sort_index),
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

impl SyncRecord for Folder {
    const FIELD_STAMPED: bool = true;
    sync_meta!();

    fn field_stamps(&self) -> Option<&FieldStamps> {
        self.field_updated_at.as_ref()
    }

    fn set_field_stamps(&mut self, stamps: Option<FieldStamps>) {
        self.field_updated_at = stamps;
    }
}

/// A playlist: hand-made, smart, or imported from Jellyfin. Merged field by field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    /// The row id.
    pub id: String,
    /// The name shown.
    pub name: String,
    /// Free text the listener wrote about it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// The folder it is filed in; `None` at the top level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    /// Whether its contents come from `smart_rules` rather than its items.
    #[serde(deserialize_with = "lenient::deserialize")]
    pub is_smart: bool,
    /// The JSON rule set; absent unless `is_smart`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smart_rules: Option<String>,
    /// A `FractionalIndex` key ordering it among all playlists.
    pub sort_index: String,
    /// When the row last changed, in milliseconds since the epoch.
    pub updated_at: Millis,
    /// Whether the playlist has been deleted (a tombstone).
    #[serde(deserialize_with = "lenient::deserialize")]
    pub deleted: bool,
    /// The device that made the last change.
    pub origin_device: String,
    /// When each field last changed. Absent on rows written before per-field merging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_updated_at: Option<FieldStamps>,
    /// The Jellyfin playlist this was imported from, if it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_jellyfin_id: Option<String>,
    /// A Jellyfin item whose cover stands in for this playlist's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artwork_item_id: Option<String>,
    /// SHA-256 of a cover uploaded for this playlist; beats `artwork_item_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_hash: Option<String>,
    /// The uploaded cover's media type (`image/jpeg`, say); set with `image_hash`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_mime: Option<String>,
}

impl Playlist {
    /// A new, empty, hand-made playlist at the top level, with a fresh id.
    pub fn new(
        name: impl Into<String>,
        sort_index: impl Into<String>,
        updated_at: Millis,
        origin_device: impl Into<String>,
    ) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            notes: None,
            folder_id: None,
            is_smart: false,
            smart_rules: None,
            sort_index: sort_index.into(),
            updated_at,
            deleted: false,
            origin_device: origin_device.into(),
            field_updated_at: None,
            source_jellyfin_id: None,
            artwork_item_id: None,
            image_hash: None,
            image_mime: None,
        }
    }
}

impl Record for Playlist {
    const TABLE: &'static str = "playlist";
    const KEY: &'static str = "id";
    const COLUMNS: &'static [&'static str] = &[
        "id",
        "name",
        "notes",
        "folderId",
        "isSmart",
        "smartRules",
        "sortIndex",
        "updatedAt",
        "deleted",
        "originDevice",
        "fieldUpdatedAt",
        "sourceJellyfinId",
        "artworkItemId",
        "imageHash",
        "imageMime",
    ];

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            name: row.get("name")?,
            notes: row.get("notes")?,
            folder_id: row.get("folderId")?,
            is_smart: row.get("isSmart")?,
            smart_rules: row.get("smartRules")?,
            sort_index: row.get("sortIndex")?,
            updated_at: row.get("updatedAt")?,
            deleted: row.get("deleted")?,
            origin_device: row.get("originDevice")?,
            field_updated_at: row.get("fieldUpdatedAt")?,
            source_jellyfin_id: row.get("sourceJellyfinId")?,
            artwork_item_id: row.get("artworkItemId")?,
            image_hash: row.get("imageHash")?,
            image_mime: row.get("imageMime")?,
        })
    }

    fn values(&self) -> Vec<SqlValue> {
        vec![
            text(&self.id),
            text(&self.name),
            opt_text(&self.notes),
            opt_text(&self.folder_id),
            flag(self.is_smart),
            opt_text(&self.smart_rules),
            text(&self.sort_index),
            int(self.updated_at),
            flag(self.deleted),
            text(&self.origin_device),
            opt_text(&self.field_updated_at.as_ref().map(FieldStamps::to_column)),
            opt_text(&self.source_jellyfin_id),
            opt_text(&self.artwork_item_id),
            opt_text(&self.image_hash),
            opt_text(&self.image_mime),
        ]
    }

    fn key(&self) -> &str {
        &self.id
    }
}

impl SyncRecord for Playlist {
    const FIELD_STAMPED: bool = true;
    sync_meta!();

    fn field_stamps(&self) -> Option<&FieldStamps> {
        self.field_updated_at.as_ref()
    }

    fn set_field_stamps(&mut self, stamps: Option<FieldStamps>) {
        self.field_updated_at = stamps;
    }
}

/// One entry in a playlist. Merged whole-row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistItem {
    /// The row id.
    pub id: String,
    /// The playlist it belongs to.
    pub playlist_id: String,
    /// The track's Jellyfin id, or the last one that resolved with
    /// [`crate::UNRESOLVED_PREFIX`] before it when the track has gone missing.
    pub jellyfin_id: String,
    /// Stored beside the id because Jellyfin ids are derived per file: a rescan after
    /// files move mints new ones, and without this every entry would dangle.
    pub content_key: String,
    /// A `FractionalIndex` key, not an integer.
    pub position: String,
    /// When the row last changed, in milliseconds since the epoch.
    pub updated_at: Millis,
    /// Whether the entry has been removed (a tombstone).
    #[serde(deserialize_with = "lenient::deserialize")]
    pub deleted: bool,
    /// The device that made the last change.
    pub origin_device: String,
}

impl PlaylistItem {
    /// A new entry with a fresh id.
    pub fn new(
        playlist_id: impl Into<String>,
        jellyfin_id: impl Into<String>,
        content_key: impl Into<String>,
        position: impl Into<String>,
        updated_at: Millis,
        origin_device: impl Into<String>,
    ) -> Self {
        Self {
            id: new_id(),
            playlist_id: playlist_id.into(),
            jellyfin_id: jellyfin_id.into(),
            content_key: content_key.into(),
            position: position.into(),
            updated_at,
            deleted: false,
            origin_device: origin_device.into(),
        }
    }
}

impl Record for PlaylistItem {
    const TABLE: &'static str = "playlistItem";
    const KEY: &'static str = "id";
    const COLUMNS: &'static [&'static str] = &[
        "id",
        "playlistId",
        "jellyfinId",
        "contentKey",
        "position",
        "updatedAt",
        "deleted",
        "originDevice",
    ];

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            playlist_id: row.get("playlistId")?,
            jellyfin_id: row.get("jellyfinId")?,
            content_key: row.get("contentKey")?,
            position: row.get("position")?,
            updated_at: row.get("updatedAt")?,
            deleted: row.get("deleted")?,
            origin_device: row.get("originDevice")?,
        })
    }

    fn values(&self) -> Vec<SqlValue> {
        vec![
            text(&self.id),
            text(&self.playlist_id),
            text(&self.jellyfin_id),
            text(&self.content_key),
            text(&self.position),
            int(self.updated_at),
            flag(self.deleted),
            text(&self.origin_device),
        ]
    }

    fn key(&self) -> &str {
        &self.id
    }
}

impl SyncRecord for PlaylistItem {
    sync_meta!();
}
