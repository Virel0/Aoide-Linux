//! The sync vocabulary: which tables travel and under what names, what goes over the
//! wire, what can never be sent, and what one sync did.

use serde::de::Deserializer;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

use crate::Millis;
use crate::json;
use crate::models::{Op, OpKind};

// MARK: - Entities

/// The tables that sync, and the two names each of them has.
///
/// The local tables are singular camelCase, the server's allow-list is plural
/// snake_case, and matching on the server is strict and case-sensitive — so these
/// eight strings *are* the contract. `track` is absent on purpose: the track cache is
/// per device and the server refuses it, and with no variant to name it an op for it
/// cannot be turned into a wire op at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SyncEntity {
    /// `playlist` locally, `playlists` on the wire.
    Playlist,
    /// `playlistItem` locally, `playlist_items` on the wire.
    PlaylistItem,
    /// `folder` locally, `folders` on the wire.
    Folder,
    /// `like` locally, `likes` on the wire.
    Like,
    /// `playEvent` locally, `play_events` on the wire. Append-only.
    PlayEvent,
    /// `queueState` locally, `queue_state` on the wire. Keyed by device.
    QueueState,
    /// `trackFlags` locally, `track_flags` on the wire. Held until the server accepts it.
    TrackFlags,
    /// `albumLike` locally, `album_likes` on the wire. Only ever pulled: this app likes
    /// an album through Melete's API, which writes the op, so there is nothing to hold.
    AlbumLike,
}

impl SyncEntity {
    /// Every entity, in declaration order.
    pub const ALL: [SyncEntity; 8] = [
        Self::Playlist,
        Self::PlaylistItem,
        Self::Folder,
        Self::Like,
        Self::PlayEvent,
        Self::QueueState,
        Self::TrackFlags,
        Self::AlbumLike,
    ];

    /// The local table name, which is what the op log stores.
    pub fn local_table(self) -> &'static str {
        match self {
            Self::Playlist => "playlist",
            Self::PlaylistItem => "playlistItem",
            Self::Folder => "folder",
            Self::Like => "like",
            Self::PlayEvent => "playEvent",
            Self::QueueState => "queueState",
            Self::TrackFlags => "trackFlags",
            Self::AlbumLike => "albumLike",
        }
    }

    /// The name in the server's allow-list.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Playlist => "playlists",
            Self::PlaylistItem => "playlist_items",
            Self::Folder => "folders",
            Self::Like => "likes",
            Self::PlayEvent => "play_events",
            Self::QueueState => "queue_state",
            Self::TrackFlags => "track_flags",
            Self::AlbumLike => "album_likes",
        }
    }

    /// Entities a server may not know yet. Their ops are held locally until the
    /// server advertises the entity in `acceptedEntities`, because an unknown entity
    /// is refused and a refused op is dead.
    pub fn needs_server_support(self) -> bool {
        self == Self::TrackFlags
    }

    /// `None` for a table that does not sync, such as `track`.
    pub fn from_local_table(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|entity| entity.local_table() == name)
    }

    /// `None` for a name this build has never heard of; the caller quarantines it.
    pub fn from_wire_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|entity| entity.wire_name() == name)
    }

    /// The payload field that has to equal the op's `entityId`. Queue state is keyed
    /// by the device, so its identity lives in `deviceId`.
    pub fn identity_key(self) -> &'static str {
        if self == Self::QueueState {
            "deviceId"
        } else {
            "id"
        }
    }

    /// Append-only tables carry no `updatedAt` and settle no conflicts.
    pub fn is_append_only(self) -> bool {
        self == Self::PlayEvent
    }
}

// MARK: - Faults

/// Why an op can never be synced. Every case is permanent: nothing that might succeed
/// on the next attempt belongs here.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SyncFault {
    /// The op has no id.
    #[error("op id is empty")]
    EmptyOpId,
    /// The op id is longer than [`SyncPolicy::MAX_OP_ID_LENGTH`] characters.
    #[error("op id is {0} characters; the server accepts {max}", max = SyncPolicy::MAX_OP_ID_LENGTH)]
    OpIdTooLong(usize),
    /// The op names no row.
    #[error("entity id is empty")]
    EmptyEntityId,
    /// The row id is longer than [`SyncPolicy::MAX_ENTITY_ID_LENGTH`] characters.
    #[error("entity id is {0} characters; the server accepts {max}", max = SyncPolicy::MAX_ENTITY_ID_LENGTH)]
    EntityIdTooLong(usize),
    /// The op's `createdAt` is zero or negative.
    #[error("createdAt is {0}; the server requires a positive millisecond timestamp")]
    NonPositiveTimestamp(Millis),
    /// Includes `track`, which has no wire name because it is never synced.
    #[error("'{0}' is not an entity this build can sync")]
    UnknownEntity(String),
    /// An operation other than lowercase `upsert` or `delete`.
    #[error("'{0}' is not an operation; the server accepts lowercase upsert or delete")]
    UnknownOperation(String),
    /// The payload is over [`SyncPolicy::MAX_PAYLOAD_BYTES`].
    #[error("payload is {bytes} bytes; the server accepts {max}", max = SyncPolicy::MAX_PAYLOAD_BYTES)]
    PayloadTooLarge {
        /// The payload's size in bytes.
        bytes: usize,
    },
    /// The payload nests deeper than [`MAX_PAYLOAD_DEPTH`].
    #[error("payload nests {depth} levels deep; the server accepts {max}", max = MAX_PAYLOAD_DEPTH)]
    PayloadTooDeep {
        /// How deep it nests.
        depth: usize,
    },
    /// The payload is not JSON, or is JSON but not an object.
    #[error("payload is not a JSON object")]
    PayloadNotAnObject,
    /// The payload's identity field (`id`, or `deviceId` for queue state) is missing or
    /// names another row than the op does.
    #[error("entity id '{entity_id}' does not match the payload's '{}'", payload_id.as_deref().unwrap_or("<missing>"))]
    IdentityMismatch {
        /// The row the op names.
        entity_id: String,
        /// The row the payload names; `None` when it names none.
        payload_id: Option<String>,
    },
    /// A delete whose payload does not have `deleted` set.
    #[error("a delete op must carry a payload with deleted set")]
    DeleteWithoutFlag,
    /// An inbound payload that does not decode as the local row.
    #[error("payload does not decode into the local row: {0}")]
    UndecodablePayload(String),
    /// An inbound delete of a play event.
    #[error("play events are append-only and cannot be deleted")]
    AppendOnlyDelete,
    /// The server said no. It will say no again, forever.
    #[error("rejected by the server: {0}")]
    ServerRejected(String),
    /// The server *threw* on this op. The one fault in the server rather than in the
    /// op, so the one worth offering to try again.
    #[error("the server failed while storing this change (HTTP {status})")]
    ServerFaulted {
        /// The HTTP status the server answered with.
        status: u16,
    },
}

impl SyncFault {
    /// Stored in the quarantine table: names the value that was wrong, not only the rule.
    pub fn reason(&self) -> String {
        self.to_string()
    }
}

/// How deep a payload may nest before the server refuses it. Checked here first, so a
/// recursive rule set never reaches the server's batch-wide parser ceiling.
pub const MAX_PAYLOAD_DEPTH: usize = 32;

// MARK: - Policy

/// The numbers the engine works to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncPolicy {
    /// Ops per push. Not the server's 1000 ceiling: a batch over it is a 400 for the
    /// whole push, and durability is per batch, so a dropped connection loses a quarter
    /// as much at 250.
    pub push_batch_size: usize,
    /// Ops per pull; the server's own default, clamped there to 1000.
    pub pull_limit: usize,
    /// How far ahead of *both* the server's receipt time and this device's clock a
    /// timestamp may sit before it is read as a broken clock. Both, because one
    /// reference alone cannot tell a fast client from a slow server.
    pub clock_skew_tolerance_ms: Millis,
}

impl SyncPolicy {
    /// The most ops the server takes in one push.
    pub const MAX_BATCH: usize = 1000;
    /// The most ops the server returns in one pull.
    pub const MAX_PULL_LIMIT: usize = 1000;
    /// The largest payload the server accepts, in bytes of compact JSON.
    pub const MAX_PAYLOAD_BYTES: usize = 256 * 1024;
    /// The longest op id the server accepts, in characters.
    pub const MAX_OP_ID_LENGTH: usize = 128;
    /// The longest row id the server accepts, in characters.
    pub const MAX_ENTITY_ID_LENGTH: usize = 256;

    /// Clamped rather than trusted: a batch of 2000 would turn every push into a 400.
    pub fn new(push_batch_size: usize, pull_limit: usize, clock_skew_tolerance_ms: Millis) -> Self {
        Self {
            push_batch_size: push_batch_size.clamp(1, Self::MAX_BATCH),
            pull_limit: pull_limit.clamp(1, Self::MAX_PULL_LIMIT),
            clock_skew_tolerance_ms: clock_skew_tolerance_ms.max(0),
        }
    }

    /// The same policy with another push batch size, clamped to 1 through [`Self::MAX_BATCH`].
    pub fn with_push_batch_size(self, size: usize) -> Self {
        Self::new(size, self.pull_limit, self.clock_skew_tolerance_ms)
    }

    /// The same policy with another pull limit, clamped to 1 through [`Self::MAX_PULL_LIMIT`].
    pub fn with_pull_limit(self, limit: usize) -> Self {
        Self::new(self.push_batch_size, limit, self.clock_skew_tolerance_ms)
    }
}

impl Default for SyncPolicy {
    fn default() -> Self {
        Self::new(250, 500, 5 * 60 * 1000)
    }
}

// MARK: - Reporting

/// What one sync did, in numbers a UI can show without lying.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Ops the server acknowledged, including ones it had already seen.
    pub pushed: usize,
    /// Ours, refused before they left or by the server. Never retried.
    pub quarantined_outbound: usize,
    /// The subset of `quarantined_outbound` the server refused.
    pub rejected: usize,
    /// Ours, refused because the playlist's share was revoked: held back, still in
    /// the log, neither quarantined nor counted in `rejected`.
    pub blocked_by_share: usize,
    /// Ops received, whatever became of them.
    pub pulled: usize,
    /// Ops that changed a local row.
    pub applied: usize,
    /// Ops that lost last-writer-wins, including our own coming back.
    pub ignored_stale: usize,
    /// Play events already present.
    pub ignored_duplicate: usize,
    /// Items applied whose playlist has not arrived yet.
    pub orphaned_items: usize,
    /// Duplicate likes for one track, collapsed to the newer row.
    pub superseded_likes: usize,
    /// Inbound ops this build cannot apply.
    pub quarantined_inbound: usize,
    /// Entries of a pulled page that could not be read as an op at all.
    pub unreadable: usize,
    /// Timestamps pulled back to the server's receipt time.
    pub clock_skewed: usize,
    /// Covers sent this sync.
    pub images_uploaded: usize,
    /// Where the pull got to; `None` when nothing was pulled.
    pub cursor: Option<PullCursor>,
}

impl SyncReport {
    /// Ops refused in either direction.
    pub fn quarantined(&self) -> usize {
        self.quarantined_outbound + self.quarantined_inbound
    }

    /// Whether anything at all moved.
    pub fn is_empty(&self) -> bool {
        self.pushed == 0 && self.pulled == 0 && self.quarantined() == 0
    }

    /// Adds another pass's numbers to these, and takes its cursor if it has one.
    pub fn absorb(&mut self, other: &SyncReport) {
        self.images_uploaded += other.images_uploaded;
        self.pushed += other.pushed;
        self.quarantined_outbound += other.quarantined_outbound;
        self.rejected += other.rejected;
        self.blocked_by_share += other.blocked_by_share;
        self.pulled += other.pulled;
        self.applied += other.applied;
        self.ignored_stale += other.ignored_stale;
        self.ignored_duplicate += other.ignored_duplicate;
        self.orphaned_items += other.orphaned_items;
        self.superseded_likes += other.superseded_likes;
        self.quarantined_inbound += other.quarantined_inbound;
        self.unreadable += other.unreadable;
        self.clock_skewed += other.clock_skewed;
        if other.cursor.is_some() {
            self.cursor = other.cursor;
        }
    }
}

/// Where a sync has got to, reported as it happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncPhase {
    /// No sync under way.
    Idle,
    /// Pushing ops.
    Pushing {
        /// Ops acknowledged so far.
        pushed: usize,
    },
    /// Pulling ops.
    Pulling {
        /// Ops applied so far.
        applied: usize,
    },
    /// The sync stopped, for a reason the listener can read.
    Failed {
        /// What went wrong.
        reason: String,
    },
}

// MARK: - Cursors

/// How far this device has *read* the server's log. The one that gets persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PullCursor(pub i64);

/// The server's head sequence, handed back by a push. Informational.
///
/// A separate type from `PullCursor` so storing it as the read position does not
/// compile: ops this user's other devices pushed can sit below it, and past that head
/// they are never offered again. Nothing in this crate persists one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PushCursor(pub i64);

impl std::fmt::Display for PullCursor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

// MARK: - Push

/// One op as the server wants it: wire entity name, lowercase operation, payload as a
/// JSON object rather than the text it is stored as. The payload is never reshaped —
/// a key the row spells `Notes` leaves as `Notes`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireOp {
    /// The op's id, unique across devices.
    pub op_id: String,
    /// The wire entity name, from [`SyncEntity::wire_name`].
    pub entity: String,
    /// The id of the row it describes.
    pub entity_id: String,
    /// `upsert` or `delete`.
    pub operation: String,
    /// The row as a JSON object.
    pub payload: Value,
    /// When the op was written, in milliseconds since the epoch on the writing device.
    pub created_at: Millis,
}

impl WireOp {
    /// Converts a stored op for the wire, or says why it can never be sent.
    ///
    /// Every rule the server enforces is checked here first, so an op certain to bounce
    /// is quarantined with a reason instead of sitting at the head of the queue. The
    /// two rules the server *cannot* check — `entityId` agreeing with the payload, and
    /// a delete carrying its flag — are the ones worth having here at all.
    ///
    /// # Errors
    ///
    /// The first rule the op breaks, as a [`SyncFault`]; every one of them is permanent.
    pub fn make(op: &Op) -> Result<WireOp, SyncFault> {
        if op.op_id.is_empty() {
            return Err(SyncFault::EmptyOpId);
        }
        let op_id_length = op.op_id.chars().count();
        if op_id_length > SyncPolicy::MAX_OP_ID_LENGTH {
            return Err(SyncFault::OpIdTooLong(op_id_length));
        }
        if op.entity_id.is_empty() {
            return Err(SyncFault::EmptyEntityId);
        }
        let entity_id_length = op.entity_id.chars().count();
        if entity_id_length > SyncPolicy::MAX_ENTITY_ID_LENGTH {
            return Err(SyncFault::EntityIdTooLong(entity_id_length));
        }
        if op.created_at <= 0 {
            return Err(SyncFault::NonPositiveTimestamp(op.created_at));
        }
        let entity = SyncEntity::from_local_table(&op.entity)
            .ok_or_else(|| SyncFault::UnknownEntity(op.entity.clone()))?;
        let operation = OpKind::parse(&op.operation)
            .ok_or_else(|| SyncFault::UnknownOperation(op.operation.clone()))?;

        // Measured on the stored text, which is the same compact JSON that goes out.
        let bytes = op.payload.len();
        if bytes > SyncPolicy::MAX_PAYLOAD_BYTES {
            return Err(SyncFault::PayloadTooLarge { bytes });
        }

        let parsed: Value =
            serde_json::from_str(&op.payload).map_err(|_| SyncFault::PayloadNotAnObject)?;
        let Some(fields) = parsed.as_object() else {
            return Err(SyncFault::PayloadNotAnObject);
        };
        let depth = json::depth(&parsed);
        if depth > MAX_PAYLOAD_DEPTH {
            return Err(SyncFault::PayloadTooDeep { depth });
        }

        let Some(identity) = fields.get(entity.identity_key()).and_then(Value::as_str) else {
            return Err(SyncFault::IdentityMismatch {
                entity_id: op.entity_id.clone(),
                payload_id: None,
            });
        };
        if identity != op.entity_id {
            return Err(SyncFault::IdentityMismatch {
                entity_id: op.entity_id.clone(),
                payload_id: Some(identity.to_owned()),
            });
        }

        // A delete whose payload does not say so travels as alive and resurrects
        // itself everywhere. The converse is left alone; the merge reads the flag.
        if operation == OpKind::Delete && !json::is_flag_set(fields.get("deleted")) {
            return Err(SyncFault::DeleteWithoutFlag);
        }

        Ok(WireOp {
            op_id: op.op_id.clone(),
            entity: entity.wire_name().to_owned(),
            entity_id: op.entity_id.clone(),
            operation: operation.as_str().to_owned(),
            payload: parsed,
            created_at: op.created_at,
        })
    }
}

/// One push: this device's ops, in the order they were written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushRequest {
    /// The pushing device.
    pub device_id: String,
    /// The ops, at most [`SyncPolicy::MAX_BATCH`].
    pub ops: Vec<WireOp>,
}

/// One op the server refused. It will refuse it again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RejectedOp {
    /// The refused op's id.
    pub op_id: String,
    /// The server's reason, or "no reason given".
    pub reason: String,
}

impl<'de> Deserialize<'de> for RejectedOp {
    /// `{"opId": …, "reason": …}`, or a bare id. The field is prose in the contract,
    /// and failing the whole response over its shape would throw away the `accepted`
    /// list beside it.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Shape {
            Bare(String),
            Full {
                #[serde(rename = "opId")]
                op_id: String,
                #[serde(default)]
                reason: Option<String>,
            },
        }
        match Shape::deserialize(deserializer)? {
            Shape::Bare(op_id) => Ok(Self {
                op_id,
                reason: "no reason given".to_owned(),
            }),
            Shape::Full { op_id, reason } => Ok(Self {
                op_id,
                reason: reason.unwrap_or_else(|| "no reason given".to_owned()),
            }),
        }
    }
}

/// What a push answered. Every field tolerates absence: a reply missing one must not
/// throw away the `accepted` list next to it, or that batch is pushed again forever.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushResponse {
    /// Includes ops the server had already seen, which is what makes retrying a
    /// timed-out push safe.
    #[serde(default)]
    pub accepted: Vec<String>,
    /// Ops the server refused, with its reasons.
    #[serde(default)]
    pub rejected: Vec<RejectedOp>,
    /// The server's head sequence. Not a pull cursor.
    #[serde(default)]
    pub cursor: Option<PushCursor>,
}

// MARK: - Pull

/// An op coming back down, with the fields the server adds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PulledOp {
    /// The op's id.
    pub op_id: String,
    /// The wire entity name, which may be one this build does not know.
    pub entity: String,
    /// The id of the row it describes.
    pub entity_id: String,
    /// The operation as sent; checked when applied.
    pub operation: String,
    /// The row as a JSON object.
    pub payload: Value,
    /// The writing device's clock; sanity-checked against `received_at`.
    pub created_at: Millis,
    /// Commit-ordered; gaps are normal.
    pub seq: i64,
    /// Which device pushed it. Recorded, never used to decide whether to apply.
    pub device_id: String,
    /// Server receipt time: the only clock every device shares.
    pub received_at: Millis,
    /// Who wrote it. Differs from this user only on a shared playlist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_user_id: Option<String>,
}

/// One page of the log.
#[derive(Debug, Clone, PartialEq)]
pub struct PullResponse {
    /// Ascending by `seq`.
    pub ops: Vec<PulledOp>,
    /// Where the next page starts.
    pub cursor: PullCursor,
    /// Exact: the server reads one row past the limit to answer it.
    pub has_more: bool,
    /// Entries this build could not read as an op at all — counted, not fatal.
    pub unreadable_ops: usize,
}

impl<'de> Deserialize<'de> for PullResponse {
    /// One unreadable op must not cost the page, because it would cost every future
    /// page: the cursor only advances when a page is applied, so a page that fails to
    /// decode is asked for again, fails again, and inbound sync stops for good. Each
    /// element is decoded on its own; one that fails has no `opId` to quarantine under,
    /// so it is counted and the cursor moves on.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            ops: Vec<Value>,
            cursor: PullCursor,
            #[serde(default)]
            has_more: bool,
        }
        let raw = Raw::deserialize(deserializer)?;
        let total = raw.ops.len();
        let ops: Vec<PulledOp> = raw
            .ops
            .into_iter()
            .filter_map(|entry| serde_json::from_value(entry).ok())
            .collect();
        let unreadable_ops = total - ops.len();
        Ok(Self {
            ops,
            cursor: raw.cursor,
            has_more: raw.has_more,
            unreadable_ops,
        })
    }
}

impl Serialize for PullResponse {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Out<'a> {
            ops: &'a [PulledOp],
            cursor: PullCursor,
            has_more: bool,
        }
        Out {
            ops: &self.ops,
            cursor: self.cursor,
            has_more: self.has_more,
        }
        .serialize(serializer)
    }
}
