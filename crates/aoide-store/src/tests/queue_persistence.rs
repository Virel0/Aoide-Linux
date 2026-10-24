//! Port of `QueuePersistenceTests.swift` ("Queue persistence").

use super::{count, pending, store};
use crate::{Millis, Op, QueuePolicy, QueueState, Record, Store};

fn tracks(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

fn numbered(count: usize) -> Vec<String> {
    (0..count).map(|i| format!("t{i}")).collect()
}

/// A queue belonging to another device, landing without an op as an applied one does.
fn receive(
    store: &Store,
    device_id: &str,
    device_name: &str,
    track_ids: &[&str],
    updated_at: Millis,
) {
    store
        .write_local(|tx| {
            QueueState::new(device_id, device_name, track_ids, 0, 0, updated_at).insert(tx)
        })
        .unwrap();
}

fn queue_ops(store: &Store) -> Vec<Op> {
    pending(store)
        .into_iter()
        .filter(|op| op.entity == "queueState")
        .collect()
}

fn local(store: &Store) -> QueueState {
    store.local_queue().unwrap().expect("a saved queue")
}

fn device_ids(queues: &[QueueState]) -> Vec<&str> {
    queues.iter().map(|q| q.device_id.as_str()).collect()
}

// MARK: - Storing

#[test]
fn a_saved_queue_comes_back_whole() {
    let store = store();
    store
        .save_queue(
            "Someone's iPhone",
            &tracks(&["a", "b", "c"]),
            1,
            42_000,
            1_000,
        )
        .unwrap();
    let restored = local(&store);
    assert_eq!(restored.device_id, "phone");
    assert_eq!(restored.device_name, "Someone's iPhone");
    assert_eq!(restored.track_id_list(), ["a", "b", "c"]);
    assert_eq!(restored.position, 1);
    assert_eq!(restored.elapsed_ms, 42_000);
    assert_eq!(restored.updated_at, 1_000);
    assert_eq!(restored.origin_device, "phone");
}

#[test]
fn saving_a_queue_writes_its_op_keyed_by_the_device() {
    let store = store();
    store
        .save_queue("iPhone", &tracks(&["a"]), 0, 0, 500)
        .unwrap();
    let ops = queue_ops(&store);
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].entity_id, "phone");
    assert_eq!(ops[0].operation, "upsert");
    assert_eq!(ops[0].created_at, 500);
    assert!(ops[0].payload.contains("\"deviceName\":\"iPhone\""));
}

#[test]
fn repeated_saves_supersede_their_unpushed_op_instead_of_stacking() {
    let store = store();
    for second in 0..10 {
        store
            .save_queue("iPhone", &tracks(&["a", "b"]), 0, second * 1_000, second)
            .unwrap();
    }
    let ops = queue_ops(&store);
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].created_at, 9);
    assert!(ops[0].payload.contains("\"elapsedMs\":9000"));
}

/// Dropping an acknowledged op would throw away the key that makes a retry safe.
#[test]
fn coalescing_leaves_acknowledged_ops_alone() {
    let store = store();
    store
        .save_queue("iPhone", &tracks(&["a"]), 0, 0, 1)
        .unwrap();
    let first = queue_ops(&store).remove(0);
    store.mark_synced(&[first.op_id]).unwrap();
    store
        .save_queue("iPhone", &tracks(&["a"]), 0, 9_000, 2)
        .unwrap();
    assert_eq!(queue_ops(&store).len(), 1);
    assert_eq!(count::<Op>(&store), 2);
}

#[test]
fn an_empty_queue_survives_the_json_round_trip_as_an_empty_list() {
    let store = store();
    store.save_queue("iPhone", &[], 0, 0, 1).unwrap();
    let restored = local(&store);
    assert_eq!(restored.track_ids, "[]");
    assert!(restored.track_id_list().is_empty());
    assert_eq!(restored.position, 0);
}

#[test]
fn ids_with_characters_json_has_to_escape_come_back_unchanged() {
    let store = store();
    let ids = tracks(&[
        "plain",
        "with \"quotes\"",
        "with\\backslash",
        "with/slash",
        "unicode–ダッシュ",
    ]);
    store.save_queue("iPhone", &ids, 0, 0, 1).unwrap();
    assert_eq!(local(&store).track_id_list(), ids);
}

// MARK: - Resuming

#[test]
fn resume_offers_other_devices_queues_and_never_this_ones() {
    let store = store();
    store
        .save_queue("iPhone", &tracks(&["a"]), 0, 0, 1_000)
        .unwrap();
    receive(&store, "mac", "Studio", &["t1"], 900);
    assert_eq!(
        device_ids(
            &store
                .resumable_queues(1_000, QueuePolicy::RESUME_WINDOW)
                .unwrap()
        ),
        ["mac"]
    );
}

#[test]
fn resume_offers_the_most_recently_updated_queue_first() {
    let store = store();
    let now: Millis = 10_000_000;
    receive(&store, "mac", "Studio", &["t1"], now - 60_000);
    receive(&store, "ipad", "iPad", &["t1"], now - 1_000);
    receive(&store, "carplay", "Car", &["t1"], now - 3_600_000);
    assert_eq!(
        device_ids(
            &store
                .resumable_queues(now, QueuePolicy::RESUME_WINDOW)
                .unwrap()
        ),
        ["ipad", "mac", "carplay"]
    );
}

#[test]
fn a_queue_older_than_the_resume_window_is_not_offered() {
    let store = store();
    let now: Millis = 10_000_000_000;
    receive(&store, "mac", "Studio", &["t1"], now - 2 * 60 * 60 * 1000);
    receive(
        &store,
        "ipad",
        "iPad",
        &["t1"],
        now - 3 * 24 * 60 * 60 * 1000,
    );
    assert_eq!(
        device_ids(
            &store
                .resumable_queues(now, QueuePolicy::RESUME_WINDOW)
                .unwrap()
        ),
        ["mac"]
    );
    // Not offering is a display decision; the row is still there to sync.
    assert_eq!(store.resumable_queues(now, Millis::MAX).unwrap().len(), 2);
}

#[test]
fn a_signed_out_devices_empty_queue_is_not_offered() {
    let store = store();
    receive(&store, "mac", "Studio", &[], 1_000);
    assert!(
        store
            .resumable_queues(1_000, QueuePolicy::RESUME_WINDOW)
            .unwrap()
            .is_empty()
    );
}

// MARK: - Skipping redundant writes

#[test]
fn a_second_of_playback_is_not_worth_a_write() {
    let store = store();
    store
        .save_queue("iPhone", &tracks(&["a", "b"]), 0, 30_000, 1_000)
        .unwrap();
    let wrote = store
        .save_queue_if_changed("iPhone", &tracks(&["a", "b"]), 0, 31_000, 99_000)
        .unwrap();
    assert!(!wrote);
    let stored = local(&store);
    assert_eq!(stored.updated_at, 1_000);
    assert_eq!(stored.elapsed_ms, 30_000);
    assert_eq!(queue_ops(&store).len(), 1);
}

#[test]
fn elapsed_moving_past_the_threshold_is_worth_a_write() {
    let store = store();
    store
        .save_queue("iPhone", &tracks(&["a"]), 0, 30_000, 1_000)
        .unwrap();
    let wrote = store
        .save_queue_if_changed(
            "iPhone",
            &tracks(&["a"]),
            0,
            30_000 + QueuePolicy::ELAPSED_THRESHOLD_MS + 1,
            2_000,
        )
        .unwrap();
    assert!(wrote);
    assert_eq!(local(&store).updated_at, 2_000);
}

#[test]
fn seeking_backwards_is_a_change_even_when_the_jump_is_small() {
    let store = store();
    store
        .save_queue("iPhone", &tracks(&["a"]), 0, 60_000, 1_000)
        .unwrap();
    assert!(
        store
            .save_queue_if_changed("iPhone", &tracks(&["a"]), 0, 1_000, 2_000)
            .unwrap()
    );
    assert_eq!(local(&store).elapsed_ms, 1_000);
}

#[test]
fn a_track_change_is_always_worth_a_write() {
    let store = store();
    store
        .save_queue("iPhone", &tracks(&["a", "b"]), 0, 1_000, 1_000)
        .unwrap();
    assert!(
        store
            .save_queue_if_changed("iPhone", &tracks(&["a", "b"]), 1, 0, 2_000)
            .unwrap()
    );
    assert_eq!(local(&store).position, 1);
}

#[test]
fn editing_the_queue_is_worth_a_write_even_at_the_same_index_and_time() {
    let store = store();
    store
        .save_queue("iPhone", &tracks(&["a", "b"]), 0, 1_000, 1_000)
        .unwrap();
    assert!(
        store
            .save_queue_if_changed("iPhone", &tracks(&["a", "c"]), 0, 1_000, 2_000)
            .unwrap()
    );
    assert_eq!(local(&store).track_id_list(), ["a", "c"]);
}

#[test]
fn a_player_that_has_never_played_anything_writes_nothing_at_all() {
    let store = store();
    assert!(
        !store
            .save_queue_if_changed("iPhone", &[], 0, 0, 1_000)
            .unwrap()
    );
    assert_eq!(store.local_queue().unwrap(), None);
    assert!(pending(&store).is_empty());
}

/// Comparing before windowing would write a byte-identical row every time, forever.
#[test]
fn a_change_past_the_cap_is_not_a_change() {
    let store = store();
    let mut list = numbered(1_000);
    store.save_queue("iPhone", &list, 0, 0, 1_000).unwrap();
    list.swap(900, 901);
    assert!(
        !store
            .save_queue_if_changed("iPhone", &list, 0, 0, 2_000)
            .unwrap()
    );
    assert_eq!(local(&store).updated_at, 1_000);
}

// MARK: - The cap

#[test]
fn a_queue_under_the_cap_keeps_its_history() {
    let store = store();
    let list = numbered(100);
    store.save_queue("iPhone", &list, 90, 0, 1).unwrap();
    let restored = local(&store);
    assert_eq!(restored.track_id_list(), list);
    assert_eq!(restored.position, 90);
}

/// What has been heard is disposable; what is coming is not.
#[test]
fn the_cap_drops_played_history_and_keeps_everything_upcoming() {
    let store = store();
    let list = numbered(1_000);
    store.save_queue("iPhone", &list, 100, 0, 1).unwrap();
    let restored = local(&store);
    let stored = restored.track_id_list();
    let position = restored.position as usize;
    assert_eq!(stored.len(), QueuePolicy::MAX_TRACKS);
    assert_eq!(stored[position], "t100");
    assert_eq!(
        stored[position..],
        list[100..100 + QueuePolicy::MAX_TRACKS - QueuePolicy::MAX_HISTORY]
    );
    assert_eq!(stored[0], "t75");
}

#[test]
fn near_the_end_of_a_long_queue_nothing_upcoming_is_lost() {
    let store = store();
    let list = numbered(1_000);
    store.save_queue("iPhone", &list, 990, 0, 1).unwrap();
    let restored = local(&store);
    let stored = restored.track_id_list();
    assert_eq!(stored.len(), QueuePolicy::MAX_TRACKS);
    assert_eq!(stored[restored.position as usize..], list[990..]);
}

#[test]
fn a_position_past_the_end_of_the_queue_restores_to_the_last_track() {
    let store = store();
    store
        .save_queue("iPhone", &tracks(&["a", "b", "c"]), 99, 0, 1)
        .unwrap();
    assert_eq!(local(&store).position, 2);
}

#[test]
fn a_negative_elapsed_is_stored_as_the_start_of_the_track() {
    let store = store();
    store
        .save_queue("iPhone", &tracks(&["a"]), 0, -1, 1)
        .unwrap();
    assert_eq!(local(&store).elapsed_ms, 0);
}

// MARK: - Sign-out

#[test]
fn clearing_empties_the_queue_in_a_form_that_can_travel() {
    let store = store();
    store
        .save_queue("iPhone", &tracks(&["a", "b"]), 1, 5_000, 1_000)
        .unwrap();
    assert!(store.clear_queue(2_000).unwrap());

    let restored = local(&store);
    assert!(restored.track_id_list().is_empty());
    assert_eq!(restored.position, 0);
    assert_eq!(restored.elapsed_ms, 0);
    assert_eq!(restored.device_name, "iPhone");
    assert_eq!(restored.updated_at, 2_000);

    let ops = queue_ops(&store);
    assert_eq!(ops.len(), 1);
    assert!(ops[0].payload.contains("\"trackIds\":\"[]\""));
}

#[test]
fn clearing_a_device_that_never_played_anything_writes_nothing() {
    let store = store();
    assert!(!store.clear_queue(1_000).unwrap());
    assert!(pending(&store).is_empty());
}
