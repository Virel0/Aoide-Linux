//! The local store: play history, playlists, taste flags and the sync op log, in SQLite.
//!
//! A port of the iPhone app's CurationKit store. Every read and write hits SQLite,
//! never the network: offline is not a degraded mode here, it is the normal one that
//! sometimes also syncs. Every syncable change is written in the same transaction as
//! the op that describes it ([`Store::record`]), and pulled ops are merged by the rules
//! in [`Store::apply`].
//!
//! Payloads are rows encoded as JSON with the phone's field names (`camelCase`, keys
//! sorted, an empty optional left out rather than `null`), so a row written here reads
//! the same on the phone and back.

mod account;
mod activity;
mod built_in;
mod covers;
mod error;
mod field_stamps;
mod finish_rate;
mod fractional_index;
mod handoff;
mod history;
mod images;
mod json;
mod merge;
mod models;
mod play_definition;
mod playlists;
mod quarantine;
mod queue;
mod recent_contexts;
mod record;
mod schema;
mod smart;
mod store;
mod sync;
mod taste;
mod track_cache;
mod track_flags;

#[cfg(test)]
mod tests;

pub use account::AccountClaim;
pub use activity::Activity;
pub use built_in::BuiltInPlaylist;
pub use covers::{
    BlobDirectory, CoverError, CoverRepair, MAX_IMAGE_BYTES, NamedJellyfinPlaylist, PlaylistCover,
    is_coverless, is_image_hash, plan_cover_repairs, sniff_image_mime,
};
pub use error::{Result, StoreError};
pub use field_stamps::{FieldStamps, METADATA_COLUMNS};
pub use finish_rate::{FinishCounts, FinishRate};
pub use fractional_index::FractionalIndex;
pub use handoff::{PickUp, QueueBroadcast, QueueFreshness, QueueSnapshot, ResumableQueue};
pub use history::{
    AlbumPlays, ArtistPlays, DayPlays, PlayWeighting, Recap, TrackPlays, TrackStats,
};
pub use images::{IMAGE_GRACE_DAYS, ImageBlob};
pub use json::canonical_json;
pub use models::{
    ContentKey, Folder, Like, Op, OpKind, PlayEvent, PlaySource, Playlist, PlaylistItem,
    QueueState, TrackFlags, TrackRef,
};
pub use play_definition::{PlayClassifier, PlayOutcome};
pub use playlists::{PlaylistError, RelinkReport, TrackLink, UNRESOLVED_PREFIX};
pub use quarantine::{QuarantinedOp, SyncDirection};
pub use queue::QueuePolicy;
pub use recent_contexts::{
    RESUME_GRID_LIMIT, RecentContext, RecentContextKind, RecentContextList, RecentContexts,
    ResumeTile, StationSeed, resume_tiles,
};
pub use record::{Record, SyncRecord};
pub use smart::{
    Calendar, Field, Match, Operator, RelativeInterval, SmartPlaylistError, SmartRule, SmartRules,
    Sort, SortDirection, TimeUnit, Value,
};
pub use store::Store;
pub use sync::{
    MAX_PAYLOAD_DEPTH, PullCursor, PullResponse, PulledOp, PushCursor, PushRequest, PushResponse,
    RejectedOp, SyncEntity, SyncFault, SyncPhase, SyncPolicy, SyncReport, WireOp,
};
pub use taste::{TasteCandidate, TasteProfile, TasteRanking};
pub use track_flags::Flag;

/// Re-exported so callers of [`Store::read`] and [`Store::write_local`] name the same
/// version of the crate the store was built against.
pub use rusqlite;

/// Milliseconds since the epoch — the timestamp every syncable row carries.
pub type Millis = i64;

/// The current time in milliseconds since the epoch.
pub fn now_millis() -> Millis {
    chrono::Utc::now().timestamp_millis()
}

/// A fresh row or op id: an upper-case hyphenated UUID, the spelling the phone mints.
pub fn new_id() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string().to_uppercase()
}
