//! One device's saved queue.

use rusqlite::Row;
use rusqlite::types::Value as SqlValue;
use serde::{Deserialize, Serialize};

use crate::Millis;
use crate::record::{Record, int, text};

/// One device's queue. Keyed by the device, not by a UUID, and replaced whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueState {
    /// The device whose queue this is; the key.
    pub device_id: String,
    /// That device's name, for "continue from …".
    pub device_name: String,
    /// A JSON array of track ids, as text inside the payload.
    pub track_ids: String,
    /// The index into `track_ids` of the track playing.
    pub position: i64,
    /// How far into that track, in milliseconds.
    pub elapsed_ms: i64,
    /// When the queue last changed, in milliseconds since the epoch.
    pub updated_at: Millis,
    /// The device that wrote it, which is always `device_id`.
    pub origin_device: String,
}

impl QueueState {
    /// A queue for `device_id`, written by that device.
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

    /// The track ids, read back from `track_ids`; empty if it does not parse.
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
