//! When to sync with the sidecar: at launch, when the window gains focus, and
//! at most once a minute. One sync at a time. A sync is an abortable Task, so
//! signing out drops it at whatever await it is on; the engine is built to be
//! dropped mid-way (see `aoide_sidecar::SyncEngine`).

use std::sync::Arc;

use aoide_sidecar::{SidecarClient, SyncEngine};
use aoide_store::SyncReport;

/// The shortest gap between two syncs.
pub const MIN_INTERVAL_MS: u64 = 60_000;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncThrottle {
    last_started: Option<u64>,
    running: bool,
}

impl SyncThrottle {
    /// Whether a sync asked for at `now` should start. Starting is recorded.
    pub fn request(&mut self, now: u64) -> bool {
        if self.running {
            return false;
        }
        if self
            .last_started
            .is_some_and(|last| now.saturating_sub(last) < MIN_INTERVAL_MS)
        {
            return false;
        }
        self.last_started = Some(now);
        self.running = true;
        true
    }

    pub fn finished(&mut self) {
        self.running = false;
    }
}

pub type Engine = Arc<SyncEngine<SidecarClient>>;

/// One sync, as text for the status line when it fails.
pub async fn run(engine: Engine) -> Result<SyncReport, String> {
    engine.sync().await.map_err(|error| {
        if error.is_sidecar_absent() {
            "This server has no Aoide sidecar; playlists and history stay on this computer."
                .to_owned()
        } else {
            format!("Sync didn't finish: {error}")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_syncs_then_at_most_once_a_minute() {
        let mut throttle = SyncThrottle::default();
        assert!(throttle.request(0));
        throttle.finished();
        // Focus a few seconds later: too soon.
        assert!(!throttle.request(5_000));
        assert!(!throttle.request(59_999));
        assert!(throttle.request(60_000));
    }

    #[test]
    fn one_sync_at_a_time() {
        let mut throttle = SyncThrottle::default();
        assert!(throttle.request(0));
        assert!(!throttle.request(120_000));
        throttle.finished();
        assert!(throttle.request(120_000));
    }
}
