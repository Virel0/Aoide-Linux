//! Play events and where they were started from.

use rusqlite::Row;
use rusqlite::types::Value as SqlValue;
use serde::{Deserialize, Serialize};

use crate::Millis;
use crate::activity::Activity;
use crate::json::flag as lenient;
use crate::new_id;
use crate::record::{Record, flag, int, opt_int, opt_text, text};

/// Where a play was started from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlaySource {
    /// An album, played through.
    Album,
    /// A playlist.
    Playlist,
    /// Shuffle across the library.
    Shuffle,
    /// A search result.
    Search,
    /// A voice request on the phone.
    Siri,
    /// The phone's car interface.
    Carplay,
    /// A smart playlist or generated mix.
    Smart,
    /// Not recorded, or a spelling this build does not know.
    Unknown,
}

impl PlaySource {
    /// The spelling stored in `PlayEvent::source`.
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
    /// The row id.
    pub id: String,
    /// The track's Jellyfin id when it was played.
    pub jellyfin_id: String,
    /// The track's [`ContentKey`](crate::ContentKey), which still finds it after a rescan.
    pub content_key: String,
    /// When the listen began, in milliseconds since the epoch.
    pub started_at: Millis,
    /// When it ended; `None` while it is under way, or if the app stopped first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Millis>,
    /// How long the track was heard, in milliseconds.
    pub ms_played: i64,
    /// The track reached its end rather than being skipped away from.
    #[serde(deserialize_with = "lenient::deserialize")]
    pub completed: bool,
    /// The listener skipped away before the end.
    #[serde(deserialize_with = "lenient::deserialize")]
    pub skipped: bool,
    /// A [`PlaySource`] spelling; kept as text so an unknown one survives a round trip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// What the listener was doing when this began. See [`Activity`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    /// The device it was heard on.
    pub origin_device: String,
}

impl PlayEvent {
    /// A listen that has just begun, from an unknown source, with a fresh id.
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

    /// The same listen under `id`.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// The same listen, started from `source`.
    pub fn with_source(mut self, source: PlaySource) -> Self {
        self.source = Some(source.as_str().to_owned());
        self
    }

    /// The same listen, during `activity` (or none).
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
