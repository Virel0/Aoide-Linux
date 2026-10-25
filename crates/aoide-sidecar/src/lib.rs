//! The Aoide sidecar client: sync push and pull, /aoide/next, /aoide/discover, and the measurements.
//!
//! The sidecar is a Jellyfin plugin on the same origin as Jellyfin, authenticated with
//! the same token. [`SidecarClient`] speaks its HTTP contract; [`SyncEngine`] joins it
//! to the local [`aoide_store::Store`]: covers first, then push, then pull, with the
//! store's merge rules applied to every pulled page.

mod client;
mod covers;
mod engine;
mod error;
mod handoff;
mod types;

#[cfg(test)]
mod tests;

pub use client::{MAX_IDS_PER_CALL, SidecarClient};
pub use covers::{CoverFetchError, cover_bytes};
pub use engine::{
    BlockedEdit, ImageByteSource, REVOKED_SHARE_CLAUSE, SHARE_RECHECK_MS, SyncEngine, SyncError,
    SyncTransport, is_share_revoked,
};
pub use error::SidecarError;
pub use handoff::{QueueSource, queues_from_server, resumable_queues};
pub use types::{
    AnalysisCoverage, AudioAnalysis, AudioAnalysisResponse, DiscoverCandidate, DiscoverFactors,
    DiscoverResponse, Measurement, NextCandidate, NextFactors, NextMode, NextRequest, NextResponse,
    PlaylistShare, ProfileSummary, QueueEntry, ShareRequest, SoundBounds, SoundBoundsResponse,
    SyncStatus, iso_week,
};
