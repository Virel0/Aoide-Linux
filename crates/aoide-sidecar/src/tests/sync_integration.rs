//! Port of `SyncIntegrationTests.swift`: "Sync against a faithful sidecar", "A server
//! that throws on one op", "Retrying after a server is fixed" and "A stall that
//! explains itself". Everything goes through `SyncEngine::sync` and the stub, because
//! these failures only exist between the parts.

use std::collections::HashSet;
use std::sync::Arc;

use aoide_store::{
    ContentKey, Like, Millis, Op, PlayEvent, PlaySource, Playlist, PullCursor, PushCursor,
    PushRequest, Record, SmartRule, SmartRules, Store, SyncDirection, SyncEntity, SyncFault,
    SyncPolicy, TrackLink, WireOp, now_millis,
};
use serde_json::{Value, json};

use super::sidecar_stub::{Sidecar, SidecarLink, refusal};
use super::{
    count, forget_acknowledgements, item_ids, make_playlist, names, payload, pending, pending_all,
    store, synced_rows, total_op_count,
};
use crate::{SidecarError, SyncEngine, SyncError, SyncTransport};

const DEFAULT_CLOCK: Millis = 2_000_000_000_000;

fn engine(store: &Arc<Store>, link: SidecarLink) -> SyncEngine<SidecarLink> {
    engine_at(store, link, SyncPolicy::default(), DEFAULT_CLOCK)
}

fn engine_with(
    store: &Arc<Store>,
    link: SidecarLink,
    policy: SyncPolicy,
) -> SyncEngine<SidecarLink> {
    engine_at(store, link, policy, DEFAULT_CLOCK)
}

/// `clock` is *this device's* clock: a phone whose writes say 2096 has an engine that
/// reads 2096 too.
fn engine_at(
    store: &Arc<Store>,
    link: SidecarLink,
    policy: SyncPolicy,
    clock: Millis,
) -> SyncEngine<SidecarLink> {
    SyncEngine::new(Arc::clone(store), link)
        .with_policy(policy)
        .with_clock(move || clock)
}

fn is_unreachable(result: &Result<aoide_store::SyncReport, SyncError>) -> bool {
    matches!(result, Err(SyncError::Transport(SidecarError::Network(message))) if message == "unreachable")
}

/// Puts a device's pending ops on the server without pulling anything back.
async fn push_only(store: &Store, link: &SidecarLink) {
    let mut queued = pending_all(store);
    while !queued.is_empty() {
        let batch: Vec<Op> = queued.drain(..queued.len().min(1_000)).collect();
        let wire = batch
            .iter()
            .filter_map(|op| WireOp::make(op).ok())
            .collect();
        let response = link
            .push(&PushRequest {
                device_id: store.device_id().to_owned(),
                ops: wire,
            })
            .await
            .unwrap();
        store.mark_synced(&response.accepted).unwrap();
    }
}

struct Library {
    list: String,
    focus: String,
}

/// Something of every syncable kind, at fixed times.
fn build_library(store: &Store, device: &str) -> Library {
    let folder = store.create_folder("Mixes", None, 1_000).unwrap();
    let late = store
        .create_playlist("Late Night", Some(&folder.id), Some("for the drive"), 1_100)
        .unwrap();
    let links: Vec<TrackLink> = (0..4)
        .map(|i| TrackLink::new(format!("t{i}"), format!("k{i}")))
        .collect();
    store.add_tracks(&links, &late.id, 1_200).unwrap();
    let focus = store.create_playlist("Focus", None, None, 1_300).unwrap();
    store
        .add_tracks(&[TrackLink::new("t9", "k9")], &focus.id, 1_400)
        .unwrap();
    store
        .record(Like::new("t1", "k1", true, 0, device), false, 1_500)
        .unwrap();
    for index in 0..3 {
        store
            .append(
                PlayEvent::new(
                    format!("t{index}"),
                    format!("k{index}"),
                    1_600 + index,
                    device,
                )
                .with_id(format!("{device}-e{index}"))
                .finished(None, 200_000, true, false),
            )
            .unwrap();
    }
    let queue: Vec<String> = ["t0", "t1", "t2"].map(str::to_owned).to_vec();
    store
        .save_queue(&format!("The {device}"), &queue, 1, 4_000, 1_700)
        .unwrap();
    // A soft delete and an edit, so deleted rows and second versions travel too.
    let second = store.items(&late.id).unwrap()[1].id.clone();
    store.remove_item(&second, 1_800).unwrap();
    store
        .rename_playlist(&focus.id, "Deep Focus", 1_900)
        .unwrap();
    Library {
        list: late.id,
        focus: focus.id,
    }
}

fn wire(op_id: &str, entity: &str, entity_id: &str, payload: Value, created_at: Millis) -> WireOp {
    WireOp {
        op_id: op_id.to_owned(),
        entity: entity.to_owned(),
        entity_id: entity_id.to_owned(),
        operation: "upsert".to_owned(),
        payload,
        created_at,
    }
}

fn playlist_name(store: &Store, id: &str) -> Option<String> {
    store.playlist(id).unwrap().map(|p| p.name)
}

// MARK: - 1. Round trip

/// Every column of every syncable table, deleted rows included.
#[tokio::test]
async fn a_library_made_on_one_device_arrives_on_the_other_byte_for_byte() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    let ids = build_library(&phone, "phone");

    let queued = pending_all(&phone).len();
    let sent = engine(&phone, sidecar.link("listener"))
        .sync()
        .await
        .unwrap();
    assert_eq!(sent.pushed, queued);
    assert_eq!(sent.quarantined_outbound, 0);
    assert_eq!(sent.rejected, 0);

    let received = engine(&mac, sidecar.link("listener")).sync().await.unwrap();
    assert_eq!(received.pulled, queued);
    assert_eq!(received.applied, queued);
    assert_eq!(received.quarantined_inbound, 0);
    assert_eq!(synced_rows(&mac), synced_rows(&phone));

    assert_eq!(names(&mac), ["Late Night", "Deep Focus"]);
    assert_eq!(item_ids(&mac, &ids.list), ["t0", "t2", "t3"]);
    assert_eq!(
        mac.folders()
            .unwrap()
            .iter()
            .map(|f| f.name.as_str())
            .collect::<Vec<_>>(),
        ["Mixes"]
    );
    assert_eq!(count::<PlayEvent>(&mac), 3);
    let queues = mac.resumable_queues(2_000, Millis::MAX).unwrap();
    assert_eq!(
        queues
            .iter()
            .map(|q| q.device_name.as_str())
            .collect::<Vec<_>>(),
        ["The phone"]
    );
}

#[tokio::test]
async fn edits_made_on_the_second_device_come_back_to_the_first() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    let (phone_sync, mac_sync) = (
        engine(&phone, sidecar.link("listener")),
        engine(&mac, sidecar.link("listener")),
    );
    let ids = build_library(&phone, "phone");
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();

    mac.rename_playlist(&ids.list, "Very Late Night", 5_000)
        .unwrap();
    mac.add_tracks(&[TrackLink::new("t7", "k7")], &ids.list, 5_100)
        .unwrap();
    mac.delete_playlist(&ids.focus, 5_200).unwrap();
    mac_sync.sync().await.unwrap();
    phone_sync.sync().await.unwrap();

    assert_eq!(
        playlist_name(&phone, &ids.list).as_deref(),
        Some("Very Late Night")
    );
    assert_eq!(item_ids(&phone, &ids.list), ["t0", "t2", "t3", "t7"]);
    assert_eq!(phone.playlist(&ids.focus).unwrap(), None);
    assert_eq!(synced_rows(&mac), synced_rows(&phone));
    assert!(pending(&phone).is_empty());
    assert!(pending(&mac).is_empty());
}

/// Three devices is where an order-dependent merge shows up.
#[tokio::test]
async fn three_devices_editing_in_turn_all_end_up_holding_the_same_rows() {
    let sidecar = Sidecar::new();
    let (phone, mac, pad) = (store("phone"), store("mac"), store("pad"));
    let syncs = [
        engine(&phone, sidecar.link("listener")),
        engine(&mac, sidecar.link("listener")),
        engine(&pad, sidecar.link("listener")),
    ];
    let ids = build_library(&phone, "phone");
    for sync in &syncs {
        sync.sync().await.unwrap();
    }

    mac.rename_playlist(&ids.list, "From the mac", 6_000)
        .unwrap();
    mac.record(Like::new("t3", "k3", true, 0, "mac"), false, 6_100)
        .unwrap();
    pad.add_tracks(&[TrackLink::new("t5", "k5")], &ids.list, 6_200)
        .unwrap();
    phone.delete_playlist(&ids.focus, 6_300).unwrap();

    for _ in 0..2 {
        for sync in &syncs {
            sync.sync().await.unwrap();
        }
    }
    assert_eq!(synced_rows(&phone), synced_rows(&mac));
    assert_eq!(synced_rows(&mac), synced_rows(&pad));
    assert_eq!(
        playlist_name(&pad, &ids.list).as_deref(),
        Some("From the mac")
    );
    assert_eq!(item_ids(&phone, &ids.list), ["t0", "t2", "t3", "t5"]);
}

#[tokio::test]
async fn one_users_ops_are_invisible_to_another() {
    let sidecar = Sidecar::new();
    let mine = store("phone");
    let theirs = store("their-phone");
    build_library(&mine, "phone");
    engine(&mine, sidecar.link("listener"))
        .sync()
        .await
        .unwrap();

    let report = engine(&theirs, sidecar.link("someone-else"))
        .sync()
        .await
        .unwrap();
    assert_eq!(report.pulled, 0);
    assert!(theirs.playlists().unwrap().is_empty());
    assert_eq!(theirs.pull_cursor().unwrap(), None);
}

#[tokio::test]
async fn a_sync_pushes_before_it_pulls() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    phone.create_playlist("Focus", None, None, 1_000).unwrap();
    engine(&phone, sidecar.link("listener"))
        .sync()
        .await
        .unwrap();
    assert_eq!(sidecar.calls(), ["push", "pull"]);
}

/// The invariants nothing on the server checks, asserted on the traffic that left.
#[tokio::test]
async fn every_op_that_leaves_agrees_with_its_own_payload() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    build_library(&phone, "phone");
    engine(&phone, sidecar.link("listener"))
        .sync()
        .await
        .unwrap();

    let sent = sidecar.ops_for("listener");
    assert_eq!(
        sent.iter()
            .map(|row| row.op.entity.as_str())
            .collect::<HashSet<_>>()
            .len(),
        6
    );
    for row in sent {
        let fields = row.op.payload.as_object().expect("payload object");
        let identity = if row.op.entity == "queue_state" {
            "deviceId"
        } else {
            "id"
        };
        assert_eq!(
            fields[identity].as_str(),
            Some(row.op.entity_id.as_str()),
            "{} identity",
            row.op.entity
        );
        assert_eq!(
            fields["originDevice"].as_str(),
            Some(row.device_id.as_str()),
            "{} origin",
            row.op.entity
        );
        if row.op.operation == "delete" {
            assert_eq!(
                fields["deleted"],
                json!(true),
                "{} delete flag",
                row.op.entity
            );
        }
    }
}

#[tokio::test]
async fn work_done_on_both_devices_while_offline_all_arrives() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    let (phone_sync, mac_sync) = (
        engine(&phone, sidecar.link("listener")),
        engine(&mac, sidecar.link("listener")),
    );
    for index in 0..3 {
        phone
            .create_playlist(&format!("Phone {index}"), None, None, 1_000 + index)
            .unwrap();
        mac.create_playlist(&format!("Mac {index}"), None, None, 1_000 + index)
            .unwrap();
        phone
            .append(
                PlayEvent::new(
                    format!("t{index}"),
                    format!("k{index}"),
                    2_000 + index,
                    "phone",
                )
                .with_id(format!("phone-e{index}"))
                .finished(None, 250_000, true, false),
            )
            .unwrap();
        mac.append(
            PlayEvent::new(
                format!("t{index}"),
                format!("k{index}"),
                3_000 + index,
                "mac",
            )
            .with_id(format!("mac-e{index}"))
            .finished(None, 250_000, true, false),
        )
        .unwrap();
    }
    for _ in 0..2 {
        phone_sync.sync().await.unwrap();
        mac_sync.sync().await.unwrap();
    }
    assert_eq!(phone.playlists().unwrap().len(), 6);
    assert_eq!(mac.playlists().unwrap().len(), 6);
    assert_eq!(count::<PlayEvent>(&phone), 6);
    assert_eq!(synced_rows(&phone), synced_rows(&mac));
}

/// A reinstall keeps the device id and loses the database.
#[tokio::test]
async fn a_reinstalled_device_rebuilds_itself_from_its_own_ops() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    build_library(&phone, "phone");
    engine(&phone, sidecar.link("listener"))
        .sync()
        .await
        .unwrap();

    let reinstalled = store("phone");
    let report = engine(&reinstalled, sidecar.link("listener"))
        .sync()
        .await
        .unwrap();
    assert!(report.applied > 0);
    assert_eq!(synced_rows(&reinstalled), synced_rows(&phone));
    assert_eq!(
        reinstalled.local_queue().unwrap().unwrap().track_id_list(),
        ["t0", "t1", "t2"]
    );
    // Restoring is not editing: it owes the server nothing.
    assert_eq!(total_op_count(&reinstalled), 0);
}

// MARK: - 2. Idempotency

#[tokio::test]
async fn pushing_the_same_ops_twice_adds_nothing_to_the_server() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    build_library(&phone, "phone");
    let sync = engine(&phone, sidecar.link("listener"));
    let first = sync.sync().await.unwrap();
    let stored = sidecar.op_count("listener");
    assert_eq!(stored, first.pushed);

    forget_acknowledgements(&phone);
    let second = sync.sync().await.unwrap();
    assert_eq!(second.pushed, first.pushed);
    assert_eq!(second.rejected, 0);
    assert_eq!(sidecar.op_count("listener"), stored);
    assert!(pending(&phone).is_empty());
    assert_eq!(names(&phone), ["Late Night", "Deep Focus"]);
}

#[tokio::test]
async fn pulling_the_same_batch_twice_changes_nothing() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    build_library(&phone, "phone");
    engine(&phone, sidecar.link("listener"))
        .sync()
        .await
        .unwrap();

    let mac_sync = engine(&mac, sidecar.link("listener"));
    let first = mac_sync.sync().await.unwrap();
    let after = synced_rows(&mac);
    assert!(first.applied > 0);

    mac.set_sync_cursor("0").unwrap();
    let second = mac_sync.sync().await.unwrap();
    assert_eq!(second.pulled, first.pulled);
    assert_eq!(second.applied, 0);
    assert_eq!(
        second.ignored_stale + second.ignored_duplicate,
        second.pulled
    );
    assert_eq!(synced_rows(&mac), after);
    assert_eq!(total_op_count(&mac), 0);
}

#[tokio::test]
async fn the_same_batch_applied_twice_by_hand_is_a_no_op_the_second_time() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    build_library(&phone, "phone");
    let link = sidecar.link("listener");
    push_only(&phone, &link).await;

    let batch = link.pull(None, 1_000).await.unwrap();
    let first = mac
        .apply(&batch.ops, batch.cursor, SyncPolicy::default(), 2_000)
        .unwrap();
    let second = mac
        .apply(&batch.ops, batch.cursor, SyncPolicy::default(), 2_000)
        .unwrap();
    assert_eq!(first.applied, batch.ops.len());
    assert_eq!(second.applied, 0);
    assert_eq!(synced_rows(&mac), synced_rows(&phone));
    assert_eq!(mac.pull_cursor().unwrap(), Some(batch.cursor));
}

// MARK: - 3. Interruption

#[tokio::test]
async fn a_pull_that_dies_midway_leaves_the_cursor_on_the_last_batch_it_applied() {
    let sidecar = Sidecar::new();
    let mac = store("mac");
    let phone = store("phone");
    let link = sidecar.link("listener");
    for index in 0..12 {
        mac.create_playlist(&format!("List {index}"), None, None, 1_000 + index)
            .unwrap();
    }
    push_only(&mac, &link).await;

    sidecar.fail_pull(2);
    let sync = engine_with(&phone, link, SyncPolicy::default().with_pull_limit(5));
    assert!(is_unreachable(&sync.sync().await));
    assert_eq!(phone.pull_cursor().unwrap(), Some(PullCursor(5)));
    assert_eq!(phone.playlists().unwrap().len(), 5);

    let resumed = sync.sync().await.unwrap();
    assert_eq!(resumed.applied, 7);
    assert_eq!(phone.playlists().unwrap().len(), 12);
    assert_eq!(phone.pull_cursor().unwrap(), Some(PullCursor(12)));
    assert_eq!(synced_rows(&phone), synced_rows(&mac));
}

#[tokio::test]
async fn repeated_interruptions_still_converge_losing_and_duplicating_nothing() {
    let sidecar = Sidecar::new();
    let mac = store("mac");
    let phone = store("phone");
    let link = sidecar.link("listener");
    for index in 0..20 {
        mac.create_playlist(&format!("List {index}"), None, None, 1_000 + index)
            .unwrap();
    }
    push_only(&mac, &link).await;

    sidecar.fail_pull(2);
    sidecar.fail_pull(4);
    let sync = engine_with(&phone, link, SyncPolicy::default().with_pull_limit(4));
    for _ in 0..3 {
        let _ = sync.sync().await;
    }
    assert_eq!(phone.playlists().unwrap().len(), 20);
    assert_eq!(names(&phone).into_iter().collect::<HashSet<_>>().len(), 20);
    assert_eq!(synced_rows(&phone), synced_rows(&mac));
    assert_eq!(total_op_count(&phone), 0);
    // Five pages would otherwise have taken five calls, not seven.
    assert_eq!(sidecar.pull_count(), 7);
    assert_eq!(sync.sync().await.unwrap().pulled, 0);
}

#[tokio::test]
async fn a_push_that_dies_is_simply_sent_again() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    build_library(&phone, "phone");
    let queued = pending_all(&phone).len();
    sidecar.fail_push(1);
    let sync = engine(&phone, sidecar.link("listener"));
    assert!(is_unreachable(&sync.sync().await));
    assert_eq!(pending_all(&phone).len(), queued);
    assert_eq!(sidecar.op_count("listener"), 0);

    let report = sync.sync().await.unwrap();
    assert_eq!(report.pushed, queued);
    assert_eq!(sidecar.op_count("listener"), queued);
}

// MARK: - 4. Ping-pong

#[tokio::test]
async fn applying_another_devices_ops_queues_nothing_to_push_back() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    build_library(&phone, "phone");
    engine(&phone, sidecar.link("listener"))
        .sync()
        .await
        .unwrap();
    let phone_ops = total_op_count(&phone);
    let mac_sync = engine(&mac, sidecar.link("listener"));
    mac_sync.sync().await.unwrap();
    assert_eq!(total_op_count(&mac), 0);
    assert!(pending(&mac).is_empty());

    let logged = sidecar.op_count("listener");
    for _ in 0..3 {
        mac_sync.sync().await.unwrap();
    }
    assert_eq!(total_op_count(&mac), 0);
    assert_eq!(sidecar.op_count("listener"), logged);
    assert_eq!(sidecar.pushes_by("mac"), None);
    engine(&phone, sidecar.link("listener"))
        .sync()
        .await
        .unwrap();
    assert_eq!(total_op_count(&phone), phone_ops);
}

#[tokio::test]
async fn the_op_count_is_a_function_of_local_edits_not_of_how_often_you_sync() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    let (phone_sync, mac_sync) = (
        engine(&phone, sidecar.link("listener")),
        engine(&mac, sidecar.link("listener")),
    );
    build_library(&phone, "phone");
    let phone_edits = total_op_count(&phone);
    for _ in 0..3 {
        phone_sync.sync().await.unwrap();
        mac_sync.sync().await.unwrap();
    }
    assert_eq!(total_op_count(&phone), phone_edits);
    assert_eq!(total_op_count(&mac), 0);

    mac.create_playlist("Mac's own", None, None, 7_000).unwrap();
    for _ in 0..3 {
        mac_sync.sync().await.unwrap();
        phone_sync.sync().await.unwrap();
    }
    assert_eq!(total_op_count(&mac), 1);
    assert_eq!(total_op_count(&phone), phone_edits);
    assert_eq!(sidecar.op_count("listener") as i64, phone_edits + 1);
}

// MARK: - 5. Conflict

#[tokio::test]
async fn two_devices_editing_one_playlist_offline_pick_the_same_winner() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    let (phone_sync, mac_sync) = (
        engine(&phone, sidecar.link("listener")),
        engine(&mac, sidecar.link("listener")),
    );
    let list = phone.create_playlist("Shared", None, None, 1_000).unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();

    phone
        .rename_playlist(&list.id, "From the phone", 5_000)
        .unwrap();
    mac.rename_playlist(&list.id, "From the mac", 6_000)
        .unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();
    phone_sync.sync().await.unwrap();

    let on_phone = phone.playlist(&list.id).unwrap().unwrap();
    assert_eq!(on_phone.name, "From the mac");
    assert_eq!(
        playlist_name(&mac, &list.id).as_deref(),
        Some("From the mac")
    );
    assert_eq!(on_phone.updated_at, 6_000);
    assert_eq!(on_phone.origin_device, "mac");
    assert_eq!(synced_rows(&phone), synced_rows(&mac));
}

/// Same millisecond: the larger device id wins, the same way on both.
#[tokio::test]
async fn a_tie_is_broken_by_the_device_id_the_same_way_on_both_devices() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    let (phone_sync, mac_sync) = (
        engine(&phone, sidecar.link("listener")),
        engine(&mac, sidecar.link("listener")),
    );
    let list = phone.create_playlist("Shared", None, None, 1_000).unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();

    phone
        .rename_playlist(&list.id, "From the phone", 5_000)
        .unwrap();
    mac.rename_playlist(&list.id, "From the mac", 5_000)
        .unwrap();
    mac_sync.sync().await.unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();

    assert_eq!(
        playlist_name(&phone, &list.id).as_deref(),
        Some("From the phone")
    );
    assert_eq!(
        playlist_name(&mac, &list.id).as_deref(),
        Some("From the phone")
    );
    assert_eq!(synced_rows(&phone), synced_rows(&mac));
}

#[tokio::test]
async fn concurrent_inserts_into_one_playlist_all_survive() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    let (phone_sync, mac_sync) = (
        engine(&phone, sidecar.link("listener")),
        engine(&mac, sidecar.link("listener")),
    );
    let list = phone.create_playlist("Shared", None, None, 1_000).unwrap();
    phone
        .add_tracks(&[TrackLink::new("t0", "k0")], &list.id, 1_100)
        .unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();

    phone
        .add_tracks(&[TrackLink::new("p1", "kp1")], &list.id, 5_000)
        .unwrap();
    mac.add_tracks(&[TrackLink::new("m1", "km1")], &list.id, 5_000)
        .unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();
    phone_sync.sync().await.unwrap();

    let ids: HashSet<String> = item_ids(&phone, &list.id).into_iter().collect();
    assert_eq!(ids, ["t0", "p1", "m1"].map(str::to_owned).into());
    assert_eq!(synced_rows(&phone), synced_rows(&mac));
}

// MARK: - 6. Cursor confusion

/// The trap, built rather than described.
#[tokio::test]
async fn storing_the_push_cursor_as_the_pull_cursor_would_lose_another_devices_ops() {
    let sidecar = Sidecar::new();
    let mac = store("mac");
    let phone = store("phone");
    let link = sidecar.link("listener");
    for index in 0..10 {
        mac.create_playlist(&format!("Mac {index}"), None, None, 1_000 + index)
            .unwrap();
    }
    push_only(&mac, &link).await;

    phone.create_playlist("Mine", None, None, 2_000).unwrap();
    let wire = pending(&phone)
        .iter()
        .filter_map(|op| WireOp::make(op).ok())
        .collect();
    let response = link
        .push(&PushRequest {
            device_id: "phone".into(),
            ops: wire,
        })
        .await
        .unwrap();
    assert_eq!(response.cursor, Some(PushCursor(11)));

    // Had that been stored as the read position: nothing, forever.
    let head = response.cursor.unwrap();
    let from_the_head = link.pull(Some(PullCursor(head.0)), 500).await.unwrap();
    assert!(from_the_head.ops.is_empty());
    assert!(!from_the_head.has_more);

    let report = engine(&phone, link).sync().await.unwrap();
    assert_eq!(report.applied, 10);
    assert_eq!(phone.playlists().unwrap().len(), 11);
    assert_eq!(phone.pull_cursor().unwrap(), Some(PullCursor(11)));
}

#[tokio::test]
async fn the_push_head_can_sit_above_every_op_this_user_has() {
    let sidecar = Sidecar::new();
    let mine = store("phone");
    let theirs = store("their-phone");
    for index in 0..5 {
        theirs
            .create_playlist(&format!("Theirs {index}"), None, None, 1_000 + index)
            .unwrap();
    }
    push_only(&theirs, &sidecar.link("someone-else")).await;

    mine.create_playlist("Mine", None, None, 2_000).unwrap();
    let link = sidecar.link("listener");
    let wire = pending(&mine)
        .iter()
        .filter_map(|op| WireOp::make(op).ok())
        .collect();
    let response = link
        .push(&PushRequest {
            device_id: "phone".into(),
            ops: wire,
        })
        .await
        .unwrap();
    // Six, not one — the sequence counts every commit on the server.
    assert_eq!(response.cursor, Some(PushCursor(6)));
    let all = link.pull(None, 500).await.unwrap();
    assert_eq!(all.ops.iter().map(|op| op.seq).collect::<Vec<_>>(), [6]);
    assert!(
        link.pull(Some(PullCursor(6)), 500)
            .await
            .unwrap()
            .ops
            .is_empty()
    );
}

// MARK: - 7. Rejection

#[tokio::test]
async fn a_rejected_op_is_quarantined_while_its_batch_mates_land() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    phone.create_playlist("First", None, None, 1_000).unwrap();
    phone.create_playlist("Doomed", None, None, 1_100).unwrap();
    phone.create_playlist("Third", None, None, 1_200).unwrap();
    let doomed = pending(&phone)
        .into_iter()
        .find(|op| op.created_at == 1_100)
        .unwrap();
    sidecar.reject(&doomed.op_id, "payload failed validation");
    let sync = engine(&phone, sidecar.link("listener"));

    let report = sync.sync().await.unwrap();
    assert_eq!(report.pushed, 2);
    assert_eq!(report.rejected, 1);
    assert_eq!(report.quarantined_outbound, 1);
    assert!(pending(&phone).is_empty());
    assert_eq!(sidecar.op_count("listener"), 2);

    let held = phone.quarantined_ops(200).unwrap().remove(0);
    assert_eq!(held.op_id, doomed.op_id);
    assert_eq!(held.sync_direction(), Some(SyncDirection::Outbound));
    assert!(held.reason.contains("payload failed validation"));
    assert!(held.payload.contains("Doomed"));

    let pushes = sidecar.pushes_by("phone");
    for _ in 0..3 {
        sync.sync().await.unwrap();
    }
    assert_eq!(sidecar.pushes_by("phone"), pushes);
    assert_eq!(sidecar.op_count("listener"), 2);
    assert_eq!(phone.quarantine_count().unwrap(), 1);
}

#[tokio::test]
async fn a_push_where_every_op_is_rejected_still_drains_the_queue() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    for index in 0..5 {
        phone
            .create_playlist(&format!("List {index}"), None, None, 1_000 + index)
            .unwrap();
    }
    for op in pending(&phone) {
        sidecar.reject(&op.op_id, "no");
    }
    let report = engine_with(
        &phone,
        sidecar.link("listener"),
        SyncPolicy::default().with_push_batch_size(2),
    )
    .sync()
    .await
    .unwrap();
    assert_eq!(report.rejected, 5);
    assert_eq!(report.pushed, 0);
    assert!(pending(&phone).is_empty());
    assert_eq!(phone.quarantine_count().unwrap(), 5);
    assert_eq!(sidecar.op_count("listener"), 0);
}

#[tokio::test]
async fn rejections_in_the_middle_of_a_multi_batch_push_do_not_take_the_batch_with_them() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    for index in 0..9 {
        phone
            .create_playlist(&format!("List {index}"), None, None, 1_000 + index)
            .unwrap();
    }
    let doomed: Vec<Op> = pending(&phone)
        .into_iter()
        .enumerate()
        .filter(|(i, _)| i % 3 == 1)
        .map(|(_, op)| op)
        .collect();
    for op in &doomed {
        sidecar.reject(&op.op_id, "no");
    }
    let report = engine_with(
        &phone,
        sidecar.link("listener"),
        SyncPolicy::default().with_push_batch_size(3),
    )
    .sync()
    .await
    .unwrap();
    assert_eq!(report.pushed, 6);
    assert_eq!(report.rejected, 3);
    assert_eq!(sidecar.op_count("listener"), 6);
    assert!(pending(&phone).is_empty());
    let held: HashSet<String> = phone
        .quarantined_ops(200)
        .unwrap()
        .into_iter()
        .map(|q| q.op_id)
        .collect();
    assert_eq!(held, doomed.into_iter().map(|op| op.op_id).collect());
}

#[tokio::test]
async fn an_op_the_server_would_refuse_is_stopped_before_it_is_sent() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    phone
        .write_local(|tx| {
            Op::new(
                "track",
                "t1",
                aoide_store::OpKind::Upsert,
                r#"{"id":"t1"}"#,
                1_000,
            )
            .with_id("cache-1")
            .insert(tx)
        })
        .unwrap();
    phone.create_playlist("Fine", None, None, 1_100).unwrap();
    let report = engine(&phone, sidecar.link("listener"))
        .sync()
        .await
        .unwrap();
    assert_eq!(report.quarantined_outbound, 1);
    assert_eq!(report.rejected, 0);
    assert_eq!(report.pushed, 1);
    assert_eq!(sidecar.op_count("listener"), 1);
    assert_eq!(sidecar.entities_for("listener"), ["playlists"]);
    assert!(
        phone.quarantined_ops(200).unwrap()[0]
            .reason
            .contains("'track'")
    );
}

#[tokio::test]
async fn an_oversized_payload_is_caught_locally_and_the_batch_around_it_still_lands() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let list = phone.create_playlist("Fine", None, None, 1_000).unwrap();
    phone
        .set_notes(Some(&"x".repeat(300 * 1024)), &list.id, 1_100)
        .unwrap();
    let report = engine(&phone, sidecar.link("listener"))
        .sync()
        .await
        .unwrap();
    assert_eq!(report.pushed, 1);
    assert_eq!(report.quarantined_outbound, 1);
    assert!(
        phone.quarantined_ops(200).unwrap()[0]
            .reason
            .contains("bytes")
    );
    assert_eq!(sidecar.op_count("listener"), 1);
}

/// The client's local validation is only worth anything if it refuses what the server
/// refuses. Every rule, both ends.
#[tokio::test]
async fn every_rule_the_server_enforces_is_a_rule_the_client_checks_first() {
    let base = || {
        wire(
            &aoide_store::new_id(),
            "playlists",
            "list-1",
            json!({"id": "list-1"}),
            1_000,
        )
    };
    let with = |change: &dyn Fn(&mut WireOp)| {
        let mut op = base();
        change(&mut op);
        op
    };
    let refused: Vec<(&str, WireOp)> = vec![
        ("an empty op id", with(&|op| op.op_id = String::new())),
        (
            "an op id over 128 characters",
            with(&|op| op.op_id = "a".repeat(129)),
        ),
        (
            "an empty entity id",
            with(&|op| op.entity_id = String::new()),
        ),
        (
            "an entity id over 256 characters",
            with(&|op| op.entity_id = "b".repeat(257)),
        ),
        ("the track cache", with(&|op| op.entity = "tracks".into())),
        (
            "an entity that does not exist",
            with(&|op| op.entity = "downloads".into()),
        ),
        (
            "an uppercase operation",
            with(&|op| op.operation = "UPSERT".into()),
        ),
        (
            "a capitalised operation",
            with(&|op| op.operation = "Delete".into()),
        ),
        (
            "a payload that is an array",
            with(&|op| op.payload = json!([1])),
        ),
        (
            "a payload that is a string",
            with(&|op| op.payload = json!("nope")),
        ),
        (
            "a payload that is null",
            with(&|op| op.payload = Value::Null),
        ),
        ("a zero timestamp", with(&|op| op.created_at = 0)),
        ("a negative timestamp", with(&|op| op.created_at = -1)),
        (
            "a payload over 256 KB",
            with(&|op| op.payload = json!({"id": "list-1", "notes": "x".repeat(300 * 1024)})),
        ),
    ];
    for (description, op) in &refused {
        assert!(
            refusal(op, &HashSet::new()).is_some(),
            "the server should refuse {description}"
        );
        // And the client refuses the same op before it leaves.
        let local = Op {
            op_id: op.op_id.clone(),
            entity: SyncEntity::from_wire_name(&op.entity)
                .map_or(op.entity.clone(), |e| e.local_table().to_owned()),
            entity_id: op.entity_id.clone(),
            operation: op.operation.clone(),
            payload: aoide_store::canonical_json(&op.payload),
            created_at: op.created_at,
            synced: false,
        };
        assert!(
            WireOp::make(&local).is_err(),
            "the client should refuse {description}"
        );
    }

    let sidecar = Sidecar::new();
    let link = sidecar.link("listener");
    let good = wire(
        "good-1",
        "playlists",
        "list-1",
        json!({"id": "list-1"}),
        1_000,
    );
    let mut ops: Vec<WireOp> = refused.into_iter().map(|(_, op)| op).collect();
    let refused_count = ops.len();
    ops.push(good);
    let response = link
        .push(&PushRequest {
            device_id: "phone".into(),
            ops,
        })
        .await
        .unwrap();
    assert_eq!(response.accepted, ["good-1"]);
    assert_eq!(response.rejected.len(), refused_count);
    assert_eq!(sidecar.op_count("listener"), 1);
}

#[tokio::test]
async fn a_batch_over_the_servers_ceiling_is_refused_outright() {
    let sidecar = Sidecar::new();
    let ops = (0..1_001)
        .map(|index| {
            wire(
                &format!("op-{index}"),
                "playlists",
                &format!("list-{index}"),
                json!({"id": format!("list-{index}")}),
                1_000,
            )
        })
        .collect();
    let error = sidecar
        .link("listener")
        .push(&PushRequest {
            device_id: "phone".into(),
            ops,
        })
        .await
        .unwrap_err();
    assert!(matches!(error, SidecarError::Status { status: 400, .. }));
    assert_eq!(sidecar.op_count("listener"), 0);
}

/// A server ahead of this build relays an entity it has never heard of.
#[tokio::test]
async fn an_entity_from_a_newer_server_is_quarantined_without_stopping_the_batch() {
    let sidecar = Sidecar::new();
    sidecar.also_allow("downloads");
    let phone = store("phone");
    let link = sidecar.link("listener");
    let ops = vec![
        wire(
            "a",
            "playlists",
            "list-1",
            payload(&make_playlist("list-1", "Late Night", 1_000, "mac")),
            1_000,
        ),
        wire(
            "b",
            "downloads",
            "d1",
            json!({"id": "d1", "sizeBytes": 42}),
            1_100,
        ),
        wire(
            "c",
            "playlists",
            "list-2",
            payload(&make_playlist("list-2", "Other", 1_200, "mac")),
            1_200,
        ),
    ];
    link.push(&PushRequest {
        device_id: "mac".into(),
        ops,
    })
    .await
    .unwrap();

    let report = engine(&phone, link.clone()).sync().await.unwrap();
    assert_eq!(report.applied, 2);
    assert_eq!(report.quarantined_inbound, 1);
    assert_eq!(phone.playlists().unwrap().len(), 2);
    assert_eq!(phone.pull_cursor().unwrap(), Some(PullCursor(3)));
    let held = phone.quarantined_ops(200).unwrap().remove(0);
    assert_eq!(held.entity, "downloads");
    assert_eq!(held.sync_direction(), Some(SyncDirection::Inbound));
    assert!(held.payload.contains("42"));
    assert_eq!(engine(&phone, link).sync().await.unwrap().pulled, 0);
    assert_eq!(phone.quarantine_count().unwrap(), 1);
}

/// Ops go up in `createdAt` order, not dependency order.
#[tokio::test]
async fn an_item_that_arrives_before_its_playlist_is_kept_not_dropped() {
    let sidecar = Sidecar::new();
    let mac = store("mac");
    let phone = store("phone");
    let link = sidecar.link("listener");
    let list = mac.create_playlist("Host", None, None, 9_000).unwrap();
    let links: Vec<TrackLink> = (0..3)
        .map(|i| TrackLink::new(format!("t{i}"), format!("k{i}")))
        .collect();
    mac.add_tracks(&links, &list.id, 1_000).unwrap();
    push_only(&mac, &link).await;
    assert_eq!(
        sidecar.entities_for("listener").last().map(String::as_str),
        Some("playlists")
    );

    let report = engine_with(&phone, link, SyncPolicy::default().with_pull_limit(2))
        .sync()
        .await
        .unwrap();
    assert_eq!(report.orphaned_items, 3);
    assert_eq!(report.applied, 4);
    assert_eq!(item_ids(&phone, &list.id), ["t0", "t1", "t2"]);
    assert_eq!(synced_rows(&phone), synced_rows(&mac));
}

/// The merge is a comparison, so the newest wins whatever position it arrives in.
#[tokio::test]
async fn the_newest_version_in_a_batch_wins_not_the_last_one() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let link = sidecar.link("listener");
    let versions = [("First", 1_000), ("Newest", 3_000), ("Middle", 2_000)];
    let ops = versions
        .iter()
        .enumerate()
        .map(|(index, (name, at))| {
            wire(
                &format!("op-{index}"),
                "playlists",
                "list-1",
                payload(&make_playlist("list-1", name, *at, "mac")),
                *at,
            )
        })
        .collect();
    link.push(&PushRequest {
        device_id: "mac".into(),
        ops,
    })
    .await
    .unwrap();
    let report = engine(&phone, link).sync().await.unwrap();
    assert_eq!(report.applied, 2);
    assert_eq!(report.ignored_stale, 1);
    assert_eq!(playlist_name(&phone, "list-1").as_deref(), Some("Newest"));
}

/// A rescan minted new ids; the fix has to reach the other device without it fighting back.
#[tokio::test]
async fn a_relink_after_a_rescan_travels_and_the_other_device_does_not_undo_it() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    let (phone_sync, mac_sync) = (
        engine(&phone, sidecar.link("listener")),
        engine(&mac, sidecar.link("listener")),
    );
    let key = ContentKey::make(
        "Talk Talk",
        "Laughing Stock",
        "Ascension Day",
        Some(366_000),
    );
    let list = phone.create_playlist("Fragile", None, None, 1_000).unwrap();
    phone
        .add_tracks(&[TrackLink::new("old-1", &key)], &list.id, 1_100)
        .unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();
    assert_eq!(item_ids(&mac, &list.id), ["old-1"]);

    phone
        .relink_items(|_| false, |_| Some("new-1".to_owned()), 5_000)
        .unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();
    assert_eq!(item_ids(&mac, &list.id), ["new-1"]);
    assert_eq!(total_op_count(&mac), 0);
    assert_eq!(synced_rows(&phone), synced_rows(&mac));
}

/// A drag writes exactly one row, and that one row reproduces the order.
#[tokio::test]
async fn a_reorder_travels_as_one_row_and_reproduces_the_order() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    let (phone_sync, mac_sync) = (
        engine(&phone, sidecar.link("listener")),
        engine(&mac, sidecar.link("listener")),
    );
    let list = phone
        .create_playlist("Set list", None, None, 1_000)
        .unwrap();
    let links: Vec<TrackLink> = (0..5)
        .map(|i| TrackLink::new(format!("t{i}"), format!("k{i}")))
        .collect();
    phone.add_tracks(&links, &list.id, 1_100).unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();

    let last = phone.items(&list.id).unwrap().pop().unwrap();
    let ops_before = total_op_count(&phone);
    phone.move_item(&last.id, &list.id, 0, 5_000).unwrap();
    assert_eq!(total_op_count(&phone), ops_before + 1);
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();
    assert_eq!(item_ids(&mac, &list.id), ["t4", "t0", "t1", "t2", "t3"]);
    assert_eq!(total_op_count(&mac), 0);
    assert_eq!(synced_rows(&phone), synced_rows(&mac));
}

/// JSON inside a JSON string inside a JSON payload: where escaping goes wrong.
#[tokio::test]
async fn smart_rules_survive_the_wire_and_still_evaluate_on_the_other_device() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    let rules = SmartRules::new(
        aoide_store::Match::All,
        vec![
            SmartRule::new(
                aoide_store::Field::Genre,
                aoide_store::Operator::Is,
                r#"Jazz "fusion" \ funk"#,
            )
            .unwrap(),
            SmartRule::new(
                aoide_store::Field::PlayCount,
                aoide_store::Operator::GreaterThan,
                2,
            )
            .unwrap(),
        ],
        Some(aoide_store::Sort::new(
            aoide_store::Field::LastPlayed,
            aoide_store::SortDirection::Desc,
        )),
        Some(100),
    )
    .unwrap();
    let list = phone.create_playlist("Rules", None, None, 1_000).unwrap();
    phone
        .record(list.setting_smart_rules(&rules), false, 1_100)
        .unwrap();
    engine(&phone, sidecar.link("listener"))
        .sync()
        .await
        .unwrap();
    engine(&mac, sidecar.link("listener")).sync().await.unwrap();

    let merged: Playlist = mac.playlist(&list.id).unwrap().unwrap();
    assert!(merged.is_smart);
    assert_eq!(merged.smart_rule_set().unwrap(), Some(rules));
    assert_eq!(
        mac.match_count_of(&merged, 2_000, aoide_store::Calendar::Utc)
            .unwrap(),
        0
    );
    assert_eq!(synced_rows(&phone), synced_rows(&mac));
}

/// Quotes, backslashes, newlines, astral emoji, right-to-left scripts.
#[tokio::test]
async fn awkward_text_survives_the_round_trip_unchanged() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    let awkward = "He said \"hi\" \\ then left\n\ttabbed 🎧👨‍👩‍👧‍👦 مرحبا Ω ünïcode";
    let list = phone
        .create_playlist(awkward, None, Some(awkward), 1_000)
        .unwrap();
    engine(&phone, sidecar.link("listener"))
        .sync()
        .await
        .unwrap();
    engine(&mac, sidecar.link("listener")).sync().await.unwrap();
    let merged = mac.playlist(&list.id).unwrap().unwrap();
    assert_eq!(merged.name, awkward);
    assert_eq!(merged.notes.as_deref(), Some(awkward));
    assert_eq!(synced_rows(&phone), synced_rows(&mac));
}

// MARK: - 8. Gaps

#[tokio::test]
async fn a_burnt_sequence_number_is_not_data_loss() {
    let sidecar = Sidecar::new();
    let mac = store("mac");
    let phone = store("phone");
    let link = sidecar.link("listener");
    for index in 0..3 {
        mac.create_playlist(&format!("Before {index}"), None, None, 1_000 + index)
            .unwrap();
    }
    push_only(&mac, &link).await;
    sidecar.burn_sequences(40);
    for index in 0..3 {
        mac.create_playlist(&format!("After {index}"), None, None, 2_000 + index)
            .unwrap();
    }
    push_only(&mac, &link).await;
    assert_eq!(
        sidecar
            .ops_for("listener")
            .iter()
            .map(|row| row.seq)
            .collect::<Vec<_>>(),
        [1, 2, 3, 44, 45, 46]
    );

    let report = engine_with(&phone, link, SyncPolicy::default().with_pull_limit(2))
        .sync()
        .await
        .unwrap();
    assert_eq!(report.applied, 6);
    assert_eq!(phone.pull_cursor().unwrap(), Some(PullCursor(46)));
    assert_eq!(phone.playlists().unwrap().len(), 6);
    assert_eq!(synced_rows(&phone), synced_rows(&mac));
}

#[tokio::test]
async fn a_cursor_inside_a_gap_still_returns_everything_above_it() {
    let sidecar = Sidecar::new();
    let mac = store("mac");
    let phone = store("phone");
    let link = sidecar.link("listener");
    mac.create_playlist("One", None, None, 1_000).unwrap();
    push_only(&mac, &link).await;
    sidecar.burn_sequences(10);
    mac.create_playlist("Two", None, None, 2_000).unwrap();
    push_only(&mac, &link).await;

    phone.set_sync_cursor("6").unwrap();
    let report = engine(&phone, link).sync().await.unwrap();
    assert_eq!(report.applied, 1);
    assert_eq!(names(&phone), ["Two"]);
    assert_eq!(phone.pull_cursor().unwrap(), Some(PullCursor(12)));
}

#[tokio::test]
async fn has_more_is_exact_so_a_full_last_page_ends_the_loop() {
    let sidecar = Sidecar::new();
    let mac = store("mac");
    let phone = store("phone");
    let link = sidecar.link("listener");
    for index in 0..8 {
        mac.create_playlist(&format!("List {index}"), None, None, 1_000 + index)
            .unwrap();
    }
    push_only(&mac, &link).await;
    let report = engine_with(&phone, link, SyncPolicy::default().with_pull_limit(4))
        .sync()
        .await
        .unwrap();
    assert_eq!(report.pulled, 8);
    assert_eq!(sidecar.pull_count(), 2);
}

#[tokio::test]
async fn a_pull_limit_above_the_servers_ceiling_is_clamped_not_refused() {
    let sidecar = Sidecar::new();
    let mac = store("mac");
    let phone = store("phone");
    let link = sidecar.link("listener");
    engine_with(
        &phone,
        link.clone(),
        SyncPolicy::default().with_pull_limit(9_999),
    )
    .sync()
    .await
    .unwrap();
    assert_eq!(
        sidecar.pull_limits_requested(),
        [SyncPolicy::MAX_PULL_LIMIT]
    );

    for index in 0..5 {
        mac.create_playlist(&format!("List {index}"), None, None, 1_000 + index)
            .unwrap();
    }
    push_only(&mac, &link).await;
    let page = link.pull(None, 9_999).await.unwrap();
    assert_eq!(page.ops.len(), 5);
    assert!(!page.has_more);
}

// MARK: - 9. Batch splitting

#[tokio::test]
async fn more_than_a_thousand_ops_go_up_in_several_batches_and_every_one_lands() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    for index in 0..1_100 {
        phone
            .append(
                PlayEvent::new(
                    format!("t{}", index % 20),
                    format!("k{}", index % 20),
                    1_000_000 + index,
                    "phone",
                )
                .with_id(format!("e-{index}"))
                .finished(None, 200_000, true, false),
            )
            .unwrap();
    }
    // Asked for five thousand; the policy clamps to what the server takes.
    let report = engine_with(
        &phone,
        sidecar.link("listener"),
        SyncPolicy::default().with_push_batch_size(5_000),
    )
    .sync()
    .await
    .unwrap();
    assert_eq!(report.pushed, 1_100);
    assert_eq!(report.rejected, 0);
    assert_eq!(sidecar.largest_batch_pushed(), SyncPolicy::MAX_BATCH);
    assert_eq!(sidecar.pushes_by("phone"), Some(2));
    assert_eq!(sidecar.op_count("listener"), 1_100);
    assert!(pending_all(&phone).is_empty());

    let received = engine(&mac, sidecar.link("listener")).sync().await.unwrap();
    assert_eq!(received.applied, 1_100);
    assert_eq!(count::<PlayEvent>(&mac), 1_100);
    assert_eq!(synced_rows(&mac), synced_rows(&phone));
}

/// Every op sharing one millisecond: only the pagination keeps batches apart.
#[tokio::test]
async fn ops_sharing_a_millisecond_still_push_exactly_once_each() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    for index in 0..600 {
        phone
            .append(
                PlayEvent::new("t1", "k1", 7_000, "phone")
                    .with_id(format!("e-{index}"))
                    .finished(None, 250_000, true, false),
            )
            .unwrap();
    }
    let report = engine_with(
        &phone,
        sidecar.link("listener"),
        SyncPolicy::default().with_push_batch_size(100),
    )
    .sync()
    .await
    .unwrap();
    assert_eq!(report.pushed, 600);
    assert_eq!(sidecar.pushes_by("phone"), Some(6));
    assert_eq!(sidecar.op_count("listener"), 600);
    assert!(pending_all(&phone).is_empty());
}

// MARK: - 10. Clock skew

/// A device whose clock says 2096 must not capture rows on other devices.
#[tokio::test]
async fn a_device_with_a_wildly_wrong_clock_does_not_capture_rows_on_other_devices() {
    let sidecar = Sidecar::with_clock(1_699_000_000_000);
    let phone = store("phone");
    let mac = store("mac");
    let phone_sync = engine_at(
        &phone,
        sidecar.link("listener"),
        SyncPolicy::default(),
        1_700_000_050_000,
    );
    let mac_sync = engine_at(
        &mac,
        sidecar.link("listener"),
        SyncPolicy::default(),
        4_000_000_000_000,
    );

    let list = phone
        .create_playlist("Shared", None, None, 1_699_000_000_000)
        .unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();

    mac.rename_playlist(&list.id, "From the mac", 4_000_000_000_000)
        .unwrap();
    sidecar.set_clock(1_699_500_000_000);
    mac_sync.sync().await.unwrap();

    phone
        .rename_playlist(&list.id, "From the phone", 1_700_000_000_000)
        .unwrap();
    sidecar.set_clock(1_700_000_050_000);
    let report = phone_sync.sync().await.unwrap();
    assert_eq!(report.clock_skewed, 1);
    assert_eq!(
        playlist_name(&phone, &list.id).as_deref(),
        Some("From the phone")
    );
}

/// A fixed clock rejoins with no edit on the device that was wrong.
#[tokio::test]
async fn a_fixed_clock_rejoins_the_fold_with_no_edit_on_the_device_that_was_wrong() {
    let honest: Millis = 1_700_000_000_000;
    let broken: Millis = 4_000_000_000_000;
    let sidecar = Sidecar::with_clock(honest);
    let phone = store("phone");
    let mac = store("mac");
    let phone_sync = engine_at(
        &phone,
        sidecar.link("listener"),
        SyncPolicy::default(),
        honest + 100_000,
    );
    let mac_sync = engine_at(
        &mac,
        sidecar.link("listener"),
        SyncPolicy::default(),
        broken,
    );

    let list = phone.create_playlist("Shared", None, None, honest).unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();

    mac.rename_playlist(&list.id, "From the mac", broken)
        .unwrap();
    sidecar.set_clock(honest + 50_000);
    mac_sync.sync().await.unwrap();

    phone
        .rename_playlist(&list.id, "From the phone", honest + 100_000)
        .unwrap();
    sidecar.set_clock(honest + 150_000);
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();

    assert_eq!(
        playlist_name(&phone, &list.id).as_deref(),
        Some("From the phone")
    );
    // Nothing here can protect the mac while its clock agrees with its row.
    assert_eq!(
        playlist_name(&mac, &list.id).as_deref(),
        Some("From the mac")
    );

    phone
        .rename_playlist(&list.id, "Corrected", honest + 200_000)
        .unwrap();
    sidecar.set_clock(honest + 210_000);
    phone_sync.sync().await.unwrap();
    let repaired = engine_at(
        &mac,
        sidecar.link("listener"),
        SyncPolicy::default(),
        honest + 220_000,
    );
    repaired.sync().await.unwrap();
    assert_eq!(playlist_name(&mac, &list.id).as_deref(), Some("Corrected"));
    assert_eq!(synced_rows(&phone), synced_rows(&mac));
}

/// The failure the skew check used to cause: a slow server splitting every device.
#[tokio::test]
async fn a_server_clock_behind_its_devices_does_not_split_them() {
    let base: Millis = 1_800_000_000_000;
    let lag: Millis = 10 * 60 * 1_000;
    let sidecar = Sidecar::with_clock(base - lag);
    let phone = store("phone");
    let mac = store("mac");
    let phone_sync = engine_at(
        &phone,
        sidecar.link("listener"),
        SyncPolicy::default(),
        base + 300_000,
    );
    let mac_sync = engine_at(
        &mac,
        sidecar.link("listener"),
        SyncPolicy::default(),
        base + 300_000,
    );

    let list = phone
        .create_playlist("Late Night", None, None, base)
        .unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();
    assert_eq!(playlist_name(&mac, &list.id).as_deref(), Some("Late Night"));

    mac.rename_playlist(&list.id, "Night Drive", base + 60_000)
        .unwrap();
    sidecar.set_clock(base + 60_000 - lag);
    mac_sync.sync().await.unwrap();
    let to_phone = phone_sync.sync().await.unwrap();
    assert_eq!(
        playlist_name(&phone, &list.id).as_deref(),
        Some("Night Drive")
    );
    assert_eq!(to_phone.clock_skewed, 0);

    phone
        .rename_playlist(&list.id, "Phone Says", base + 180_000)
        .unwrap();
    sidecar.set_clock(base + 180_000 - lag);
    phone_sync.sync().await.unwrap();
    let to_mac = mac_sync.sync().await.unwrap();
    assert_eq!(playlist_name(&mac, &list.id).as_deref(), Some("Phone Says"));
    assert_eq!(to_mac.clock_skewed, 0);
    assert_eq!(synced_rows(&phone), synced_rows(&mac));
}

#[tokio::test]
async fn a_week_old_op_pushed_today_is_not_treated_as_a_broken_clock() {
    let sidecar = Sidecar::with_clock(1_700_000_000_000);
    let mac = store("mac");
    let phone = store("phone");
    let link = sidecar.link("listener");
    let week_ago: Millis = 1_700_000_000_000 - 7 * 24 * 60 * 60 * 1_000;
    mac.create_playlist("Written on a plane", None, None, week_ago)
        .unwrap();
    push_only(&mac, &link).await;
    let report = engine(&phone, link).sync().await.unwrap();
    assert_eq!(report.clock_skewed, 0);
    assert_eq!(phone.playlists().unwrap()[0].updated_at, week_ago);
    assert_eq!(synced_rows(&phone), synced_rows(&mac));
}

// MARK: - 11. Soft deletes

#[tokio::test]
async fn a_delete_travels_and_takes_on_the_other_device() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    let (phone_sync, mac_sync) = (
        engine(&phone, sidecar.link("listener")),
        engine(&mac, sidecar.link("listener")),
    );
    let list = phone
        .create_playlist("Temporary", None, None, 1_000)
        .unwrap();
    phone
        .add_tracks(&[TrackLink::new("t1", "k1")], &list.id, 1_100)
        .unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();
    assert!(mac.playlist(&list.id).unwrap().is_some());

    phone.delete_playlist(&list.id, 5_000).unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();
    assert_eq!(mac.playlist(&list.id).unwrap(), None);
    // The row is still there, carrying the fact of the deletion.
    assert!(
        mac.read(|conn| Playlist::fetch(conn, &list.id))
            .unwrap()
            .unwrap()
            .deleted
    );
    assert!(mac.items(&list.id).unwrap().is_empty());
    assert_eq!(synced_rows(&phone), synced_rows(&mac));
}

/// Deletes-always-win would let a device offline for a week destroy work done since.
#[tokio::test]
async fn a_delete_and_a_newer_edit_resolve_the_same_way_on_both_devices() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    let (phone_sync, mac_sync) = (
        engine(&phone, sidecar.link("listener")),
        engine(&mac, sidecar.link("listener")),
    );
    let list = phone
        .create_playlist("Contested", None, None, 1_000)
        .unwrap();
    phone
        .add_tracks(&[TrackLink::new("t1", "k1")], &list.id, 1_100)
        .unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();

    phone.delete_playlist(&list.id, 5_000).unwrap();
    mac.rename_playlist(&list.id, "Kept", 6_000).unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();
    phone_sync.sync().await.unwrap();

    assert_eq!(playlist_name(&phone, &list.id).as_deref(), Some("Kept"));
    assert_eq!(playlist_name(&mac, &list.id).as_deref(), Some("Kept"));
    // Revived empty: the documented cost of per-row item merging.
    assert!(phone.items(&list.id).unwrap().is_empty());
    assert_eq!(synced_rows(&phone), synced_rows(&mac));
}

#[tokio::test]
async fn a_delete_newer_than_the_edit_it_races_wins_on_both_devices() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    let (phone_sync, mac_sync) = (
        engine(&phone, sidecar.link("listener")),
        engine(&mac, sidecar.link("listener")),
    );
    let list = phone
        .create_playlist("Contested", None, None, 1_000)
        .unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();

    mac.rename_playlist(&list.id, "Renamed", 5_000).unwrap();
    phone.delete_playlist(&list.id, 6_000).unwrap();
    mac_sync.sync().await.unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();

    assert_eq!(phone.playlist(&list.id).unwrap(), None);
    assert_eq!(mac.playlist(&list.id).unwrap(), None);
    assert_eq!(synced_rows(&phone), synced_rows(&mac));
}

#[tokio::test]
async fn the_same_track_liked_on_two_devices_collapses_to_one_row_everywhere() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    let (phone_sync, mac_sync) = (
        engine(&phone, sidecar.link("listener")),
        engine(&mac, sidecar.link("listener")),
    );
    phone
        .record(
            Like {
                id: "like-phone".into(),
                ..Like::new("t1", "k1", true, 0, "phone")
            },
            false,
            5_000,
        )
        .unwrap();
    mac.record(
        Like {
            id: "like-mac".into(),
            ..Like::new("t1", "k1", false, 0, "mac")
        },
        false,
        6_000,
    )
    .unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();
    phone_sync.sync().await.unwrap();

    let likes = |store: &Store| store.read(Like::fetch_all).unwrap();
    assert_eq!(
        likes(&phone)
            .iter()
            .map(|l| l.id.as_str())
            .collect::<Vec<_>>(),
        ["like-mac"]
    );
    assert_eq!(
        likes(&mac)
            .iter()
            .map(|l| l.id.as_str())
            .collect::<Vec<_>>(),
        ["like-mac"]
    );
    assert!(!likes(&phone)[0].liked);
    assert_eq!(synced_rows(&phone), synced_rows(&mac));
}

// MARK: - 12. Play events

#[tokio::test]
async fn two_devices_listening_merges_without_either_losing_a_play() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    let (phone_sync, mac_sync) = (
        engine(&phone, sidecar.link("listener")),
        engine(&mac, sidecar.link("listener")),
    );
    for index in 0..3 {
        phone
            .append(
                PlayEvent::new("t1", "k1", 1_000 + index, "phone")
                    .with_id(format!("phone-e{index}"))
                    .finished(None, 200_000, true, false),
            )
            .unwrap();
        mac.append(
            PlayEvent::new("t1", "k1", 2_000 + index, "mac")
                .with_id(format!("mac-e{index}"))
                .finished(None, 200_000, true, false),
        )
        .unwrap();
    }
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();
    phone_sync.sync().await.unwrap();
    assert_eq!(phone.play_count("t1").unwrap(), 6);
    assert_eq!(mac.play_count("t1").unwrap(), 6);
    assert_eq!(synced_rows(&phone), synced_rows(&mac));
}

#[tokio::test]
async fn the_same_play_event_delivered_twice_is_not_counted_twice() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    for index in 0..4 {
        phone
            .append(
                PlayEvent::new("t1", "k1", 1_000 + index, "phone")
                    .with_id(format!("e{index}"))
                    .finished(None, 200_000, true, false),
            )
            .unwrap();
    }
    engine(&phone, sidecar.link("listener"))
        .sync()
        .await
        .unwrap();
    let mac_sync = engine(&mac, sidecar.link("listener"));
    mac_sync.sync().await.unwrap();
    mac.set_sync_cursor("0").unwrap();
    let again = mac_sync.sync().await.unwrap();
    assert_eq!(again.ignored_duplicate, 4);
    assert_eq!(again.applied, 0);
    assert_eq!(mac.play_count("t1").unwrap(), 4);
}

/// The amendment that has to survive the wire.
#[tokio::test]
async fn an_amended_play_event_reaches_the_other_device() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    let (phone_sync, mac_sync) = (
        engine(&phone, sidecar.link("listener")),
        engine(&mac, sidecar.link("listener")),
    );
    let started = phone
        .begin_play("t1", "k1", PlaySource::Album, None, 5_000)
        .unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();
    assert_eq!(mac.play_count("t1").unwrap(), 0);

    phone
        .finish_play(&started, 250_000, Some(250_000), 255_000, None)
        .unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();
    let merged = mac
        .read(|conn| PlayEvent::fetch(conn, &started.id))
        .unwrap()
        .unwrap();
    assert_eq!(merged.ms_played, 250_000);
    assert!(merged.completed);
    assert_eq!(merged.ended_at, Some(255_000));
    let phone_plays = phone.play_count("t1").unwrap();
    assert_eq!(phone_plays, 1);
    assert_eq!(mac.play_count("t1").unwrap(), phone_plays);
    assert_eq!(synced_rows(&mac), synced_rows(&phone));
}

#[tokio::test]
async fn an_amended_skip_reaches_the_other_device_too() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    let (phone_sync, mac_sync) = (
        engine(&phone, sidecar.link("listener")),
        engine(&mac, sidecar.link("listener")),
    );
    let started = phone
        .begin_play("t1", "k1", PlaySource::Unknown, None, 5_000)
        .unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();

    phone
        .finish_play(&started, 3_000, Some(210_000), 8_000, None)
        .unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();
    assert!(
        mac.read(|conn| PlayEvent::fetch(conn, &started.id))
            .unwrap()
            .unwrap()
            .skipped
    );
    let phone_skips = phone.stats("t1").unwrap().skip_count;
    assert_eq!(phone_skips, 1);
    assert_eq!(mac.stats("t1").unwrap().skip_count, phone_skips);
    assert_eq!(synced_rows(&mac), synced_rows(&phone));
}

/// No sync in between: the amendment supersedes the unsynced begin op.
#[tokio::test]
async fn a_play_that_starts_and_ends_between_syncs_travels_as_one_op() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    let started = phone
        .begin_play("t1", "k1", PlaySource::Unknown, None, 5_000)
        .unwrap();
    phone
        .finish_play(&started, 250_000, Some(250_000), 255_000, None)
        .unwrap();
    assert_eq!(pending(&phone).len(), 1);
    engine(&phone, sidecar.link("listener"))
        .sync()
        .await
        .unwrap();
    engine(&mac, sidecar.link("listener")).sync().await.unwrap();
    assert_eq!(sidecar.op_count("listener"), 1);
    assert!(
        mac.read(|conn| PlayEvent::fetch(conn, &started.id))
            .unwrap()
            .unwrap()
            .completed
    );
    assert_eq!(synced_rows(&mac), synced_rows(&phone));
}

#[tokio::test]
async fn a_replayed_amendment_does_not_undo_itself() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    let started = phone
        .begin_play("t1", "k1", PlaySource::Unknown, None, 5_000)
        .unwrap();
    engine(&phone, sidecar.link("listener"))
        .sync()
        .await
        .unwrap();
    phone
        .finish_play(&started, 250_000, Some(250_000), 255_000, None)
        .unwrap();
    engine(&phone, sidecar.link("listener"))
        .sync()
        .await
        .unwrap();

    let mac_sync = engine(&mac, sidecar.link("listener"));
    mac_sync.sync().await.unwrap();
    let settled = synced_rows(&mac);
    mac.set_sync_cursor("0").unwrap();
    let replay = mac_sync.sync().await.unwrap();
    assert_eq!(replay.applied, 0);
    assert_eq!(replay.ignored_duplicate, 2);
    assert_eq!(synced_rows(&mac), settled);
    assert_eq!(mac.play_count("t1").unwrap(), 1);
}

#[tokio::test]
async fn simultaneous_appends_never_conflict() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    let (phone_sync, mac_sync) = (
        engine(&phone, sidecar.link("listener")),
        engine(&mac, sidecar.link("listener")),
    );
    phone
        .append(
            PlayEvent::new("t1", "k1", 5_000, "phone")
                .with_id("phone-e")
                .finished(None, 200_000, true, false),
        )
        .unwrap();
    mac.append(
        PlayEvent::new("t1", "k1", 5_000, "mac")
            .with_id("mac-e")
            .finished(None, 200_000, true, false),
    )
    .unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();
    phone_sync.sync().await.unwrap();
    assert_eq!(phone.play_count("t1").unwrap(), 2);
    assert_eq!(mac.play_count("t1").unwrap(), 2);
    assert_eq!(synced_rows(&phone), synced_rows(&mac));
}

// MARK: - Queue state

#[tokio::test]
async fn each_devices_queue_is_its_own_row_and_both_survive_the_merge() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let mac = store("mac");
    let (phone_sync, mac_sync) = (
        engine(&phone, sidecar.link("listener")),
        engine(&mac, sidecar.link("listener")),
    );
    phone
        .save_queue("iPhone", &["a".to_owned(), "b".to_owned()], 0, 1_000, 5_000)
        .unwrap();
    mac.save_queue("Studio", &["c".to_owned(), "d".to_owned()], 1, 2_000, 6_000)
        .unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();
    phone_sync.sync().await.unwrap();

    let ids = |store: &Store| {
        store
            .resumable_queues(6_000, Millis::MAX)
            .unwrap()
            .into_iter()
            .map(|q| q.device_id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&phone), ["mac"]);
    assert_eq!(ids(&mac), ["phone"]);
    assert_eq!(
        phone.local_queue().unwrap().unwrap().track_id_list(),
        ["a", "b"]
    );
    assert_eq!(synced_rows(&phone), synced_rows(&mac));
}

#[tokio::test]
async fn rapid_queue_saves_collapse_to_one_op_rather_than_stacking() {
    let sidecar = Sidecar::new();
    let phone = store("phone");
    let tracks: Vec<String> = ["a", "b", "c"].map(str::to_owned).to_vec();
    for index in 0..20 {
        phone
            .save_queue("iPhone", &tracks, index % 3, index * 10_000, 5_000 + index)
            .unwrap();
    }
    assert_eq!(pending(&phone).len(), 1);
    let report = engine(&phone, sidecar.link("listener"))
        .sync()
        .await
        .unwrap();
    assert_eq!(report.pushed, 1);
    assert_eq!(sidecar.op_count("listener"), 1);
    assert_eq!(phone.local_queue().unwrap().unwrap().elapsed_ms, 190_000);
}

// MARK: - A server that throws on one op

fn default_engine(store: &Arc<Store>, link: SidecarLink) -> SyncEngine<SidecarLink> {
    SyncEngine::new(Arc::clone(store), link)
}

#[tokio::test]
async fn the_offending_op_is_found_and_quarantined_and_the_rest_get_through() {
    let sidecar = Sidecar::new();
    let store = store("phone");
    for index in 0..8 {
        store
            .create_playlist(&format!("List {index}"), None, None, now_millis())
            .unwrap();
    }
    let ops: Vec<String> = pending(&store).into_iter().map(|op| op.op_id).collect();
    let victim = ops[3].clone();
    sidecar.poison(&victim);

    let report = default_engine(&store, sidecar.link("u1"))
        .sync()
        .await
        .unwrap();
    assert_eq!(report.quarantined_outbound, 1);
    assert_eq!(report.pushed, ops.len() - 1);
    assert!(pending(&store).is_empty());
    let held = store.quarantined_ops(200).unwrap();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].op_id, victim);
}

/// Only an op that reproduces the failure by itself is blamed for it.
#[tokio::test]
async fn a_fault_that_does_not_reproduce_alone_quarantines_nothing() {
    let sidecar = Sidecar::new();
    let store = store("phone");
    for index in 0..4 {
        store
            .create_playlist(&format!("List {index}"), None, None, now_millis())
            .unwrap();
    }
    for op in pending(&store) {
        sidecar.poison(&op.op_id);
    }
    sidecar.clear_poison_after_next_failure();
    let report = default_engine(&store, sidecar.link("u1"))
        .sync()
        .await
        .unwrap();
    assert_eq!(report.quarantined_outbound, 0);
    assert!(store.quarantined_ops(200).unwrap().is_empty());
    assert!(pending(&store).is_empty());
}

// MARK: - Retrying after a server is fixed

#[tokio::test]
async fn a_held_change_goes_through_once_the_server_stops_failing() {
    let sidecar = Sidecar::new();
    let store = store("phone");
    let list = store
        .create_playlist("Held Back", None, None, now_millis())
        .unwrap();
    let op_id = pending(&store)[0].op_id.clone();
    sidecar.poison(&op_id);
    let engine = default_engine(&store, sidecar.link("u1"));
    engine.sync().await.unwrap();
    assert_eq!(store.quarantined_ops(200).unwrap().len(), 1);
    assert_eq!(store.retryable_quarantine_count().unwrap(), 1);

    sidecar.unpoison(&op_id);
    assert_eq!(store.requeue_quarantined(None).unwrap(), 1);
    assert!(store.quarantined_ops(200).unwrap().is_empty());
    let report = engine.sync().await.unwrap();
    assert_eq!(report.pushed, 1);
    let stored = sidecar.ops_for("u1");
    assert!(stored.iter().any(|row| row.op.op_id == op_id));
    assert!(stored.iter().any(|row| row.op.entity_id == list.id));
}

#[test]
fn a_permanently_invalid_op_is_not_offered_a_retry() {
    let store = store("phone");
    store
        .create_playlist("Bad", None, None, now_millis())
        .unwrap();
    let op = pending(&store).remove(0);
    store
        .quarantine(&op, &SyncFault::PayloadTooLarge { bytes: 999_999 }, 100)
        .unwrap();
    assert_eq!(store.retryable_quarantine_count().unwrap(), 0);
    assert_eq!(store.requeue_quarantined(None).unwrap(), 0);
    assert_eq!(store.quarantined_ops(200).unwrap().len(), 1);
}

// MARK: - A stall that explains itself

fn store_with_work() -> Arc<Store> {
    let store = store("phone");
    for index in 0..3 {
        store
            .create_playlist(&format!("List {index}"), None, None, now_millis())
            .unwrap();
    }
    store
}

#[tokio::test]
async fn a_server_that_acknowledges_nothing_says_so() {
    let sidecar = Sidecar::new();
    sidecar.stop_acknowledging();
    let store = store_with_work();
    let engine = default_engine(&store, sidecar.link("u1"));
    assert!(engine.sync().await.is_err());
    let error = engine.sync().await.unwrap_err();
    let SyncError::PushStalled {
        pending,
        accepted,
        rejected,
        unmatched_rejections,
    } = error
    else {
        panic!("expected a stall, got {error}")
    };
    assert_eq!(
        (pending, accepted, rejected, unmatched_rejections),
        (3, 0, 0, 0)
    );
}

/// A rejection naming an op never sent is told apart from the server saying nothing.
#[tokio::test]
async fn a_server_rejecting_ids_it_was_never_sent_is_told_apart_from_one_saying_nothing() {
    let sidecar = Sidecar::new();
    sidecar.reject_with_unknown_ids();
    let store = store_with_work();
    let engine = default_engine(&store, sidecar.link("u1"));
    let _ = engine.sync().await;
    let error = engine.sync().await.unwrap_err();
    let SyncError::PushStalled {
        accepted,
        rejected,
        unmatched_rejections,
        ..
    } = error
    else {
        panic!("expected a stall, got {error}")
    };
    assert_eq!(accepted, 0);
    assert_eq!(
        rejected, 3,
        "the server did answer, in a shape that matched nothing"
    );
    assert_eq!(unmatched_rejections, 3);
    assert!(store.quarantined_ops(200).unwrap().is_empty());
    assert_eq!(pending(&store).len(), 3);
}

// MARK: - Not in the Swift suite: what the contract says that the phone does not do

/// A 503 is "storage unavailable": the ops are fine and stay queued. Bisecting it as a
/// server fault would quarantine every op in the batch one by one.
#[tokio::test]
async fn a_503_keeps_every_op_queued_and_quarantines_nothing() {
    struct Unavailable;

    impl SyncTransport for Unavailable {
        async fn push(
            &self,
            _request: &PushRequest,
        ) -> Result<aoide_store::PushResponse, SidecarError> {
            Err(SidecarError::Status {
                status: 503,
                path: "aoide/sync/push".into(),
                detail: Some(r#"{"title":"Sync storage unavailable"}"#.into()),
            })
        }

        async fn pull(
            &self,
            _since: Option<PullCursor>,
            _limit: usize,
        ) -> Result<aoide_store::PullResponse, SidecarError> {
            unreachable!("a failed push ends the sync before the pull")
        }
    }

    let store = store_with_work();
    let error = SyncEngine::new(Arc::clone(&store), Unavailable)
        .sync()
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        SyncError::Transport(SidecarError::Status { status: 503, .. })
    ));
    assert_eq!(pending(&store).len(), 3);
    assert_eq!(store.quarantine_count().unwrap(), 0);
}

/// Dropping a sync mid-flight — what sign-out does — must not leave the engine locked
/// or wait on the network, and the next sync finishes the work.
#[tokio::test]
async fn a_sync_dropped_mid_flight_releases_the_engine_and_sign_out_never_waits_on_it() {
    let sidecar = Sidecar::new();
    sidecar.set_pull_delay(std::time::Duration::from_secs(30));
    let store = store("phone");
    store
        .save_queue("Desk", &["t1".to_owned()], 0, 0, 1_000)
        .unwrap();
    let engine = Arc::new(default_engine(&store, sidecar.link("listener")));

    let running = tokio::spawn({
        let engine = Arc::clone(&engine);
        async move { engine.sync().await }
    });
    // The push has landed and the pull is waiting on the network.
    while sidecar.op_count("listener") == 0 {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    // Sign-out's local work completes while the sync is still in flight.
    let cleared = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        store.clear_queue(2_000)
    })
    .await;
    assert!(
        cleared
            .expect("clearing the queue waited on the network")
            .unwrap()
    );

    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    sidecar.set_pull_delay(std::time::Duration::ZERO);
    let report = engine.sync().await.expect("the engine was left locked");
    assert_eq!(
        report.pushed, 1,
        "the cleared queue goes out with the next sync"
    );
    assert!(pending(&store).is_empty());
}
