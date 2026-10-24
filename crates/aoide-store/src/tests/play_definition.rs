//! Port of `PlayDefinitionTests.swift` ("One definition of a play"): the count beside a
//! track and the count a smart rule reads must be the same number.

use std::collections::BTreeSet;

use super::store;
use crate::{
    Calendar, Field, Match, Millis, Operator, PlayClassifier, PlayEvent, PlaySource, Record,
    SmartRule, SmartRules, Store, TrackLink, TrackRef, TrackStats,
};

const NOW: Millis = 1_700_000_000_000;
const DURATIONS: [Option<i64>; 7] = [
    None,
    Some(0),
    Some(90_000),
    Some(180_000),
    Some(200_000),
    Some(600_000),
    Some(1_200_000),
];
const LISTENS: [i64; 21] = [
    0, 1, 17_999, 18_000, 29_999, 30_000, 44_999, 45_000, 47_999, 48_000, 89_999, 90_000, 99_999,
    100_000, 119_999, 120_000, 239_999, 240_000, 300_000, 600_000, 1_200_000,
];

fn case_id(duration: Option<i64>, played: i64) -> String {
    format!("d{:09}-p{:09}", duration.unwrap_or(-1), played)
}

/// One track per (duration, listen), each with exactly one event.
fn matrix(classified: bool) -> Store {
    let store = store();
    for duration in DURATIONS {
        for played in LISTENS {
            let id = case_id(duration, played);
            let mut track = TrackRef::new(&id, &id, &id, "A", "A", NOW);
            track.duration_ms = duration;
            store.write_local(|tx| track.insert(tx)).unwrap();
            if classified {
                // The player's path: the classifier stamps `completed` and `skipped`.
                store
                    .record_play(
                        &TrackLink::new(&id, &id),
                        NOW - 1_000,
                        played,
                        duration,
                        PlaySource::Unknown,
                        None,
                    )
                    .unwrap();
            } else {
                // A sync or an import: a finished event carrying neither verdict.
                store
                    .append(PlayEvent::new(&id, &id, NOW - 1_000, "other").finished(
                        Some(NOW),
                        played,
                        false,
                        false,
                    ))
                    .unwrap();
            }
        }
    }
    store
}

fn all_ids() -> Vec<String> {
    DURATIONS
        .iter()
        .flat_map(|d| LISTENS.iter().map(move |p| case_id(*d, *p)))
        .collect()
}

fn smart_set(store: &Store, field: Field, count: i64) -> BTreeSet<String> {
    let rules = SmartRules::new(
        Match::All,
        vec![SmartRule::new(field, Operator::Is, count).unwrap()],
        None,
        None,
    )
    .unwrap();
    store
        .evaluate(&rules, NOW, Calendar::Local)
        .unwrap()
        .into_iter()
        .collect()
}

fn stats_set(store: &Store, count: i64, pick: fn(&TrackStats) -> i64) -> BTreeSet<String> {
    store
        .stats_for(&all_ids())
        .unwrap()
        .into_values()
        .filter(|stats| pick(stats) == count)
        .map(|stats| stats.jellyfin_id)
        .collect()
}

#[test]
fn the_play_count_beside_a_track_and_the_one_a_smart_rule_reads_are_the_same_number() {
    for classified in [true, false] {
        let store = matrix(classified);
        for count in [0, 1] {
            assert_eq!(
                stats_set(&store, count, |s| s.play_count),
                smart_set(&store, Field::PlayCount, count),
                "classified {classified}"
            );
        }
    }
}

#[test]
fn the_skip_count_beside_a_track_and_the_one_a_smart_rule_reads_are_the_same_number() {
    for classified in [true, false] {
        let store = matrix(classified);
        for count in [0, 1] {
            assert_eq!(
                stats_set(&store, count, |s| s.skip_count),
                smart_set(&store, Field::SkipCount, count),
                "classified {classified}"
            );
        }
    }
}

#[test]
fn the_sql_and_the_classifier_reach_the_same_verdict_on_every_boundary() {
    let store = matrix(true);
    for duration in DURATIONS {
        for played in LISTENS {
            let expected = PlayClassifier::classify(played, duration, None);
            let actual = store.stats(&case_id(duration, played)).unwrap();
            assert_eq!(
                actual.play_count,
                i64::from(expected.counts_as_play),
                "play: {duration:?} {played}"
            );
            assert_eq!(
                actual.skip_count,
                i64::from(expected.skipped),
                "skip: {duration:?} {played}"
            );
        }
    }
}

#[test]
fn no_listen_is_ever_both_a_play_and_a_skip_and_the_middle_band_survives() {
    let store = matrix(true);
    let mut neither = 0;
    for duration in DURATIONS {
        for played in LISTENS {
            let stats = store.stats(&case_id(duration, played)).unwrap();
            assert!(stats.play_count + stats.skip_count <= 1);
            if stats.play_count + stats.skip_count == 0 {
                neither += 1;
            }
        }
    }
    assert!(
        neither > 0,
        "the neither band collapsed; the two thresholds have met"
    );
}

/// A track played in March and skipped this morning was last played in March.
#[test]
fn a_skip_does_not_move_the_last_played_date_in_either_consumer() {
    let store = store();
    let day: Millis = 86_400_000;
    let mut track = TrackRef::new("t1", "k1", "T", "A", "A", NOW);
    track.duration_ms = Some(200_000);
    store.write_local(|tx| track.insert(tx)).unwrap();
    let link = TrackLink::new("t1", "k1");
    store
        .record_play(
            &link,
            NOW - 200 * day,
            200_000,
            Some(200_000),
            PlaySource::Unknown,
            None,
        )
        .unwrap();
    store
        .record_play(
            &link,
            NOW - day,
            3_000,
            Some(200_000),
            PlaySource::Unknown,
            None,
        )
        .unwrap();

    let stats = store.stats("t1").unwrap();
    assert_eq!(stats.play_count, 1);
    assert_eq!(stats.skip_count, 1);
    assert_eq!(stats.last_played_at, Some(NOW - 200 * day));
    assert_eq!(stats.last_started_at, Some(NOW - day));

    let not_recently = SmartRules::new(
        Match::All,
        vec![SmartRule::new(Field::LastPlayed, Operator::NotInTheLast, "30d").unwrap()],
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        store.evaluate(&not_recently, NOW, Calendar::Local).unwrap(),
        ["t1"]
    );
}
