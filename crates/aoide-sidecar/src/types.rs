//! What the `/aoide/*` endpoints send and answer, field for field as the client
//! contract documents them. Every response field is `camelCase` on the wire.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};

// MARK: - Sync status

/// `GET /aoide/sync/status`: diagnostics, and since 1.9.0.0 the exact list of entities
/// push will accept. Every field optional, so an older server's answer still reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    /// Where Melete's database is, on the server's disk.
    #[serde(default)]
    pub database_path: Option<String>,
    /// The version of the database's schema.
    #[serde(default)]
    pub schema_version: Option<i64>,
    /// `false` with `error` set means pushes will fail while pulls keep working.
    #[serde(default)]
    pub writable: Option<bool>,
    /// Should read `wal`.
    #[serde(default)]
    pub journal_mode: Option<String>,
    /// Whether the database's directory is writable, which SQLite's journal
    /// files need as much as the database file itself.
    #[serde(default)]
    pub directory_writable: Option<bool>,
    /// The newest op's sequence number: the cursor a pull that has caught up
    /// ends at.
    #[serde(default)]
    pub cursor: Option<i64>,
    /// How many ops the server's log holds.
    #[serde(default)]
    pub op_count: Option<i64>,
    /// The server's version: `melete/0.1.0`, or the Jellyfin plugin's `1.9.0.0`.
    #[serde(default)]
    pub plugin_version: Option<String>,
    /// Absent on servers before 1.9.0.0 — which then accept only the original six.
    #[serde(default)]
    pub accepted_entities: Option<Vec<String>>,
    /// What is wrong with the database, when something is; see `writable`.
    #[serde(default)]
    pub error: Option<String>,
}

// MARK: - What plays next

/// Which kind of queue `POST /aoide/next` is filling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NextMode {
    /// A queue that keeps going: ranked by taste, freshness, similarity to the
    /// seed and arc.
    Infinity,
    /// Auto DJ: the same, plus how well each candidate mixes out of the seed
    /// ([`NextFactors::mixability`]).
    Autodj,
}

/// `POST /aoide/next`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NextRequest {
    /// The record playing.
    pub seed: String,
    /// What is queued after the seed, in order. At most 1000.
    pub queue: Vec<String>,
    /// What this device heard lately, newest first. At most 40.
    pub recent: Vec<String>,
    /// What the queue is for; Auto DJ adds mixability to the score.
    pub mode: NextMode,
    /// How many candidates to return, best first. 1 to 200.
    pub limit: u32,
}

impl NextRequest {
    /// The most `queue` ids the contract takes.
    pub const MAX_QUEUE: usize = 1000;
    /// The most `recent` ids the contract takes.
    pub const MAX_RECENT: usize = 40;
    /// The largest `limit` the contract takes.
    pub const MAX_LIMIT: u32 = 200;

    /// Anything past the contract's ceilings is dropped from the far end — the oldest
    /// of the recent, the furthest of the queue — rather than refused, so a long queue
    /// never stops the music. `limit` is held to 1…200 for the same reason.
    pub fn new(
        seed: impl Into<String>,
        queue: &[String],
        recent: &[String],
        mode: NextMode,
        limit: u32,
    ) -> Self {
        Self {
            seed: seed.into(),
            queue: queue.iter().take(Self::MAX_QUEUE).cloned().collect(),
            recent: recent.iter().take(Self::MAX_RECENT).cloned().collect(),
            mode,
            limit: limit.clamp(1, Self::MAX_LIMIT),
        }
    }
}

/// How much history the taste term stood on. With no finished plays, taste is 0 for
/// everything — "still learning what you like".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSummary {
    /// How many finished plays the taste profile was built from (the latest 200
    /// within 90 days, from every device). 0 means no taste yet.
    pub events: i64,
    /// Milliseconds since the Unix epoch: where the counted history begins.
    pub since: i64,
}

/// Why a record was chosen, each in 0…1.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NextFactors {
    /// How well the record fits what this user finishes: genre, artist, and
    /// their own finish rate. Reported clamped to 0…1.
    pub taste: f64,
    /// Whether the record is of the seed's kind. Absent before 1.17.0.0.
    #[serde(default)]
    pub kinship: Option<f64>,
    /// 1 when not heard lately, 0.5 when its artist was, 0 when it was itself.
    pub freshness: f64,
    /// Closeness to the seed in genre, tempo, key and energy: the mean of the
    /// parts both sides have data for.
    pub similarity: f64,
    /// `null` outside Auto DJ and for a pair either side of which is unread — absent,
    /// not 0.
    #[serde(default)]
    pub mixability: Option<f64>,
    /// How well the record's energy follows the set's direction; 0.5 when it
    /// has no arrangement to read.
    pub arc: f64,
}

impl NextFactors {
    /// What the server reports for a pair the planner refused: it can still be
    /// crossfaded. Anything at or under it is not a mix.
    pub const CROSSFADE_ONLY: f64 = 0.15;

    /// One line for a queue row: the figures as percentages, and a word when freshness
    /// cost something. Freshness is left out at 1, where nearly every row sits.
    pub fn summary(&self) -> String {
        let pct = |value: f64| format!("{}%", (value * 100.0).round() as i64);
        let mut parts = vec![format!("Taste {}", pct(self.taste))];
        if let Some(kinship) = self.kinship {
            parts.push(format!("Kin {}", pct(kinship)));
        }
        parts.push(format!("Fits {}", pct(self.similarity)));
        if let Some(mixability) = self.mixability {
            parts.push(if mixability <= Self::CROSSFADE_ONLY {
                "Crossfade".to_owned()
            } else {
                format!("Mixes {}", pct(mixability))
            });
        }
        parts.push(format!("Arc {}", pct(self.arc)));
        if self.freshness < 0.25 {
            parts.push("heard lately".to_owned());
        } else if self.freshness < 0.75 {
            parts.push("same artist lately".to_owned());
        }
        parts.join(" · ")
    }
}

/// One record `POST /aoide/next` proposes.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NextCandidate {
    /// The track's Melete id.
    pub id: String,
    /// The weighted sum the candidates are ranked by. Not bounded to 0…1: the
    /// freshness penalties take it below zero.
    pub score: f64,
    /// The parts of the score, for showing why it was picked.
    pub factors: NextFactors,
}

/// `POST /aoide/next`'s answer.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NextResponse {
    /// In the order they should play.
    pub candidates: Vec<NextCandidate>,
    /// How much history the taste term stood on.
    pub profile: ProfileSummary,
}

// MARK: - Discover

/// Only `taste` is measured. `kinship`, `similarity`, `mixability` and `arc` are `null`
/// because there is no seed, and `freshness` is 1 because nothing in the pool has been
/// heard. The nulls are kept as nulls rather than read as zeros.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverFactors {
    /// As [`NextFactors::taste`]: the one factor Discover ranks by.
    pub taste: f64,
    /// `null` from the server: there is no seed.
    #[serde(default)]
    pub kinship: Option<f64>,
    /// 1 when sent: nothing in the pool has been heard.
    #[serde(default)]
    pub freshness: Option<f64>,
    /// `null` from the server: there is no seed.
    #[serde(default)]
    pub similarity: Option<f64>,
    /// `null` from the server: there is no seed.
    #[serde(default)]
    pub mixability: Option<f64>,
    /// `null` from the server: there is no seed.
    #[serde(default)]
    pub arc: Option<f64>,
}

/// One record of the week's Discover list.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverCandidate {
    /// The track's Melete id.
    pub id: String,
    /// What the list is ranked by: taste alone.
    pub score: f64,
    /// The parts of the score.
    pub factors: DiscoverFactors,
}

/// `GET /aoide/discover`: the same all week, on every device.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverResponse {
    /// The ISO week the list belongs to, in UTC: `2026-W43`. Refresh when it changes.
    pub week: String,
    /// Tracks this user has never played, best first.
    pub candidates: Vec<DiscoverCandidate>,
    /// How much history the taste term stood on.
    pub profile: ProfileSummary,
}

impl DiscoverResponse {
    /// How many candidates the clients ask for.
    pub const DEFAULT_LIMIT: u32 = 30;
    /// The largest `limit` the server takes; more is a 400.
    pub const MAX_LIMIT: u32 = 100;
}

/// The ISO 8601 week of an instant in UTC, spelled as the server spells it. The list
/// turns at Monday 00:00 UTC on every device at once.
pub fn iso_week(millis: i64) -> String {
    let instant = DateTime::<Utc>::from_timestamp_millis(millis).unwrap_or_default();
    let week = instant.iso_week();
    format!("{:04}-W{:02}", week.year(), week.week())
}

// MARK: - Measurements

/// What the server knows about one track, in the three states the contract
/// distinguishes plus the one it leaves implicit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Measurement<T> {
    /// Measured, with something to report.
    Measured(T),
    /// Measured, with nothing to report (`null`): play the file whole, use no tempo.
    NothingToReport,
    /// Queued: play it as it is this time and ask again next time.
    Pending,
    /// Missing from both: unknown, not visible, or no file to measure.
    Unknown,
}

fn lookup<'a, T>(
    measured: &'a BTreeMap<String, Option<T>>,
    pending: &[String],
    id: &str,
) -> Measurement<&'a T> {
    match measured.get(id) {
        Some(Some(value)) => Measurement::Measured(value),
        Some(None) => Measurement::NothingToReport,
        None if pending.iter().any(|queued| queued == id) => Measurement::Pending,
        None => Measurement::Unknown,
    }
}

/// Where a track's sound starts and stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundBounds {
    /// Where the first sound is, less 60 ms of margin, from the start of the
    /// file; never below 0.
    pub sound_start_ms: i64,
    /// Where the last sound is, plus 200 ms of margin, from the start of the
    /// file; never past its end.
    pub sound_end_ms: i64,
}

/// `GET /aoide/sound-bounds`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundBoundsResponse {
    /// Measured tracks by track id; `None` for one measured with nothing to
    /// trim.
    #[serde(default)]
    pub bounds: BTreeMap<String, Option<SoundBounds>>,
    /// Ids queued for measuring, not yet done.
    #[serde(default)]
    pub pending: Vec<String>,
}

impl SoundBoundsResponse {
    /// What the answer says about track `id`, in the four states of
    /// [`Measurement`].
    pub fn get(&self, id: &str) -> Measurement<&SoundBounds> {
        lookup(&self.bounds, &self.pending, id)
    }

    pub(crate) fn absorb(&mut self, other: SoundBoundsResponse) {
        self.bounds.extend(other.bounds);
        self.pending.extend(other.pending);
    }
}

/// Loudness and tempo. Every field is independently nullable: a track can have a
/// loudness and no usable tempo, which is the common case for ambient.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioAnalysis {
    /// EBU R128 integrated loudness. Normalise to −18 LUFS, attenuating only.
    #[serde(default)]
    pub loudness_lufs: Option<f64>,
    /// Above zero is not an error: a track clipped by its own master.
    #[serde(default)]
    pub true_peak_dbfs: Option<f64>,
    /// Not sent below 0.5 confidence.
    #[serde(default)]
    pub bpm: Option<f64>,
    /// How sure the tempo detector is of `bpm`, 0…1.
    #[serde(default)]
    pub bpm_confidence: Option<f64>,
    /// How much of the track keeps that tempo. Absent before 1.12.0.0.
    #[serde(default)]
    pub bpm_stability: Option<f64>,
}

/// `GET /aoide/audio-analysis`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioAnalysisResponse {
    /// Measured tracks by track id; `None` for one measured with nothing to
    /// report.
    #[serde(default)]
    pub analysis: BTreeMap<String, Option<AudioAnalysis>>,
    /// Ids queued for measuring, not yet done.
    #[serde(default)]
    pub pending: Vec<String>,
}

impl AudioAnalysisResponse {
    /// What the answer says about track `id`, in the four states of
    /// [`Measurement`].
    pub fn get(&self, id: &str) -> Measurement<&AudioAnalysis> {
        lookup(&self.analysis, &self.pending, id)
    }

    pub(crate) fn absorb(&mut self, other: AudioAnalysisResponse) {
        self.analysis.extend(other.analysis);
        self.pending.extend(other.pending);
    }
}

/// `GET /aoide/analysis/coverage`: how far the lazy measurement has got. The counts are
/// global, not per user. Fields added after 1.14.0.0 are optional.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisCoverage {
    /// Audio tracks on the server.
    pub tracks: i64,
    /// Of those, how many have sound bounds measured.
    pub sound_bounds: i64,
    /// Of those, how many have loudness and tempo measured.
    pub audio_analysis: i64,
    /// Decoded for a grid — a progress bar wants this against `tracks`.
    #[serde(default)]
    pub beat_grids: Option<i64>,
    /// Of those, how many came out with a grid. Not a backlog.
    #[serde(default)]
    pub gridded: Option<i64>,
    /// How many have an arrangement (sections and their energy) read.
    #[serde(default)]
    pub arrangements: Option<i64>,
    /// Of `tracks`, how many the server has described. Absent from older
    /// servers.
    #[serde(default)]
    pub described: Option<i64>,
    /// Queued or decoding right now. Zero with work outstanding means nothing is
    /// asking, not that anything is stuck.
    pub measuring: i64,
    /// Whether the server measures the whole library in the background, rather
    /// than only what clients ask for.
    #[serde(default)]
    pub sweep_enabled: Option<bool>,
    /// The same in a sentence, for a person.
    #[serde(default)]
    pub summary: Option<String>,
}

// MARK: - Handoff

/// One device's row from `GET /aoide/queue`, read leniently: the payload is the
/// writing client's own row, verbatim, so `trackIds` may be an array or the JSON text
/// the phone stores, and an older desktop build put the fields beside the payload
/// rather than inside it. Freshness is the server's clock — `age_seconds`,
/// `received_at` — never `updated_at`, which a device with a wrong clock would win
/// every handover with.
#[derive(Debug, Clone, PartialEq)]
pub struct QueueEntry {
    /// The writing device's id, as its sync ops carry it.
    pub device_id: String,
    /// This device's own row.
    pub is_current_device: bool,
    /// Seconds since the server received the row, on the server's clock.
    pub age_seconds: Option<f64>,
    /// When the server received it, on the server's clock.
    pub received_at: Option<i64>,
    /// The **writing** device's clock. Not for judging freshness.
    pub updated_at: Option<i64>,
    /// The name the device gave itself, shown as "Pick up from …".
    pub device_name: Option<String>,
    /// The queue's tracks as track ids, in play order.
    pub track_ids: Vec<String>,
    /// The 0-based index into `track_ids` of the track playing.
    pub position: i64,
    /// How far into that track, in milliseconds.
    pub elapsed_ms: i64,
}

impl QueueEntry {
    /// `None` for an entry with no device id; everything else has a default.
    pub fn from_json(entry: &serde_json::Value) -> Option<Self> {
        let object = entry.as_object()?;
        let payload = object.get("payload").and_then(serde_json::Value::as_object);
        // Inside the payload first, then beside it.
        let field = |key: &str| {
            payload
                .and_then(|payload| payload.get(key))
                .filter(|value| !value.is_null())
                .or_else(|| object.get(key).filter(|value| !value.is_null()))
        };
        let device_id = object
            .get("deviceId")
            .and_then(serde_json::Value::as_str)
            .or_else(|| field("deviceId").and_then(serde_json::Value::as_str))?
            .to_owned();
        let track_ids = match field("trackIds") {
            Some(serde_json::Value::Array(items)) => items
                .iter()
                .filter_map(|id| id.as_str().map(str::to_owned))
                .collect(),
            Some(serde_json::Value::String(text)) => {
                serde_json::from_str::<Vec<String>>(text).unwrap_or_default()
            }
            _ => Vec::new(),
        };
        let integer = |key: &str| {
            field(key)
                .and_then(serde_json::Value::as_f64)
                .map(|v| v as i64)
        };
        Some(Self {
            device_id,
            is_current_device: object
                .get("isCurrentDevice")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            age_seconds: object
                .get("ageSeconds")
                .and_then(serde_json::Value::as_f64)
                .filter(|age| age.is_finite()),
            received_at: object.get("receivedAt").and_then(serde_json::Value::as_i64),
            updated_at: object
                .get("updatedAt")
                .and_then(serde_json::Value::as_i64)
                .or_else(|| integer("updatedAt")),
            device_name: field("deviceName")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            track_ids,
            position: integer("position").unwrap_or(0),
            elapsed_ms: integer("elapsedMs").unwrap_or(0),
        })
    }
}

/// A list endpoint's entries, as a bare array or wrapped under one of `keys`.
pub(crate) fn list_from<'a>(body: &'a serde_json::Value, keys: &[&str]) -> &'a [serde_json::Value] {
    if let Some(items) = body.as_array() {
        return items;
    }
    keys.iter()
        .find_map(|key| body.get(*key).and_then(serde_json::Value::as_array))
        .map_or(&[], Vec::as_slice)
}

// MARK: - Collaborative playlists

/// One grant from `GET /aoide/shares`: shares owned by the caller and shares granted
/// to the caller, in one list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistShare {
    /// The Aoide playlist's id, as its sync ops carry it.
    pub playlist_id: String,
    /// The Melete user id the playlist is shared with.
    pub grantee_user_id: String,
    /// The grantee may edit, not only listen.
    #[serde(default)]
    pub can_edit: bool,
    /// Absent when the server does not name an owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_user_id: Option<String>,
}

impl PlaylistShare {
    /// Whether this grant is evidence that `user_id` may **edit** the playlist: an
    /// editable share that names them, or that they own. Without a `user_id` only an
    /// ownerless share counts — claiming somebody else's access would mean a refused
    /// push every sync, where doubting it merely keeps an edit visible as held back.
    pub fn grants_edit_to(&self, user_id: Option<&str>) -> bool {
        if !self.can_edit {
            return false;
        }
        match (self.owner_user_id.as_deref(), user_id) {
            (None, _) => true,
            (Some(owner), Some(user)) => owner == user || self.grantee_user_id == user,
            (Some(_), None) => false,
        }
    }
}

/// `POST /aoide/shares`. Only the owner can share.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareRequest {
    /// The Aoide playlist to share.
    pub playlist_id: String,
    /// The Melete user id to share it with.
    pub grantee_user_id: String,
    /// Whether they may edit it, or only listen.
    pub can_edit: bool,
}
