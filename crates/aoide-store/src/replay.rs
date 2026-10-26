//! Replay: your listening as a story — this month, this year, or all of it. The
//! numbers are [`Recap`]'s; this is the arithmetic the screen needs around them, so
//! the figure on screen is the figure that was tested (iOS `ReplayView`, desktop
//! `replay-period.ts`).

use chrono::{DateTime, Datelike, NaiveDate, Utc};

use crate::Millis;
use crate::error::Result;
use crate::history::Recap;
use crate::smart::{Calendar, resolve_local};
use crate::store::Store;

/// The three periods, in the order the segmented control shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReplayPeriod {
    /// Since midnight on the 1st of this month.
    Month,
    /// Since midnight on 1 January.
    Year,
    /// Since the epoch: everything.
    AllTime,
}

impl ReplayPeriod {
    /// All three, in display order.
    pub const ALL: [ReplayPeriod; 3] = [Self::Month, Self::Year, Self::AllTime];

    /// What the segmented control says.
    pub fn label(self) -> &'static str {
        match self {
            Self::Month => "This Month",
            Self::Year => "This Year",
            Self::AllTime => "All Time",
        }
    }

    /// Where the period's window opens: midnight on the 1st of this month or on 1
    /// January, on the wall clock of `calendar` — "this month" means the month on the
    /// wall, so a listener east of Greenwich does not see the first evening of it
    /// filed under last month. All time opens at the epoch.
    pub fn start(self, now: Millis, calendar: Calendar) -> Millis {
        let first = |date: Option<NaiveDate>| -> Millis {
            let Some(midnight) = date.and_then(|d| d.and_hms_opt(0, 0, 0)) else {
                return 0;
            };
            match calendar {
                Calendar::Utc => midnight.and_utc().timestamp_millis(),
                Calendar::Local => resolve_local(midnight).map_or(0, |at| at.timestamp_millis()),
            }
        };
        let today = match calendar {
            Calendar::Utc => DateTime::<Utc>::from_timestamp_millis(now).map(|at| at.date_naive()),
            Calendar::Local => DateTime::<Utc>::from_timestamp_millis(now)
                .map(|at| at.with_timezone(&chrono::Local).date_naive()),
        };
        match self {
            Self::AllTime => 0,
            Self::Month => first(today.and_then(|d| d.with_day(1))),
            Self::Year => first(today.and_then(|d| NaiveDate::from_ymd_opt(d.year(), 1, 1))),
        }
    }
}

/// Milliseconds as hours, to one decimal: a tile reads "12.5", not "12.4999".
pub fn hours_listened(total_ms_played: i64) -> f64 {
    (total_ms_played as f64 / 3_600_000.0 * 10.0).round() / 10.0
}

impl Recap {
    /// The Hours tile.
    pub fn hours(&self) -> f64 {
        hours_listened(self.total_ms_played)
    }

    /// Nothing to show: the empty state, not a page of zeros.
    pub fn is_empty(&self) -> bool {
        self.total_plays == 0
    }

    /// The busiest day as a date to format — the day it names, never shifted by a
    /// time zone on the way to the screen.
    pub fn busiest_date(&self) -> Option<NaiveDate> {
        NaiveDate::parse_from_str(&self.busiest_day.as_ref()?.day, "%Y-%m-%d").ok()
    }
}

impl Store {
    /// The recap for `period` as of `now`: from the period's start, with no end — a
    /// play another device stamped a little ahead of this clock still counts. The
    /// busiest day is bucketed in the same `calendar` the period is measured in.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn replay(&self, period: ReplayPeriod, now: Millis, calendar: Calendar) -> Result<Recap> {
        self.recap_in(period.start(now, calendar), Millis::MAX, calendar)
    }
}
