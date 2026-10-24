//! When to sync with the sidecar: at launch, when the window gains focus, and
//! at most once a minute; or straight away when the listener asks (Sync Now
//! in Settings). One sync at a time. A sync is an abortable Task, so
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

    /// Sync Now: starts unless one is running, however recent the last.
    pub fn request_now(&mut self, now: u64) -> bool {
        if self.running {
            return false;
        }
        self.last_started = Some(now);
        self.running = true;
        true
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn finished(&mut self) {
        self.running = false;
    }
}

/// What Settings shows about syncing.
#[derive(Debug, Clone, Copy)]
pub struct Status<'a> {
    /// There is a sidecar client to sync with.
    pub available: bool,
    pub running: bool,
    /// The last sync's outcome, in words.
    pub last: Option<&'a Result<String, String>>,
}

/// One sync's outcome in plain words.
pub fn describe(report: &SyncReport) -> String {
    let changes = |n: usize| {
        if n == 1 {
            "1 change".to_owned()
        } else {
            format!("{n} changes")
        }
    };
    let mut done = Vec::new();
    if report.pushed > 0 {
        done.push(format!("sent {}", changes(report.pushed)));
    }
    if report.applied > 0 {
        done.push(format!("brought in {}", changes(report.applied)));
    }
    let mut words = if done.is_empty() {
        "Synced. Nothing had changed.".to_owned()
    } else {
        format!("Synced: {}.", done.join(" and "))
    };
    if report.quarantined_outbound > 0 {
        words.push_str(&format!(
            " {} couldn't be sent and {} set aside.",
            changes(report.quarantined_outbound),
            if report.quarantined_outbound == 1 {
                "was"
            } else {
                "were"
            }
        ));
    }
    words
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
    fn sync_now_ignores_the_minute_but_not_a_running_sync() {
        let mut throttle = SyncThrottle::default();
        assert!(throttle.request(0));
        assert!(!throttle.request_now(1_000), "one is running");
        assert!(throttle.is_running());
        throttle.finished();
        assert!(throttle.request_now(1_000));
        throttle.finished();
        // And a Sync Now counts as the last sync for the minute.
        assert!(!throttle.request(30_000));
    }

    #[test]
    fn outcomes_read_as_plain_words() {
        assert_eq!(
            describe(&SyncReport::default()),
            "Synced. Nothing had changed."
        );
        let report = SyncReport {
            pushed: 3,
            pulled: 5,
            applied: 1,
            ..SyncReport::default()
        };
        assert_eq!(
            describe(&report),
            "Synced: sent 3 changes and brought in 1 change."
        );
        let report = SyncReport {
            applied: 2,
            quarantined_outbound: 1,
            ..SyncReport::default()
        };
        assert_eq!(
            describe(&report),
            "Synced: brought in 2 changes. 1 change couldn't be sent and was set aside."
        );
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
