//! Telling Jellyfin what is playing: started, progress every ten seconds and
//! on pause or resume, stopped. What makes play counts and "recently played"
//! work on the server, and what stops it showing a phantom session.
//!
//! A state machine over a clock the caller passes in, so the timing is
//! tested without waiting; the app sends the reports it returns.

use aoide_audio::EntryId;

/// How often progress is reported while playing.
pub const PROGRESS_EVERY_MS: u64 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    Started {
        item: String,
        position_ms: u64,
    },
    Progress {
        item: String,
        position_ms: u64,
        paused: bool,
    },
    Stopped {
        item: String,
        position_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Reporting {
    entry: EntryId,
    item: String,
    position_ms: u64,
    paused: bool,
    last_report_at: u64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Reporter {
    current: Option<Reporting>,
}

impl Reporter {
    /// A track was heard to start. Stops the previous one first. The same entry
    /// starting again (a seek) is not a new session on the server.
    pub fn started(&mut self, entry: EntryId, item: &str, now: u64) -> Vec<Report> {
        if self.current.as_ref().is_some_and(|c| c.entry == entry) {
            return Vec::new();
        }
        let mut reports: Vec<Report> = self.stopped().into_iter().collect();
        reports.push(Report::Started {
            item: item.to_owned(),
            position_ms: 0,
        });
        self.current = Some(Reporting {
            entry,
            item: item.to_owned(),
            position_ms: 0,
            paused: false,
            last_report_at: now,
        });
        reports
    }

    /// A position from the engine. Reports progress when ten seconds have passed
    /// since the last report.
    pub fn position(&mut self, entry: EntryId, position_ms: u64, now: u64) -> Option<Report> {
        let current = self.current.as_mut().filter(|c| c.entry == entry)?;
        current.position_ms = position_ms;
        if now.saturating_sub(current.last_report_at) < PROGRESS_EVERY_MS {
            return None;
        }
        current.last_report_at = now;
        Some(Report::Progress {
            item: current.item.clone(),
            position_ms,
            paused: current.paused,
        })
    }

    /// Paused or resumed: reported straight away, so the server's session shows it.
    pub fn paused(&mut self, paused: bool, now: u64) -> Option<Report> {
        let current = self.current.as_mut()?;
        if current.paused == paused {
            return None;
        }
        current.paused = paused;
        current.last_report_at = now;
        Some(Report::Progress {
            item: current.item.clone(),
            position_ms: current.position_ms,
            paused,
        })
    }

    /// Playback stopped, the queue ran out, or the session is ending.
    pub fn stopped(&mut self) -> Option<Report> {
        let current = self.current.take()?;
        Some(Report::Stopped {
            item: current.item,
            position_ms: current.position_ms,
        })
    }

    #[cfg(test)]
    pub fn current_item(&self) -> Option<&str> {
        self.current.as_ref().map(|c| c.item.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_track_starts_reports_progress_and_stops() {
        let mut reporter = Reporter::default();
        assert_eq!(
            reporter.started(EntryId(1), "a", 0),
            [Report::Started {
                item: "a".into(),
                position_ms: 0
            }]
        );
        assert_eq!(reporter.position(EntryId(1), 4_000, 4_000), None);
        assert_eq!(
            reporter.position(EntryId(1), 10_000, 10_000),
            Some(Report::Progress {
                item: "a".into(),
                position_ms: 10_000,
                paused: false
            })
        );
        assert_eq!(reporter.position(EntryId(1), 15_000, 15_000), None);
        assert!(reporter.position(EntryId(1), 20_250, 20_250).is_some());
        assert_eq!(
            reporter.stopped(),
            Some(Report::Stopped {
                item: "a".into(),
                position_ms: 20_250
            })
        );
        assert_eq!(reporter.stopped(), None);
    }

    #[test]
    fn the_next_track_stops_the_previous_first() {
        let mut reporter = Reporter::default();
        reporter.started(EntryId(1), "a", 0);
        reporter.position(EntryId(1), 180_000, 180_000);
        assert_eq!(
            reporter.started(EntryId(2), "b", 181_000),
            [
                Report::Stopped {
                    item: "a".into(),
                    position_ms: 180_000
                },
                Report::Started {
                    item: "b".into(),
                    position_ms: 0
                }
            ]
        );
        assert_eq!(reporter.current_item(), Some("b"));
    }

    #[test]
    fn a_seek_is_not_a_new_session() {
        let mut reporter = Reporter::default();
        reporter.started(EntryId(1), "a", 0);
        assert!(reporter.started(EntryId(1), "a", 5_000).is_empty());
    }

    #[test]
    fn pause_and_resume_report_at_once() {
        let mut reporter = Reporter::default();
        reporter.started(EntryId(1), "a", 0);
        reporter.position(EntryId(1), 3_000, 3_000);
        assert_eq!(
            reporter.paused(true, 3_500),
            Some(Report::Progress {
                item: "a".into(),
                position_ms: 3_000,
                paused: true
            })
        );
        assert_eq!(reporter.paused(true, 4_000), None);
        assert!(matches!(
            reporter.paused(false, 9_000),
            Some(Report::Progress { paused: false, .. })
        ));
    }

    #[test]
    fn positions_of_another_entry_are_ignored() {
        let mut reporter = Reporter::default();
        reporter.started(EntryId(1), "a", 0);
        assert_eq!(reporter.position(EntryId(2), 50_000, 50_000), None);
    }
}
