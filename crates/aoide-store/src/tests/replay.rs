//! Replay: the periods, the hours, the busiest day as a date (desktop
//! `replay-period.test.ts`), and a recap for a period read from the store.

use chrono::{DateTime, Datelike, Local, NaiveDate, TimeZone, Timelike, Utc};

use super::store;
use crate::{
    Calendar, Millis, PlayEvent, PlaySource, ReplayPeriod, Store, TrackRef, hours_listened,
};

fn utc(y: i32, m: u32, d: u32, h: u32, min: u32, s: u32) -> Millis {
    Utc.with_ymd_and_hms(y, m, d, h, min, s)
        .unwrap()
        .timestamp_millis()
}

/// Mid-month, mid-afternoon, so a window opened at "now" or at midnight today is
/// caught as well as one a month off.
const NOW_PARTS: (i32, u32, u32, u32, u32, u32) = (2026, 9, 17, 15, 42, 7);

fn now() -> Millis {
    let (y, m, d, h, min, s) = NOW_PARTS;
    utc(y, m, d, h, min, s) + 300
}

#[test]
fn the_periods_open_at_midnight_on_the_first_and_on_new_year() {
    assert_eq!(
        ReplayPeriod::Month.start(now(), Calendar::Utc),
        utc(2026, 9, 1, 0, 0, 0)
    );
    assert_eq!(
        ReplayPeriod::Year.start(now(), Calendar::Utc),
        utc(2026, 1, 1, 0, 0, 0)
    );
    assert_eq!(ReplayPeriod::AllTime.start(now(), Calendar::Utc), 0);
}

/// On this machine's wall clock, whatever zone it is in: midnight on the 1st, and
/// the millisecond before it is last month.
#[test]
fn local_periods_open_on_the_wall_clock() {
    for period in [ReplayPeriod::Month, ReplayPeriod::Year] {
        let start = ReplayPeriod::start(period, now(), Calendar::Local);
        let opened = Local.timestamp_millis_opt(start).unwrap();
        assert_eq!(opened.day(), 1, "{period:?}");
        assert_eq!((opened.hour(), opened.minute(), opened.second()), (0, 0, 0));
        assert!(start <= now());
        let before = Local.timestamp_millis_opt(start - 1).unwrap();
        let today = DateTime::<Utc>::from_timestamp_millis(now())
            .unwrap()
            .with_timezone(&Local);
        match period {
            ReplayPeriod::Month => {
                assert_eq!(opened.month(), today.month());
                assert_ne!(before.month(), opened.month());
            }
            _ => {
                assert_eq!(opened.month(), 1);
                assert_eq!(opened.year(), today.year());
                assert_eq!(before.year(), opened.year() - 1);
            }
        }
    }
}

#[test]
fn the_three_periods_come_in_the_phones_order() {
    let labels: Vec<&str> = ReplayPeriod::ALL.iter().map(|p| p.label()).collect();
    assert_eq!(labels, ["This Month", "This Year", "All Time"]);
}

#[test]
fn hours_round_to_one_decimal() {
    assert_eq!(hours_listened(0), 0.0);
    assert_eq!(hours_listened(90 * 60 * 1_000), 1.5);
    assert_eq!(hours_listened(3_599_999), 1.0);
    assert_eq!(hours_listened(3_780_000), 1.1);
}

fn cache(store: &Store, id: &str, artist: &str, album: &str) {
    let mut row = TrackRef::new(id, id, id, artist, album, 1);
    row.duration_ms = Some(200_000);
    store.cache_tracks(&[row]).unwrap();
}

fn listen(store: &Store, id: &str, at: Millis) {
    store
        .append(
            PlayEvent::new(id, id, at, "phone")
                .with_source(PlaySource::Album)
                .finished(Some(at + 200_000), 200_000, true, false),
        )
        .unwrap();
}

#[test]
fn a_period_recap_counts_from_its_start_and_names_the_busiest_day_as_a_date() {
    let store = store();
    cache(&store, "t1", "Aphex Twin", "SAW");
    cache(&store, "t2", "Boards of Canada", "MHTRTC");
    listen(&store, "t1", utc(2026, 8, 31, 23, 0, 0)); // last month
    listen(&store, "t1", utc(2026, 9, 2, 10, 0, 0));
    listen(&store, "t1", utc(2026, 9, 2, 11, 0, 0));
    listen(&store, "t1", utc(2026, 9, 2, 12, 0, 0));
    listen(&store, "t2", utc(2026, 9, 3, 23, 30, 0));
    listen(&store, "t2", utc(2026, 9, 4, 0, 30, 0));
    listen(&store, "t9", utc(2026, 9, 5, 12, 0, 0)); // not in the cache

    let month = store
        .replay(ReplayPeriod::Month, now(), Calendar::Utc)
        .unwrap();
    assert_eq!(month.total_plays, 6);
    assert!(!month.is_empty());
    assert_eq!(month.distinct_tracks, 3);
    assert_eq!(month.distinct_artists, 2);
    assert_eq!(month.distinct_albums, 2);
    assert_eq!(month.unattributed_plays, 1);
    assert_eq!(month.hours(), hours_listened(6 * 200_000));
    assert_eq!(month.top_tracks[0].jellyfin_id, "t1");
    assert_eq!(month.top_artists[0].artist, "Aphex Twin");
    assert_eq!(month.top_albums[0].album, "SAW");
    // Bucketed in UTC here: the 23:30 and the 00:30 fall on two different days.
    assert_eq!(month.busiest_date(), NaiveDate::from_ymd_opt(2026, 9, 2));
    assert_eq!(month.busiest_day.as_ref().unwrap().play_count, 3);

    let all = store
        .replay(ReplayPeriod::AllTime, now(), Calendar::Utc)
        .unwrap();
    assert_eq!(all.total_plays, 7);
}

#[test]
fn a_period_with_nothing_in_it_is_the_empty_state() {
    let recap = store()
        .replay(ReplayPeriod::Year, now(), Calendar::Utc)
        .unwrap();
    assert!(recap.is_empty());
    assert_eq!(recap.busiest_date(), None);
    assert_eq!(recap.hours(), 0.0);
}
