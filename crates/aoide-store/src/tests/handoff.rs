//! Handoff: the debounced publisher, the local fallback for other devices' queues, and
//! pick-up — the iOS `ResumeGrid.pickUp` / `QueueRestorer` and desktop `pick-up.ts`
//! rules.

use std::collections::HashSet;

use super::{pending, store};
use crate::{
    Millis, QueueBroadcast, QueueFreshness, QueuePolicy, QueueSnapshot, QueueState, Record,
    ResumableQueue, Store,
};

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|id| (*id).to_owned()).collect()
}

fn snapshot(tracks: &[&str], position: i64, elapsed_ms: i64) -> QueueSnapshot {
    QueueSnapshot {
        device_name: "Desk".to_owned(),
        track_ids: ids(tracks),
        position,
        elapsed_ms,
    }
}

fn queue_ops(store: &Store) -> usize {
    pending(store)
        .iter()
        .filter(|op| op.entity == "queueState")
        .count()
}

fn receive(store: &Store, device: &str, name: &str, tracks: &[&str], at: Millis) {
    store
        .write_local(|tx| QueueState::new(device, name, tracks, 1, 30_000, at).insert(tx))
        .unwrap();
}

fn remote(tracks: &[&str], position: usize, elapsed_ms: i64) -> ResumableQueue {
    ResumableQueue {
        device_id: "phone-2".to_owned(),
        device_name: "Phone".to_owned(),
        track_ids: ids(tracks),
        position,
        elapsed_ms,
        age_ms: 0,
        observed_at: 0,
        freshness: QueueFreshness::Server,
    }
}

// MARK: - Publishing

#[test]
fn nothing_is_written_until_the_burst_settles() {
    let store = store();
    let mut broadcast = QueueBroadcast::new();
    broadcast.note(snapshot(&["a", "b"], 0, 0), 1_000);
    assert_eq!(
        broadcast.due_at(),
        Some(1_000 + QueueBroadcast::DEBOUNCE_MS)
    );
    assert!(!broadcast.flush_due(&store, 1_500).unwrap());
    assert_eq!(store.local_queue().unwrap(), None);

    assert!(broadcast.flush_due(&store, 2_500).unwrap());
    let saved = store.local_queue().unwrap().unwrap();
    assert_eq!(saved.track_id_list(), ids(&["a", "b"]));
    assert_eq!(saved.device_name, "Desk");
    assert_eq!(broadcast.due_at(), None);
}

/// Five skips in a row are one row and one op, carrying where the skipping ended.
#[test]
fn a_burst_of_track_changes_is_one_write() {
    let store = store();
    let mut broadcast = QueueBroadcast::new();
    for position in 0..5 {
        broadcast.note(
            snapshot(&["a", "b", "c", "d", "e"], position, 0),
            1_000 + position * 200,
        );
    }
    assert!(
        broadcast
            .flush_due(&store, 1_800 + QueueBroadcast::DEBOUNCE_MS)
            .unwrap()
    );
    assert_eq!(store.local_queue().unwrap().unwrap().position, 4);
    assert_eq!(queue_ops(&store), 1);
}

/// A steady stream of edits must not hold the row back for ever.
#[test]
fn a_stream_of_changes_is_written_by_the_longest_wait() {
    let store = store();
    let mut broadcast = QueueBroadcast::new();
    let mut now = 0;
    let mut position = 0;
    while now < QueueBroadcast::MAX_WAIT_MS {
        broadcast.note(snapshot(&["a", "b", "c"], position % 3, 0), now);
        assert!(broadcast.due_at().unwrap() <= QueueBroadcast::MAX_WAIT_MS);
        now += 1_000;
        position += 1;
    }
    assert!(
        broadcast
            .flush_due(&store, QueueBroadcast::MAX_WAIT_MS)
            .unwrap()
    );
}

/// Position ticks alone never reschedule a waiting write, and the store's threshold
/// keeps them from writing a row each.
#[test]
fn elapsed_time_alone_rides_along_without_rescheduling() {
    let store = store();
    let mut broadcast = QueueBroadcast::new();
    broadcast.note(snapshot(&["a"], 0, 0), 0);
    let due = broadcast.due_at();
    broadcast.note(snapshot(&["a"], 0, 1_000), 1_000);
    assert_eq!(broadcast.due_at(), due);
    assert!(broadcast.flush_due(&store, 5_000).unwrap());
    assert_eq!(store.local_queue().unwrap().unwrap().elapsed_ms, 1_000);

    // Two seconds later: under the threshold, so nothing new is written.
    broadcast.note(snapshot(&["a"], 0, 3_000), 6_000);
    assert!(!broadcast.flush_due(&store, 10_000).unwrap());
    assert_eq!(queue_ops(&store), 1);
}

#[test]
fn flushing_now_ignores_the_debounce_and_nothing_waiting_writes_nothing() {
    let store = store();
    let mut broadcast = QueueBroadcast::new();
    assert!(!broadcast.flush_now(&store, 0).unwrap());
    broadcast.note(snapshot(&["a"], 0, 0), 0);
    assert!(broadcast.flush_now(&store, 1).unwrap());
    assert!(!broadcast.flush_now(&store, 2).unwrap());
}

#[test]
fn a_published_queue_is_windowed_like_any_saved_one() {
    let store = store();
    let mut broadcast = QueueBroadcast::new();
    let long: Vec<String> = (0..QueuePolicy::MAX_TRACKS + 100)
        .map(|i| format!("t{i}"))
        .collect();
    broadcast.note(
        QueueSnapshot {
            device_name: "Desk".to_owned(),
            track_ids: long,
            position: 300,
            elapsed_ms: 0,
        },
        0,
    );
    broadcast.flush_now(&store, 1).unwrap();
    let saved = store.local_queue().unwrap().unwrap();
    assert_eq!(saved.track_id_list().len(), QueuePolicy::MAX_TRACKS);
    assert_eq!(
        saved.track_id_list()[saved.position as usize],
        "t300",
        "the current track survives the window"
    );
}

// MARK: - Reading the local rows

#[test]
fn local_resumable_queues_are_other_devices_newest_first_within_a_day() {
    let store = store();
    let now: Millis = 10 * QueuePolicy::RESUME_WINDOW;
    store
        .save_queue("Desk", &ids(&["mine"]), 0, 0, now)
        .unwrap();
    receive(&store, "car", "Car", &["c1", "c2"], now - 60_000);
    receive(&store, "phone-2", "Phone", &["p1", "p2"], now - 1_000);
    receive(
        &store,
        "old",
        "Old Laptop",
        &["o1"],
        now - QueuePolicy::RESUME_WINDOW - 1,
    );
    receive(&store, "empty", "Signed Out", &[], now);

    let queues = store.local_resumable_queues(now).unwrap();
    let devices: Vec<&str> = queues.iter().map(|q| q.device_id.as_str()).collect();
    assert_eq!(devices, ["phone-2", "car"]);
    let phone = &queues[0];
    assert_eq!(phone.device_name, "Phone");
    assert_eq!(phone.position, 1);
    assert_eq!(phone.elapsed_ms, 30_000);
    assert_eq!(phone.age_ms, 1_000);
    assert_eq!(phone.played_at(), now - 1_000);
    assert_eq!(phone.freshness, QueueFreshness::Writer);
    assert_eq!(phone.current_track_id(), Some("p2"));
    assert_eq!(phone.title(), "Pick up from Phone");
}

#[test]
fn a_position_past_the_end_reads_as_the_last_track() {
    let state = QueueState::new("phone-2", "Phone", &["a", "b"], 9, 0, 0);
    let queue = ResumableQueue::from_local(&state, 0).unwrap();
    assert_eq!(queue.position, 1);
    assert!(
        ResumableQueue::from_local(&QueueState::new("x", "X", &[] as &[&str], 0, 0, 0), 0)
            .is_none()
    );
}

// MARK: - Picking up

/// Previous must still reach the start of what they were listening to.
#[test]
fn picking_up_keeps_earlier_tracks_and_starts_at_the_named_one() {
    let plan = remote(&["a", "b", "c", "d"], 2, 41_200)
        .pick_up(|_| true)
        .unwrap();
    assert_eq!(plan.track_ids, ids(&["a", "b", "c", "d"]));
    assert_eq!(plan.start_index, 2);
    assert_eq!(plan.elapsed_ms, 41_200);
}

/// A track this server no longer has is left out without shifting the start.
#[test]
fn a_missing_earlier_track_does_not_move_the_start() {
    let gone: HashSet<&str> = ["a"].into();
    let plan = remote(&["a", "b", "c", "d"], 2, 5_000)
        .pick_up(|id| !gone.contains(id))
        .unwrap();
    assert_eq!(plan.track_ids, ids(&["b", "c", "d"]));
    assert_eq!(plan.start_index, 1, "still on c");
    assert_eq!(plan.elapsed_ms, 5_000);
}

/// The elapsed time belonged to the missing track, so the next one starts from 0.
#[test]
fn a_missing_current_track_starts_the_next_one_from_the_top() {
    let gone: HashSet<&str> = ["c"].into();
    let plan = remote(&["a", "b", "c", "d"], 2, 5_000)
        .pick_up(|id| !gone.contains(id))
        .unwrap();
    assert_eq!(plan.track_ids, ids(&["a", "b", "d"]));
    assert_eq!(plan.start_index, 2, "d");
    assert_eq!(plan.elapsed_ms, 0);
}

#[test]
fn with_nothing_after_it_the_last_playable_track_starts() {
    let gone: HashSet<&str> = ["c", "d"].into();
    let plan = remote(&["a", "b", "c", "d"], 2, 5_000)
        .pick_up(|id| !gone.contains(id))
        .unwrap();
    assert_eq!(plan.track_ids, ids(&["a", "b"]));
    assert_eq!(plan.start_index, 1);
    assert_eq!(plan.elapsed_ms, 0);
}

#[test]
fn nothing_playable_is_nothing_to_pick_up() {
    assert_eq!(remote(&["a", "b"], 0, 0).pick_up(|_| false), None);
}
