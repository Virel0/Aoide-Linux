//! Jellyfin measures time in .NET ticks: 10,000,000 per second, 100 ns each.

use std::time::Duration;

/// Ticks in one second.
pub const TICKS_PER_SECOND: i64 = 10_000_000;

const NANOS_PER_TICK: u32 = 100;

/// Ticks as a duration. Negative ticks (which a malformed file can produce) are zero.
pub fn to_duration(ticks: i64) -> Duration {
    if ticks <= 0 {
        return Duration::ZERO;
    }
    let seconds = ticks / TICKS_PER_SECOND;
    let rest = (ticks % TICKS_PER_SECOND) as u32;
    Duration::new(seconds as u64, rest * NANOS_PER_TICK)
}

/// A duration as ticks, saturating at `i64::MAX`. Sub-tick precision is dropped.
pub fn from_duration(duration: Duration) -> i64 {
    i64::try_from(duration.as_nanos() / u128::from(NANOS_PER_TICK)).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_second_is_ten_million_ticks() {
        assert_eq!(to_duration(TICKS_PER_SECOND), Duration::from_secs(1));
        assert_eq!(from_duration(Duration::from_secs(1)), TICKS_PER_SECOND);
    }

    #[test]
    fn fractions_survive_both_ways() {
        assert_eq!(to_duration(25_000_000), Duration::from_millis(2_500));
        assert_eq!(from_duration(Duration::from_millis(2_500)), 25_000_000);
        assert_eq!(to_duration(1), Duration::from_nanos(100));
    }

    #[test]
    fn a_long_track_round_trips() {
        // 44,000,000,000 ticks is 4,400 seconds.
        let ticks = 44_000_000_000;
        assert_eq!(to_duration(ticks), Duration::from_secs(4_400));
        assert_eq!(from_duration(to_duration(ticks)), ticks);
    }

    #[test]
    fn negative_ticks_are_zero() {
        assert_eq!(to_duration(-5), Duration::ZERO);
    }

    #[test]
    fn sub_tick_precision_is_dropped_not_rounded_up() {
        assert_eq!(from_duration(Duration::from_nanos(199)), 1);
    }

    #[test]
    fn huge_durations_saturate() {
        assert_eq!(from_duration(Duration::MAX), i64::MAX);
        assert_eq!(
            to_duration(i64::MAX).as_secs(),
            (i64::MAX / TICKS_PER_SECOND) as u64
        );
    }
}
