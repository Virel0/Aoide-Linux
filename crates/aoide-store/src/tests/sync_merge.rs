//! Port of `SyncMergeTests.swift` ("Applying pulled ops").

use serde_json::json;

use super::{
    Pulled, all, apply_one, apply_one_at, count, fetch, make_playlist, payload, pending,
    pulled_raw, store,
};
use crate::merge::{incoming_wins, is_credible, is_more_finished};
use crate::{
    Folder, Like, Millis, OpKind, PlayEvent, Playlist, PlaylistItem, PullCursor, QueueState, Store,
    SyncDirection, SyncEntity, SyncPolicy, now_millis,
};

fn stored_playlist(store: &Store) -> Option<Playlist> {
    fetch::<Playlist>(store, "list-1")
}

fn quarantined_reason(store: &Store) -> String {
    store
        .quarantined_ops(200)
        .unwrap()
        .first()
        .map(|q| q.reason.clone())
        .unwrap_or_default()
}

// MARK: - The invariant everything else depends on

#[test]
fn applying_inbound_ops_never_queues_an_outbound_one() {
    let store = store();
    let mine = store
        .create_playlist("Mine", None, None, now_millis())
        .unwrap();
    let before = store.pending_ops(1_000, &Default::default()).unwrap().len();

    let batch = vec![
        Pulled::new(
            SyncEntity::Playlist,
            &make_playlist("theirs", "Late Night", 9_000, false, "mac"),
            "theirs",
        )
        .seq(1)
        .op(),
        Pulled::new(
            SyncEntity::PlaylistItem,
            &PlaylistItem {
                id: "item-1".into(),
                ..PlaylistItem::new(&mine.id, "t1", "k1", "V", 9_000, "mac")
            },
            "item-1",
        )
        .seq(2)
        .op(),
        Pulled::new(
            SyncEntity::Folder,
            &Folder {
                id: "f1".into(),
                ..Folder::new("Mixes", "V", 9_000, "mac")
            },
            "f1",
        )
        .seq(3)
        .op(),
        Pulled::new(
            SyncEntity::Like,
            &Like {
                id: "like-1".into(),
                ..Like::new("t1", "k1", true, 9_000, "mac")
            },
            "like-1",
        )
        .seq(4)
        .op(),
        Pulled::new(
            SyncEntity::PlayEvent,
            &PlayEvent::new("t1", "k1", 9_000, "mac").with_id("e1"),
            "e1",
        )
        .seq(5)
        .op(),
        Pulled::new(
            SyncEntity::QueueState,
            &QueueState::new("mac", "Studio", &["t1"], 0, 0, 9_000),
            "mac",
        )
        .seq(6)
        .op(),
    ];

    let report = store
        .apply(&batch, PullCursor(6), SyncPolicy::default(), 10_000)
        .unwrap();
    assert_eq!(report.applied, 6);
    assert_eq!(
        store.pending_ops(1_000, &Default::default()).unwrap().len(),
        before
    );
}

#[test]
fn applying_the_same_op_twice_changes_nothing() {
    let store = store();
    let op = Pulled::new(
        SyncEntity::Playlist,
        &make_playlist("list-1", "Late Night", 5_000, false, "mac"),
        "list-1",
    )
    .op();
    assert_eq!(apply_one(&store, op.clone()).applied, 1);
    let second = apply_one(&store, op);
    assert_eq!(second.applied, 0);
    assert_eq!(second.ignored_stale, 1);
    assert_eq!(count::<Playlist>(&store), 1);
}

/// Our own ops go through the same merge path as everyone else's.
#[test]
fn our_own_op_coming_back_leaves_the_row_and_the_queue_alone() {
    let store = store();
    let saved = store
        .record(make_playlist("list-1", "Focus", 0, false, ""), false, 5_000)
        .unwrap();
    let pending_ops = pending(&store);

    let mine = Pulled::new(SyncEntity::Playlist, &saved, &saved.id)
        .op_id(&pending_ops[0].op_id)
        .created_at(5_000)
        .device("phone")
        .op();
    let report = apply_one(&store, mine);
    assert_eq!(report.applied, 0);
    assert_eq!(report.ignored_stale, 1);
    assert_eq!(stored_playlist(&store).unwrap().name, "Focus");
    // Still pending: applying it is not the server acknowledging it.
    assert_eq!(pending(&store).len(), 1);
}

#[test]
fn our_own_ops_restore_rows_we_no_longer_have() {
    let store = store();
    let op = Pulled::new(
        SyncEntity::Playlist,
        &make_playlist("list-1", "Late Night", 5_000, false, "phone"),
        "list-1",
    )
    .device("phone")
    .op();
    assert_eq!(apply_one(&store, op).applied, 1);
    assert_eq!(stored_playlist(&store).unwrap().name, "Late Night");
}

// MARK: - Last writer wins, per row

#[test]
fn a_newer_inbound_row_replaces_an_older_local_one() {
    let store = store();
    store
        .record(make_playlist("list-1", "Old", 0, false, ""), false, 1_000)
        .unwrap();
    let op = Pulled::new(
        SyncEntity::Playlist,
        &make_playlist("list-1", "New", 2_000, false, "mac"),
        "list-1",
    )
    .op();
    assert_eq!(apply_one(&store, op).applied, 1);
    assert_eq!(stored_playlist(&store).unwrap().name, "New");
}

#[test]
fn an_older_inbound_row_loses_and_writes_nothing() {
    let store = store();
    store
        .record(make_playlist("list-1", "New", 0, false, ""), false, 5_000)
        .unwrap();
    let op = Pulled::new(
        SyncEntity::Playlist,
        &make_playlist("list-1", "Old", 2_000, false, "mac"),
        "list-1",
    )
    .op();
    assert_eq!(apply_one(&store, op).ignored_stale, 1);
    assert_eq!(stored_playlist(&store).unwrap().name, "New");
}

#[test]
fn two_writes_in_the_same_millisecond_are_settled_by_the_device_id() {
    assert!(incoming_wins((5, "zed"), (5, "abe"), true));
    assert!(!incoming_wins((5, "abe"), (5, "zed"), true));
    // Identical in both fields is not "later", which is what makes replay safe.
    assert!(!incoming_wins((5, "abe"), (5, "abe"), true));

    let store = store();
    store
        .record(make_playlist("list-1", "Local", 0, false, ""), false, 3_000)
        .unwrap();
    let op = Pulled::new(
        SyncEntity::Playlist,
        &make_playlist("list-1", "Remote", 3_000, false, "zzz-mac"),
        "list-1",
    )
    .op();
    assert_eq!(apply_one(&store, op).applied, 1);
    assert_eq!(stored_playlist(&store).unwrap().name, "Remote");
}

// MARK: - Deletes

#[test]
fn an_inbound_delete_soft_deletes_the_local_row() {
    let store = store();
    store
        .record(
            make_playlist("list-1", "Late Night", 0, false, ""),
            false,
            1_000,
        )
        .unwrap();
    let op = Pulled::new(
        SyncEntity::Playlist,
        &make_playlist("list-1", "Late Night", 2_000, true, "mac"),
        "list-1",
    )
    .operation(OpKind::Delete)
    .op();
    assert_eq!(apply_one(&store, op).applied, 1);
    assert!(stored_playlist(&store).unwrap().deleted);
    assert_eq!(store.playlist("list-1").unwrap(), None);
}

/// A delete is a versioned write like any other.
#[test]
fn a_newer_inbound_edit_revives_a_row_deleted_locally() {
    let store = store();
    let list = store
        .record(
            make_playlist("list-1", "Late Night", 0, false, ""),
            false,
            1_000,
        )
        .unwrap();
    store.record(list, true, 2_000).unwrap();
    assert!(stored_playlist(&store).unwrap().deleted);

    let op = Pulled::new(
        SyncEntity::Playlist,
        &make_playlist("list-1", "Renamed", 3_000, false, "mac"),
        "list-1",
    )
    .op();
    assert_eq!(apply_one(&store, op).applied, 1);
    let stored = stored_playlist(&store).unwrap();
    assert!(!stored.deleted);
    assert_eq!(stored.name, "Renamed");
}

#[test]
fn an_inbound_delete_older_than_a_local_edit_loses() {
    let store = store();
    store
        .record(make_playlist("list-1", "Kept", 0, false, ""), false, 9_000)
        .unwrap();
    let op = Pulled::new(
        SyncEntity::Playlist,
        &make_playlist("list-1", "Late Night", 2_000, true, "mac"),
        "list-1",
    )
    .operation(OpKind::Delete)
    .op();
    assert_eq!(apply_one(&store, op).ignored_stale, 1);
    assert!(!stored_playlist(&store).unwrap().deleted);
}

#[test]
fn a_delete_whose_payload_does_not_say_so_is_quarantined() {
    let store = store();
    let op = Pulled::new(
        SyncEntity::Playlist,
        &make_playlist("list-1", "Late Night", 2_000, false, "mac"),
        "list-1",
    )
    .operation(OpKind::Delete)
    .op();
    assert_eq!(apply_one(&store, op).quarantined_inbound, 1);
    assert_eq!(stored_playlist(&store), None);
    assert!(quarantined_reason(&store).contains("deleted"));
}

/// The contract writes `deleted = 1`, after the SQLite column; quarantining that would
/// lose the deletion for good, since the cursor has moved past it.
#[test]
fn a_flag_spelled_as_1_applies_rather_than_being_quarantined() {
    let store = store();
    let mut fields = payload(&make_playlist("list-1", "Gone", 2_000, true, "mac"));
    fields["deleted"] = json!(1);
    fields["isSmart"] = json!(0);
    let op = pulled_raw(
        SyncEntity::Playlist.wire_name(),
        "list-1",
        fields,
        "delete",
        1,
    );

    let report = apply_one(&store, op);
    assert_eq!(report.applied, 1);
    assert_eq!(report.quarantined_inbound, 0);
    let stored = stored_playlist(&store).unwrap();
    assert!(stored.deleted);
    assert!(!stored.is_smart);
}

#[test]
fn a_field_that_is_not_a_flag_at_all_is_still_quarantined() {
    let store = store();
    let mut fields = payload(&make_playlist("list-1", "Late Night", 2_000, false, "mac"));
    fields["name"] = json!(7);
    let op = pulled_raw(
        SyncEntity::Playlist.wire_name(),
        "list-1",
        fields,
        "upsert",
        1,
    );
    assert_eq!(apply_one(&store, op).quarantined_inbound, 1);
    assert_eq!(stored_playlist(&store), None);
}

// MARK: - Playlist items

/// The parent is usually in a later batch; refusing the item would turn a wait into loss.
#[test]
fn an_item_for_a_playlist_we_have_never_seen_is_stored_and_counted() {
    let store = store();
    let item = PlaylistItem {
        id: "item-1".into(),
        ..PlaylistItem::new("nobody", "t1", "k1", "V", 1_000, "mac")
    };
    let report = apply_one(
        &store,
        Pulled::new(SyncEntity::PlaylistItem, &item, "item-1").op(),
    );
    assert_eq!(report.applied, 1);
    assert_eq!(report.orphaned_items, 1);
    assert_eq!(count::<PlaylistItem>(&store), 1);
    assert_eq!(store.items("nobody").unwrap().len(), 1);
}

#[test]
fn an_item_whose_playlist_is_already_here_is_not_an_orphan() {
    let store = store();
    let list = store
        .create_playlist("Host", None, None, now_millis())
        .unwrap();
    let item = PlaylistItem {
        id: "item-1".into(),
        ..PlaylistItem::new(&list.id, "t1", "k1", "V", 1_000, "mac")
    };
    let report = apply_one(
        &store,
        Pulled::new(SyncEntity::PlaylistItem, &item, "item-1").op(),
    );
    assert_eq!(report.applied, 1);
    assert_eq!(report.orphaned_items, 0);
    let ids: Vec<String> = store
        .items(&list.id)
        .unwrap()
        .into_iter()
        .map(|i| i.jellyfin_id)
        .collect();
    assert_eq!(ids, ["t1"]);
}

// MARK: - Likes

/// Two devices liking one track offline mint two rows, and the table is unique on the
/// track; inserting the second blindly would roll back the whole batch.
#[test]
fn a_duplicate_like_for_one_track_collapses_instead_of_failing_the_batch() {
    let store = store();
    store
        .record(
            Like {
                id: "like-local".into(),
                ..Like::new("t1", "k1", true, 0, "")
            },
            false,
            1_000,
        )
        .unwrap();
    let remote = Like {
        id: "like-remote".into(),
        ..Like::new("t1", "k1", false, 5_000, "mac")
    };
    let report = apply_one(
        &store,
        Pulled::new(SyncEntity::Like, &remote, "like-remote").op(),
    );
    assert_eq!(report.applied, 1);
    assert_eq!(report.superseded_likes, 1);

    let likes = all::<Like>(&store);
    assert_eq!(
        likes.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(),
        ["like-remote"]
    );
    assert!(!likes[0].liked);
}

#[test]
fn the_older_of_two_duplicate_likes_does_not_displace_the_newer() {
    let store = store();
    store
        .record(
            Like {
                id: "like-local".into(),
                ..Like::new("t1", "k1", true, 0, "")
            },
            false,
            9_000,
        )
        .unwrap();
    let remote = Like {
        id: "like-remote".into(),
        ..Like::new("t1", "k1", false, 1_000, "mac")
    };
    assert_eq!(
        apply_one(
            &store,
            Pulled::new(SyncEntity::Like, &remote, "like-remote").op()
        )
        .ignored_stale,
        1
    );
    assert_eq!(
        all::<Like>(&store)
            .iter()
            .map(|l| l.id.as_str())
            .collect::<Vec<_>>(),
        ["like-local"]
    );
}

// MARK: - Play events

fn event(started_at: Millis) -> PlayEvent {
    PlayEvent::new("t1", "k1", started_at, "mac").with_id("e1")
}

#[test]
fn a_play_event_is_inserted_once_and_never_again() {
    let store = store();
    let mut played = event(5_000);
    played.ms_played = 1_000;
    let op = Pulled::new(SyncEntity::PlayEvent, &played, "e1").op();
    assert_eq!(apply_one(&store, op.clone()).applied, 1);
    let second = apply_one(&store, op);
    assert_eq!(second.applied, 0);
    assert_eq!(second.ignored_duplicate, 1);
    assert_eq!(count::<PlayEvent>(&store), 1);
}

/// The one amendment history allows, and the one insert-if-absent would drop.
#[test]
fn a_finished_play_replaces_the_bare_start_it_was_opened_with() {
    let store = store();
    let started = event(5_000);
    apply_one(
        &store,
        Pulled::new(SyncEntity::PlayEvent, &started, "e1")
            .seq(1)
            .op(),
    );

    let finished = started.finished(Some(250_000), 245_000, true, false);
    let report = apply_one(
        &store,
        Pulled::new(SyncEntity::PlayEvent, &finished, "e1")
            .seq(2)
            .op(),
    );
    assert_eq!(report.applied, 1);
    let stored = fetch::<PlayEvent>(&store, "e1").unwrap();
    assert_eq!(stored.ms_played, 245_000);
    assert!(stored.completed);
    assert_eq!(count::<PlayEvent>(&store), 1);
}

/// A batch replayed from a rewound cursor delivers the bare start again.
#[test]
fn a_play_event_never_becomes_less_finished_than_it_already_is() {
    let store = store();
    let started = event(5_000);
    let finished = started
        .clone()
        .finished(Some(250_000), 245_000, true, false);
    apply_one(
        &store,
        Pulled::new(SyncEntity::PlayEvent, &finished, "e1")
            .seq(1)
            .op(),
    );
    let report = apply_one(
        &store,
        Pulled::new(SyncEntity::PlayEvent, &started, "e1")
            .seq(2)
            .op(),
    );
    assert_eq!(report.applied, 0);
    assert_eq!(report.ignored_duplicate, 1);
    assert_eq!(fetch::<PlayEvent>(&store, "e1").unwrap().ms_played, 245_000);
}

#[test]
fn outcome_ordering_is_by_when_the_listen_ended() {
    let started = event(5_000);
    let mut finished = started.clone();
    finished.ended_at = Some(250_000);
    let mut later = started.clone();
    later.ended_at = Some(260_000);

    assert!(is_more_finished(&finished, &started));
    assert!(!is_more_finished(&started, &finished));
    assert!(is_more_finished(&later, &finished));
    assert!(!is_more_finished(&finished, &later));
    assert!(!is_more_finished(&started, &started));
    assert!(!is_more_finished(&finished, &finished));
}

#[test]
fn a_delete_aimed_at_history_is_quarantined_rather_than_obeyed() {
    let store = store();
    let op = Pulled::new(SyncEntity::PlayEvent, &event(5_000), "e1")
        .operation(OpKind::Delete)
        .op();
    assert_eq!(apply_one(&store, op).quarantined_inbound, 1);
    assert_eq!(count::<PlayEvent>(&store), 0);
}

// MARK: - Queue state

#[test]
fn a_queue_row_is_keyed_by_its_device_not_by_a_uuid() {
    let store = store();
    let state = QueueState::new("mac", "Studio", &["a", "b"], 1, 500, 5_000);
    assert_eq!(
        apply_one(
            &store,
            Pulled::new(SyncEntity::QueueState, &state, "mac").op()
        )
        .applied,
        1
    );
    let resumable = store.resumable_queues(5_000, Millis::MAX).unwrap();
    assert_eq!(
        resumable
            .iter()
            .map(|q| q.device_id.as_str())
            .collect::<Vec<_>>(),
        ["mac"]
    );
    assert_eq!(resumable[0].track_id_list(), ["a", "b"]);
}

#[test]
fn an_older_queue_row_does_not_overwrite_a_newer_one() {
    let store = store();
    let newer = QueueState::new("mac", "Studio", &["new"], 0, 0, 9_000);
    apply_one(
        &store,
        Pulled::new(SyncEntity::QueueState, &newer, "mac")
            .seq(1)
            .op(),
    );
    let older = QueueState::new("mac", "Studio", &["old"], 0, 0, 1_000);
    assert_eq!(
        apply_one(
            &store,
            Pulled::new(SyncEntity::QueueState, &older, "mac")
                .seq(2)
                .op()
        )
        .ignored_stale,
        1
    );
    assert_eq!(
        fetch::<QueueState>(&store, "mac").unwrap().track_id_list(),
        ["new"]
    );
}

#[test]
fn a_queue_row_with_a_mismatched_device_is_quarantined() {
    let store = store();
    let state = QueueState::new("mac", "Studio", &["a"], 0, 0, 1_000);
    assert_eq!(
        apply_one(
            &store,
            Pulled::new(SyncEntity::QueueState, &state, "ipad").op()
        )
        .quarantined_inbound,
        1
    );
    assert_eq!(count::<QueueState>(&store), 0);
}

// MARK: - Ops this build cannot apply

#[test]
fn an_unknown_entity_is_quarantined_not_crashed_on() {
    let store = store();
    let report = apply_one(
        &store,
        pulled_raw("downloads", "d1", json!({"id": "d1"}), "upsert", 1),
    );
    assert_eq!(report.quarantined_inbound, 1);
    assert!(quarantined_reason(&store).contains("downloads"));
}

/// The payload is the only copy: the cursor has moved past it.
#[test]
fn a_quarantined_inbound_op_keeps_its_payload_and_its_direction() {
    let store = store();
    apply_one(
        &store,
        pulled_raw(
            "downloads",
            "d1",
            json!({"id": "d1", "size": 42}),
            "upsert",
            1,
        ),
    );
    let quarantined = store.quarantined_ops(200).unwrap().remove(0);
    assert_eq!(quarantined.sync_direction(), Some(SyncDirection::Inbound));
    assert_eq!(quarantined.entity, "downloads");
    assert!(quarantined.payload.contains("\"size\":42"));
}

#[test]
fn a_payload_that_does_not_decode_into_the_local_row_is_quarantined() {
    let store = store();
    // `name` is not optional, and no default can honestly stand in for it.
    let fields = json!({"id": "list-1", "updatedAt": 1, "deleted": false, "originDevice": "mac", "sortIndex": "V"});
    assert_eq!(
        apply_one(
            &store,
            pulled_raw(
                SyncEntity::Playlist.wire_name(),
                "list-1",
                fields,
                "upsert",
                1
            )
        )
        .quarantined_inbound,
        1
    );
    assert_eq!(stored_playlist(&store), None);
}

/// Unknown keys are ignored, so an added column decodes and is simply absent here.
#[test]
fn a_newer_clients_extra_column_applies_rather_than_quarantining() {
    let store = store();
    let mut fields = payload(&make_playlist("list-1", "Late Night", 5_000, false, "mac"));
    fields["colourTheme"] = json!("dusk");
    assert_eq!(
        apply_one(
            &store,
            pulled_raw(
                SyncEntity::Playlist.wire_name(),
                "list-1",
                fields,
                "upsert",
                1
            )
        )
        .applied,
        1
    );
    assert_eq!(stored_playlist(&store).unwrap().name, "Late Night");
}

#[test]
fn an_op_that_disagrees_with_its_own_payload_is_quarantined() {
    let store = store();
    let op = Pulled::new(
        SyncEntity::Playlist,
        &make_playlist("list-1", "Late Night", 1_000, false, "mac"),
        "list-2",
    )
    .op();
    assert_eq!(apply_one(&store, op).quarantined_inbound, 1);
    assert_eq!(count::<Playlist>(&store), 0);
}

#[test]
fn a_payload_that_is_not_an_object_is_quarantined() {
    let store = store();
    let op = pulled_raw(
        SyncEntity::Playlist.wire_name(),
        "list-1",
        json!(["nope"]),
        "upsert",
        1,
    );
    assert_eq!(apply_one(&store, op).quarantined_inbound, 1);
}

#[test]
fn an_operation_the_server_would_not_have_accepted_is_quarantined() {
    let store = store();
    let fields = payload(&make_playlist("list-1", "Late Night", 1, false, "mac"));
    let op = pulled_raw(
        SyncEntity::Playlist.wire_name(),
        "list-1",
        fields,
        "UPSERT",
        1,
    );
    assert_eq!(apply_one(&store, op).quarantined_inbound, 1);
}

/// One bad op must not abandon the batch around it.
#[test]
fn a_bad_op_is_quarantined_and_the_rest_of_the_batch_still_lands() {
    let store = store();
    let batch = vec![
        Pulled::new(
            SyncEntity::Playlist,
            &make_playlist("a", "Late Night", 1_000, false, "mac"),
            "a",
        )
        .seq(1)
        .op(),
        pulled_raw("downloads", "d1", json!({"id": "d1"}), "upsert", 2),
        Pulled::new(
            SyncEntity::Playlist,
            &make_playlist("b", "Late Night", 1_000, false, "mac"),
            "b",
        )
        .seq(3)
        .op(),
    ];
    let report = store
        .apply(&batch, PullCursor(3), SyncPolicy::default(), 5_000)
        .unwrap();
    assert_eq!(report.applied, 2);
    assert_eq!(report.quarantined_inbound, 1);
    assert_eq!(count::<Playlist>(&store), 2);
    assert_eq!(store.pull_cursor().unwrap(), Some(PullCursor(3)));
}

// MARK: - Clock skew

/// A device whose clock says 2099 would win every conflict it ever entered.
#[test]
fn a_timestamp_impossibly_far_ahead_of_the_server_is_pulled_back() {
    let store = store();
    store
        .record(
            make_playlist("list-1", "Local", 0, false, "aaa"),
            false,
            1_000_000,
        )
        .unwrap();
    let broken = make_playlist("list-1", "Broken clock", 4_000_000_000_000, false, "mac");
    let op = Pulled::new(SyncEntity::Playlist, &broken, "list-1")
        .created_at(4_000_000_000_000)
        .received_at(500_000)
        .op();
    // No earlier than the device's own last write; see the Swift suite for why.
    let report = apply_one_at(&store, op, 1_000_000);
    assert_eq!(report.clock_skewed, 1);
    // Corrected to the receipt time, older than the local row, so it does not win.
    assert_eq!(report.ignored_stale, 1);
    assert_eq!(stored_playlist(&store).unwrap().name, "Local");
}

#[test]
fn a_corrected_row_is_stored_with_the_corrected_time_not_the_claimed_one() {
    let store = store();
    let broken = make_playlist("list-1", "Late Night", 4_000_000_000_000, false, "mac");
    let op = Pulled::new(SyncEntity::Playlist, &broken, "list-1")
        .created_at(4_000_000_000_000)
        .received_at(500_000)
        .op();
    assert_eq!(apply_one(&store, op).applied, 1);
    assert_eq!(stored_playlist(&store).unwrap().updated_at, 500_000);
}

/// An op written offline and pushed days later is normal.
#[test]
fn an_old_timestamp_is_never_corrected() {
    let store = store();
    let old = make_playlist("list-1", "Late Night", 1_000, false, "mac");
    let op = Pulled::new(SyncEntity::Playlist, &old, "list-1")
        .created_at(1_000)
        .received_at(900_000_000)
        .op();
    assert_eq!(apply_one(&store, op).clock_skewed, 0);
    assert_eq!(stored_playlist(&store).unwrap().updated_at, 1_000);
}

#[test]
fn ordinary_drift_inside_the_tolerance_is_not_treated_as_a_broken_clock() {
    let tolerance = SyncPolicy::default().clock_skew_tolerance_ms;
    let credible = |timestamp: Millis, received_at: Millis, now: Millis| {
        is_credible(timestamp, received_at, now, tolerance)
    };
    assert!(credible(1_000 + tolerance, 1_000, 1_000));
    assert!(!credible(1_001 + tolerance, 1_000, 1_000));
    // Nothing overflows near the end of i64.
    assert!(credible(Millis::MAX, Millis::MAX, Millis::MAX));
}

/// Ahead of one reference is ordinary; ahead of both is a broken clock.
#[test]
fn a_timestamp_ahead_of_only_one_reference_is_still_believed() {
    let tolerance = SyncPolicy::default().clock_skew_tolerance_ms;
    let far: Millis = 10_000_000;
    assert!(is_credible(far, far - 3_600_000, far, tolerance));
    assert!(!is_credible(far + 3_600_000, far, far, tolerance));
}

/// A local row stamped by a clock since corrected has no order to compute against, so
/// it loses to a copy the server witnessed.
#[test]
fn a_local_row_this_device_can_no_longer_date_loses_to_an_inbound_one() {
    let store = store();
    store
        .record(
            make_playlist("list-1", "Stamped in 2096", 0, false, "phone"),
            false,
            4_000_000_000_000,
        )
        .unwrap();
    let op = Pulled::new(
        SyncEntity::Playlist,
        &make_playlist("list-1", "Corrected", 1_000_000, false, "mac"),
        "list-1",
    )
    .received_at(1_100_000)
    .op();
    let report = apply_one_at(&store, op, 1_200_000);
    assert_eq!(report.applied, 1);
    assert_eq!(report.clock_skewed, 0);
    let stored = stored_playlist(&store).unwrap();
    assert_eq!(stored.name, "Corrected");
    assert_eq!(stored.updated_at, 1_000_000);
}

/// While the clock is still wrong, the device keeps its own row.
#[test]
fn a_device_whose_clock_is_still_wrong_keeps_its_own_row() {
    let store = store();
    store
        .record(
            make_playlist("list-1", "Stamped in 2096", 0, false, "phone"),
            false,
            4_000_000_000_000,
        )
        .unwrap();
    let op = Pulled::new(
        SyncEntity::Playlist,
        &make_playlist("list-1", "Corrected", 1_000_000, false, "mac"),
        "list-1",
    )
    .received_at(1_100_000)
    .op();
    let report = apply_one_at(&store, op, 4_000_000_000_000);
    assert_eq!(report.ignored_stale, 1);
    assert_eq!(stored_playlist(&store).unwrap().name, "Stamped in 2096");
}

/// A play event wins no conflicts; `startedAt` is not ours to rewrite.
#[test]
fn a_skewed_play_event_is_counted_but_not_rewritten() {
    let store = store();
    let skewed = PlayEvent::new("t1", "k1", 4_000_000_000_000, "mac").with_id("e1");
    let op = Pulled::new(SyncEntity::PlayEvent, &skewed, "e1")
        .created_at(4_000_000_000_000)
        .received_at(500_000)
        .op();
    let report = apply_one(&store, op);
    assert_eq!(report.clock_skewed, 1);
    assert_eq!(report.applied, 1);
    assert_eq!(
        fetch::<PlayEvent>(&store, "e1").unwrap().started_at,
        4_000_000_000_000
    );
}

// MARK: - The cursor

#[test]
fn the_cursor_advances_with_the_batch_it_describes() {
    let store = store();
    assert_eq!(store.pull_cursor().unwrap(), None);
    let op = Pulled::new(
        SyncEntity::Playlist,
        &make_playlist("list-1", "Late Night", 1_000, false, "mac"),
        "list-1",
    )
    .seq(77)
    .op();
    let report = store
        .apply(&[op], PullCursor(77), SyncPolicy::default(), 5_000)
        .unwrap();
    assert_eq!(report.cursor, Some(PullCursor(77)));
    assert_eq!(store.pull_cursor().unwrap(), Some(PullCursor(77)));
}

// MARK: - Not in the Swift suite: the per-field path through the applier

/// The applier, not just `merged`, keeps a row-level loser's newer field. Pinned
/// because the Swift suite exercises this only through two whole devices.
#[test]
fn a_stale_inbound_row_still_contributes_the_field_it_changed_last() {
    let store = store();
    let mut local = make_playlist("list-1", "Original", 200, false, "phone");
    local.notes = Some("described here".into());
    local.field_updated_at = Some(crate::FieldStamps::from_pairs([
        ("name", 50),
        ("notes", 200),
    ]));
    store
        .write_local(|tx| crate::Record::save(&local, tx))
        .unwrap();

    let mut remote = make_playlist("list-1", "Renamed there", 100, false, "mac");
    remote.notes = Some("old".into());
    remote.field_updated_at = Some(crate::FieldStamps::from_pairs([
        ("name", 100),
        ("notes", 10),
    ]));
    let report = apply_one(
        &store,
        Pulled::new(SyncEntity::Playlist, &remote, "list-1").op(),
    );

    assert_eq!(report.applied, 1);
    let stored = stored_playlist(&store).unwrap();
    assert_eq!(stored.name, "Renamed there");
    assert_eq!(stored.notes.as_deref(), Some("described here"));
    // The row-level winner supplied the metadata.
    assert_eq!(stored.updated_at, 200);
    assert_eq!(stored.origin_device, "phone");
}
