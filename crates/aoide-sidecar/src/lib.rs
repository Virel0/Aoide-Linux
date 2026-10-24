//! The Aoide sidecar client: sync push and pull, /aoide/next, /aoide/discover, and the measurements.
//!
//! The sidecar is a Jellyfin plugin on the same origin as Jellyfin, authenticated with
//! the same token. [`SidecarClient`] speaks its HTTP contract; [`SyncEngine`] joins it
//! to the local [`aoide_store::Store`]: covers first, then push, then pull, with the
//! store's merge rules applied to every pulled page.

mod client;
mod engine;
mod error;
mod types;

#[cfg(test)]
mod tests;

pub use client::{MAX_IDS_PER_CALL, SidecarClient};
pub use engine::{ImageByteSource, SyncEngine, SyncError, SyncTransport};
pub use error::SidecarError;
pub use types::{
    AnalysisCoverage, AudioAnalysis, AudioAnalysisResponse, DiscoverCandidate, DiscoverFactors,
    DiscoverResponse, Measurement, NextCandidate, NextFactors, NextMode, NextRequest, NextResponse,
    ProfileSummary, SoundBounds, SoundBoundsResponse, SyncStatus, iso_week,
};
