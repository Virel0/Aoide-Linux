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
mod import;
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
pub use import::{
    ImportError, ImportMatch, ImportResult, ImportedPlaylist, ImportedTrack, JellyfinSearch,
    LibraryTrack, MATCH_ROWS_PER_REQUEST, MatchedBy, MatchedTrack, SEARCH_LIMIT, SEARCH_TERM_CHARS,
    SPOTIFY_EMBED_TRACK_LIMIT, SidecarMatch, TrackMatcher, Verdict, fetch_spotify_playlist,
    fetch_spotify_playlist_from, match_playlist, parse_playlist_csv, parse_spotify_embed,
    spotify_embed_url, spotify_playlist_id,
};
pub use types::{
    AnalysisCoverage, AudioAnalysis, AudioAnalysisResponse, DiscoverCandidate, DiscoverFactors,
    DiscoverResponse, Measurement, NextCandidate, NextFactors, NextMode, NextRequest, NextResponse,
    PlaylistShare, ProfileSummary, QueueEntry, ShareRequest, SoundBounds, SoundBoundsResponse,
    SyncStatus, iso_week,
};
