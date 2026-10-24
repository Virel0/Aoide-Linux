//! The rows, and how each maps to its table and its payload.
//!
//! Field names on the wire are the phone's: `camelCase`, an empty optional left out
//! rather than written as `null`, flags as JSON booleans. The phone's JSON decoder and
//! this one both ignore keys they do not know, so a column either side adds later
//! rides along instead of failing the row.

use rusqlite::Row;
use rusqlite::types::Value as SqlValue;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use crate::Millis;
use crate::activity::Activity;
use crate::field_stamps::FieldStamps;
use crate::json::flag as lenient;
use crate::new_id;
use crate::record::{Record, SyncRecord, flag, int, opt_int, opt_text, text};

/// The four metadata accessors every syncable row implements identically.
macro_rules! sync_meta {
    () => {
        fn id(&self) -> &str {
            &self.id
        }

        fn updated_at(&self) -> Millis {
            self.updated_at
        }

        fn deleted(&self) -> bool {
            self.deleted
        }

        fn origin_device(&self) -> &str {
            &self.origin_device
        }

        fn set_meta(&mut self, updated_at: Millis, deleted: bool, origin_device: &str) {
            self.updated_at = updated_at;
            self.deleted = deleted;
            self.origin_device = origin_device.to_owned();
        }
    };
}

// MARK: - Track cache

/// A row of the local mirror of the library. Never synced: every device rebuilds it
/// from its own connection to the same server.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrackRef {
    pub jellyfin_id: String,
    pub content_key: String,
    pub musicbrainz_id: Option<String>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: Option<String>,
    /// The album's own id, for drawing a playlist cover from what it holds.
    pub album_id: Option<String>,
    pub duration_ms: Option<i64>,
    pub year: Option<i64>,
    /// A JSON array. Text because SQLite has no array type and it is only read whole.
    pub genres: String,
    pub last_seen_at: Millis,
}

impl TrackRef {
    pub fn new(
        jellyfin_id: impl Into<String>,
        content_key: impl Into<String>,
        title: impl Into<String>,
        artist: impl Into<String>,
        album: impl Into<String>,
        last_seen_at: Millis,
    ) -> Self {
        Self {
            jellyfin_id: jellyfin_id.into(),
            content_key: content_key.into(),
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            genres: "[]".to_owned(),
            last_seen_at,
            ..Self::default()
        }
    }

    pub fn with_genres<S: AsRef<str>>(mut self, genres: &[S]) -> Self {
        self.set_genres(genres);
        self
    }

    pub fn set_genres<S: AsRef<str>>(&mut self, genres: &[S]) {
        let list: Vec<&str> = genres.iter().map(AsRef::as_ref).collect();
        self.genres = serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_owned());
    }

    pub fn genre_list(&self) -> Vec<String> {
        serde_json::from_str(&self.genres).unwrap_or_default()
    }
}

impl Record for TrackRef {
    const TABLE: &'static str = "track";
    const KEY: &'static str = "jellyfinId";
    const COLUMNS: &'static [&'static str] = &[
        "jellyfinId",
        "contentKey",
        "musicbrainzId",
        "title",
        "artist",
        "album",
        "albumArtist",
        "albumId",
        "durationMs",
        "year",
        "genres",
        "lastSeenAt",
    ];

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            jellyfin_id: row.get("jellyfinId")?,
            content_key: row.get("contentKey")?,
            musicbrainz_id: row.get("musicbrainzId")?,
            title: row.get("title")?,
            artist: row.get("artist")?,
            album: row.get("album")?,
            album_artist: row.get("albumArtist")?,
            album_id: row.get("albumId")?,
            duration_ms: row.get("durationMs")?,
            year: row.get("year")?,
            genres: row.get("genres")?,
            last_seen_at: row.get("lastSeenAt")?,
        })
    }

    fn values(&self) -> Vec<SqlValue> {
        vec![
            text(&self.jellyfin_id),
            text(&self.content_key),
            opt_text(&self.musicbrainz_id),
            text(&self.title),
            text(&self.artist),
            text(&self.album),
            opt_text(&self.album_artist),
            opt_text(&self.album_id),
            opt_int(self.duration_ms),
            opt_int(self.year),
            text(&self.genres),
            int(self.last_seen_at),
        ]
    }

    fn key(&self) -> &str {
        &self.jellyfin_id
    }
}

/// The fingerprint that survives a Jellyfin rescan.
///
/// Normalised hard, because the point is to match the *same recording* after its id
/// changed. Duration is rounded to the second so a re-encode that shifts it by a few
/// milliseconds still matches.
pub struct ContentKey;

impl ContentKey {
    pub fn make(artist: &str, album: &str, title: &str, duration_ms: Option<i64>) -> String {
        let seconds = duration_ms.map_or(0, |ms| (ms + 500) / 1000);
        [
            normalise(artist),
            normalise(album),
            normalise(title),
            seconds.to_string(),
        ]
        .join("|")
    }
}

/// Diacritic- and case-insensitive folding, then trimmed, then one pass collapsing
/// double spaces — the phone's order of operations.
fn normalise(text: &str) -> String {
    let folded: String = text
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase();
    folded.trim().replace("  ", " ")
}

// MARK: - Curation

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub sort_index: String,
    pub updated_at: Millis,
    #[serde(deserialize_with = "lenient::deserialize")]
    pub deleted: bool,
    pub origin_device: String,
    /// When each field last changed. Absent on rows written before per-field merging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_updated_at: Option<FieldStamps>,
}

impl Folder {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    #[serde(deserialize_with = "lenient::deserialize")]
    pub is_smart: bool,
    /// The JSON rule set; absent unless `is_smart`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smart_rules: Option<String>,
    pub sort_index: String,
    pub updated_at: Millis,
    #[serde(deserialize_with = "lenient::deserialize")]
    pub deleted: bool,
    pub origin_device: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_mime: Option<String>,
}

impl Playlist {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistItem {
    pub id: String,
    pub playlist_id: String,
    pub jellyfin_id: String,
    /// Stored beside the id because Jellyfin ids are derived per file: a rescan after
    /// files move mints new ones, and without this every entry would dangle.
    pub content_key: String,
    /// A `FractionalIndex` key, not an integer.
    pub position: String,
    pub updated_at: Millis,
    #[serde(deserialize_with = "lenient::deserialize")]
    pub deleted: bool,
    pub origin_device: String,
}

impl PlaylistItem {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Like {
    pub id: String,
    pub jellyfin_id: String,
    pub content_key: String,
    #[serde(deserialize_with = "lenient::deserialize")]
    pub liked: bool,
    pub updated_at: Millis,
    #[serde(deserialize_with = "lenient::deserialize")]
    pub deleted: bool,
    pub origin_device: String,
}

impl Like {
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

// MARK: - Taste flags

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
    pub id: String,
    pub jellyfin_id: String,
    pub content_key: String,
    /// Excluded from mixes, stations, smart-playlist results and shuffle.
    #[serde(deserialize_with = "lenient::deserialize")]
    pub not_interested: bool,
    /// New plays are not recorded; existing history is untouched.
    #[serde(deserialize_with = "lenient::deserialize")]
    pub dont_count: bool,
    pub updated_at: Millis,
    #[serde(deserialize_with = "lenient::deserialize")]
    pub deleted: bool,
    pub origin_device: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field_updated_at: Option<FieldStamps>,
}

impl TrackFlags {
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

// MARK: - History

/// Where a play was started from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlaySource {
    Album,
    Playlist,
    Shuffle,
    Search,
    Siri,
    Carplay,
    Smart,
    Unknown,
}

impl PlaySource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Album => "album",
            Self::Playlist => "playlist",
            Self::Shuffle => "shuffle",
            Self::Search => "search",
            Self::Siri => "siri",
            Self::Carplay => "carplay",
            Self::Smart => "smart",
            Self::Unknown => "unknown",
        }
    }
}

/// One listen. Append-only, with the single amendment `finish_play` makes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayEvent {
    pub id: String,
    pub jellyfin_id: String,
    pub content_key: String,
    pub started_at: Millis,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Millis>,
    pub ms_played: i64,
    /// The track reached its end rather than being skipped away from.
    #[serde(deserialize_with = "lenient::deserialize")]
    pub completed: bool,
    #[serde(deserialize_with = "lenient::deserialize")]
    pub skipped: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// What the listener was doing when this began. See [`Activity`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    pub origin_device: String,
}

impl PlayEvent {
    pub fn new(
        jellyfin_id: impl Into<String>,
        content_key: impl Into<String>,
        started_at: Millis,
        origin_device: impl Into<String>,
    ) -> Self {
        Self {
            id: new_id(),
            jellyfin_id: jellyfin_id.into(),
            content_key: content_key.into(),
            started_at,
            ended_at: None,
            ms_played: 0,
            completed: false,
            skipped: false,
            source: Some(PlaySource::Unknown.as_str().to_owned()),
            activity: None,
            origin_device: origin_device.into(),
        }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    pub fn with_source(mut self, source: PlaySource) -> Self {
        self.source = Some(source.as_str().to_owned());
        self
    }

    pub fn with_activity(mut self, activity: Option<Activity>) -> Self {
        self.activity = activity.map(|a| a.as_str().to_owned());
        self
    }

    /// A finished listen: `ms_played`, when it ended, and the two verdicts.
    pub fn finished(
        mut self,
        ended_at: Option<Millis>,
        ms_played: i64,
        completed: bool,
        skipped: bool,
    ) -> Self {
        self.ended_at = ended_at;
        self.ms_played = ms_played;
        self.completed = completed;
        self.skipped = skipped;
        self
    }
}

impl Record for PlayEvent {
    const TABLE: &'static str = "playEvent";
    const KEY: &'static str = "id";
    const COLUMNS: &'static [&'static str] = &[
        "id",
        "jellyfinId",
        "contentKey",
        "startedAt",
        "endedAt",
        "msPlayed",
        "completed",
        "skipped",
        "source",
        "activity",
        "originDevice",
    ];

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            jellyfin_id: row.get("jellyfinId")?,
            content_key: row.get("contentKey")?,
            started_at: row.get("startedAt")?,
            ended_at: row.get("endedAt")?,
            ms_played: row.get("msPlayed")?,
            completed: row.get("completed")?,
            skipped: row.get("skipped")?,
            source: row.get("source")?,
            activity: row.get("activity")?,
            origin_device: row.get("originDevice")?,
        })
    }

    fn values(&self) -> Vec<SqlValue> {
        vec![
            text(&self.id),
            text(&self.jellyfin_id),
            text(&self.content_key),
            int(self.started_at),
            opt_int(self.ended_at),
            int(self.ms_played),
            flag(self.completed),
            flag(self.skipped),
            opt_text(&self.source),
            opt_text(&self.activity),
            text(&self.origin_device),
        ]
    }

    fn key(&self) -> &str {
        &self.id
    }
}

/// One device's queue. Keyed by the device, not by a UUID, and replaced whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueState {
    pub device_id: String,
    pub device_name: String,
    /// A JSON array of Jellyfin ids, as text inside the payload.
    pub track_ids: String,
    pub position: i64,
    pub elapsed_ms: i64,
    pub updated_at: Millis,
    pub origin_device: String,
}

impl QueueState {
    pub fn new<S: AsRef<str>>(
        device_id: impl Into<String>,
        device_name: impl Into<String>,
        track_ids: &[S],
        position: i64,
        elapsed_ms: i64,
        updated_at: Millis,
    ) -> Self {
        let device_id = device_id.into();
        let list: Vec<&str> = track_ids.iter().map(AsRef::as_ref).collect();
        Self {
            origin_device: device_id.clone(),
            device_id,
            device_name: device_name.into(),
            track_ids: serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_owned()),
            position,
            elapsed_ms,
            updated_at,
        }
    }

    pub fn track_id_list(&self) -> Vec<String> {
        serde_json::from_str(&self.track_ids).unwrap_or_default()
    }
}

impl Record for QueueState {
    const TABLE: &'static str = "queueState";
    const KEY: &'static str = "deviceId";
    const COLUMNS: &'static [&'static str] = &[
        "deviceId",
        "deviceName",
        "trackIds",
        "position",
        "elapsedMs",
        "updatedAt",
        "originDevice",
    ];

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            device_id: row.get("deviceId")?,
            device_name: row.get("deviceName")?,
            track_ids: row.get("trackIds")?,
            position: row.get("position")?,
            elapsed_ms: row.get("elapsedMs")?,
            updated_at: row.get("updatedAt")?,
            origin_device: row.get("originDevice")?,
        })
    }

    fn values(&self) -> Vec<SqlValue> {
        vec![
            text(&self.device_id),
            text(&self.device_name),
            text(&self.track_ids),
            int(self.position),
            int(self.elapsed_ms),
            int(self.updated_at),
            text(&self.origin_device),
        ]
    }

    fn key(&self) -> &str {
        &self.device_id
    }
}

// MARK: - The outbound log

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpKind {
    Upsert,
    Delete,
}

impl OpKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Upsert => "upsert",
            Self::Delete => "delete",
        }
    }

    /// Case-sensitive, as the server is: `UPSERT` is not an operation.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "upsert" => Some(Self::Upsert),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }
}

/// One outbound change, written in the same transaction as the row it describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Op {
    pub op_id: String,
    /// The local table name; translated to the server's spelling at the wire.
    pub entity: String,
    pub entity_id: String,
    /// Stored as text, so a row written by anything else can hold anything.
    pub operation: String,
    /// The row as canonical JSON.
    pub payload: String,
    pub created_at: Millis,
    pub synced: bool,
}

impl Op {
    pub fn new(
        entity: impl Into<String>,
        entity_id: impl Into<String>,
        operation: OpKind,
        payload: impl Into<String>,
        created_at: Millis,
    ) -> Self {
        Self {
            op_id: new_id(),
            entity: entity.into(),
            entity_id: entity_id.into(),
            operation: operation.as_str().to_owned(),
            payload: payload.into(),
            created_at,
            synced: false,
        }
    }

    pub fn with_id(mut self, op_id: impl Into<String>) -> Self {
        self.op_id = op_id.into();
        self
    }
}

impl Record for Op {
    const TABLE: &'static str = "op";
    const KEY: &'static str = "opId";
    const COLUMNS: &'static [&'static str] = &[
        "opId",
        "entity",
        "entityId",
        "operation",
        "payload",
        "createdAt",
        "synced",
    ];

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            op_id: row.get("opId")?,
            entity: row.get("entity")?,
            entity_id: row.get("entityId")?,
            operation: row.get("operation")?,
            payload: row.get("payload")?,
            created_at: row.get("createdAt")?,
            synced: row.get("synced")?,
        })
    }

    fn values(&self) -> Vec<SqlValue> {
        vec![
            text(&self.op_id),
            text(&self.entity),
            text(&self.entity_id),
            text(&self.operation),
            text(&self.payload),
            int(self.created_at),
            flag(self.synced),
        ]
    }

    fn key(&self) -> &str {
        &self.op_id
    }
}
