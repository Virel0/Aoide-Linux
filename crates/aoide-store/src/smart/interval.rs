//! Relative dates: `-30d`, `-6m`, `1y`, and the calendar they are counted in.

use chrono::{DateTime, Days, Local, Months, NaiveDateTime, TimeZone, Utc};

use super::error::SmartPlaylistError;
use crate::Millis;

/// The unit of a [`RelativeInterval`]: its suffix letter is the first letter of the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimeUnit {
    /// `h`: exact hours.
    Hours,
    /// `d`: calendar days, keeping the wall-clock time.
    Days,
    /// `w`: seven calendar days.
    Weeks,
    /// `m`: calendar months; a month before 31 March is the end of February.
    Months,
    /// `y`: twelve calendar months.
    Years,
}

/// A span written the way rules store it: `-30d`, `-6m`, `1y`. Kept relative until
/// evaluation, so "not played in a month" keeps moving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RelativeInterval {
    /// How many units; zero is now.
    pub amount: u32,
    /// Which unit.
    pub unit: TimeUnit,
}

/// Which calendar month and year arithmetic happens in. A parameter because the answer
/// depends on a time zone: a test using the machine's would pass in one city and fail
/// in another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Calendar {
    /// Coordinated universal time: the same answer on every machine, as tests need.
    Utc,
    /// The machine's own time zone, which is what a listener means by "a month ago".
    Local,
}

impl RelativeInterval {
    /// Parses `-30d` or `30d`: the leading minus is decoration, since every operator
    /// already looks backwards.
    ///
    /// # Errors
    ///
    /// [`SmartPlaylistError::MalformedInterval`] when the text is not digits followed by
    /// one of `h`, `d`, `w`, `m` or `y`, or the number does not fit.
    pub fn parse(written: &str) -> std::result::Result<Self, SmartPlaylistError> {
        let refuse = || SmartPlaylistError::MalformedInterval(written.to_owned());
        let lowered = written.trim_matches([' ', '\t']).to_lowercase();
        let body = lowered.strip_prefix('-').unwrap_or(&lowered);
        let unit = match body.chars().last() {
            Some('h') => TimeUnit::Hours,
            Some('d') => TimeUnit::Days,
            Some('w') => TimeUnit::Weeks,
            Some('m') => TimeUnit::Months,
            Some('y') => TimeUnit::Years,
            _ => return Err(refuse()),
        };
        let digits = &body[..body.len() - 1];
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
            return Err(refuse());
        }
        let amount = digits.parse().map_err(|_| refuse())?;
        Ok(Self { amount, unit })
    }

    /// The instant this far before `now`.
    ///
    /// Calendar arithmetic rather than multiples of a day: a month before 31 March is
    /// the end of February. Hours are an absolute shift; days, weeks, months and years
    /// keep the wall-clock time in `calendar`.
    pub fn ago(&self, now: Millis, calendar: Calendar) -> Millis {
        if self.unit == TimeUnit::Hours {
            return now.saturating_sub(i64::from(self.amount) * 3_600_000);
        }
        let Some(instant) = DateTime::<Utc>::from_timestamp_millis(now) else {
            return now;
        };
        match calendar {
            Calendar::Utc => self
                .shift(instant.naive_utc())
                .map_or(now, |naive| naive.and_utc().timestamp_millis()),
            Calendar::Local => {
                let local = instant.with_timezone(&Local).naive_local();
                self.shift(local)
                    .and_then(resolve_local)
                    .map_or(now, |resolved| resolved.timestamp_millis())
            }
        }
    }

    fn shift(&self, naive: NaiveDateTime) -> Option<NaiveDateTime> {
        let amount = self.amount;
        match self.unit {
            TimeUnit::Hours => None,
            TimeUnit::Days => naive.checked_sub_days(Days::new(u64::from(amount))),
            TimeUnit::Weeks => naive.checked_sub_days(Days::new(u64::from(amount) * 7)),
            TimeUnit::Months => naive.checked_sub_months(Months::new(amount)),
            TimeUnit::Years => naive.checked_sub_months(Months::new(amount.checked_mul(12)?)),
        }
    }
}

/// A local wall-clock time to an instant: the earlier reading of an ambiguous hour,
/// and the first valid time after a skipped one.
pub(crate) fn resolve_local(naive: NaiveDateTime) -> Option<DateTime<Local>> {
    if let Some(resolved) = Local.from_local_datetime(&naive).earliest() {
        return Some(resolved);
    }
    Local
        .from_local_datetime(&(naive + chrono::Duration::hours(1)))
        .earliest()
}
