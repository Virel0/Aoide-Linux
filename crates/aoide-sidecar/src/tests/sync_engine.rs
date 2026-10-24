//! Port of `SyncEngineTests.swift` ("Sync engine"), against the in-memory sidecar with
//! `FakeSyncServer`'s clock.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use aoide_store::{
    Like, OpKind, PlayEvent, PullCursor, PullResponse, PushRequest, PushResponse, Record, Store,
    SyncEntity, SyncPhase, SyncPolicy, TrackLink, WireOp, now_millis,
};

use super::sidecar_stub::{Sidecar, SidecarLink};
use super::{item_ids, make_playlist, names, payload, pending, store};
use crate::{SidecarError, SyncEngine, SyncError, SyncTransport};

fn server() -> Arc<Sidecar> {
    Sidecar::with_clock(2_000)
}

fn engine(store: &Arc<Store>, server: &Arc<Sidecar>) -> SyncEngine<SidecarLink> {
    engine_with(store, server, SyncPolicy::default())
}

fn engine_with(
    store: &Arc<Store>,
    server: &Arc<Sidecar>,
    policy: SyncPolicy,
) -> SyncEngine<SidecarLink> {
    SyncEngine::new(Arc::clone(store), server.link("listener"))
        .with_policy(policy)
        .with_clock(|| 10_000)
}

fn is_offline(error: &SyncError) -> bool {
    matches!(error, SyncError::Transport(SidecarError::Network(message)) if message == "offline")
}

fn seeded_playlists(count: usize) -> Vec<WireOp> {
    (0..count)
        .map(|index| WireOp {
            op_id: format!("mac-op-{index}"),
            entity: SyncEntity::Playlist.wire_name().to_owned(),
            entity_id: format!("mac-{index}"),
            operation: "upsert".to_owned(),
            payload: payload(&make_playlist(
                &format!("mac-{index}"),
                &format!("Mac {index}"),
                1_000,
                "mac",
            )),
            created_at: 1_000,
        })
        .collect()
}

// MARK: - Push

#[tokio::test]
async fn pending_ops_are_pushed_and_stop_being_pending() {
    let store = store("phone");
    store
        .create_playlist("Focus", None, None, now_millis())
        .unwrap();
    let server = server();
    let report = engine(&store, &server).sync().await.unwrap();
    assert_eq!(report.pushed, 1);
    assert!(pending(&store).is_empty());
    let log = server.ops_for("listener");
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].op.entity, "playlists");
    assert_eq!(server.last_pushed_device().as_deref(), Some("phone"));
}

#[tokio::test]
async fn nothing_pending_means_nothing_is_pushed() {
    let store = store("phone");
    let server = server();
    engine(&store, &server).sync().await.unwrap();
    assert_eq!(server.push_count(), 0);
}

/// The engine's validation and the server's rules have to agree.
#[tokio::test]
async fn a_realistic_set_of_local_changes_is_accepted_whole() {
    let store = store("phone");
    let now = now_millis();
    let list = store
        .create_playlist("Late Night", None, None, now)
        .unwrap();
    let links: Vec<TrackLink> = (0..5)
        .map(|i| TrackLink::new(format!("t{i}"), format!("k{i}")))
        .collect();
    store.add_tracks(&links, &list.id, now).unwrap();
    let folder = store.create_folder("Mixes", None, now).unwrap();
    store
        .move_playlist(&list.id, Some(&folder.id), now)
        .unwrap();
    store
        .record(Like::new("t1", "k1", true, 1, "phone"), false, now)
        .unwrap();
    store
        .append(PlayEvent::new("t1", "k1", 1, "phone"))
        .unwrap();
    store
        .save_queue("iPhone", &["t0".to_owned(), "t1".to_owned()], 0, 0, now)
        .unwrap();
    store.delete_playlist(&list.id, now).unwrap();

    let queued = store.pending_ops(1_000, &Default::default()).unwrap().len();
    let server = server();
    let report = engine(&store, &server).sync().await.unwrap();
    assert_eq!(report.pushed, queued);
    assert_eq!(report.quarantined_outbound, 0);
    assert_eq!(store.quarantine_count().unwrap(), 0);
}

#[tokio::test]
async fn pushes_are_split_into_batches_no_larger_than_the_policy_allows() {
    let store = store("phone");
    for index in 0..25 {
        store
            .create_playlist(&format!("List {index}"), None, None, now_millis())
            .unwrap();
    }
    let server = server();
    let report = engine_with(
        &store,
        &server,
        SyncPolicy::default().with_push_batch_size(10),
    )
    .sync()
    .await
    .unwrap();
    assert_eq!(report.pushed, 25);
    assert_eq!(server.push_count(), 3);
    assert_eq!(server.largest_batch_pushed(), 10);
    assert!(pending(&store).is_empty());
}

/// Over the limit the server 400s the whole push, so the ceiling is never sent.
#[test]
fn the_batch_size_can_never_exceed_the_servers_ceiling() {
    assert_eq!(
        SyncPolicy::default()
            .with_push_batch_size(5_000)
            .push_batch_size,
        SyncPolicy::MAX_BATCH
    );
    assert_eq!(
        SyncPolicy::default()
            .with_push_batch_size(0)
            .push_batch_size,
        1
    );
    assert_eq!(
        SyncPolicy::default().with_pull_limit(5_000).pull_limit,
        SyncPolicy::MAX_PULL_LIMIT
    );
    assert!(SyncPolicy::default().push_batch_size <= SyncPolicy::MAX_BATCH);
}

#[tokio::test]
async fn a_push_that_times_out_can_simply_be_sent_again() {
    let store = store("phone");
    store
        .create_playlist("Focus", None, None, now_millis())
        .unwrap();
    let server = server();
    let engine = engine(&store, &server);
    server.go_offline_for_next_push();
    assert!(is_offline(&engine.sync().await.unwrap_err()));
    assert_eq!(pending(&store).len(), 1);

    let report = engine.sync().await.unwrap();
    assert_eq!(report.pushed, 1);
    assert!(pending(&store).is_empty());
    assert_eq!(server.op_count("listener"), 1);
}

#[tokio::test]
async fn an_op_the_server_rejects_is_quarantined_and_never_sent_again() {
    let store = store("phone");
    store
        .create_playlist("Doomed", None, None, now_millis())
        .unwrap();
    store
        .create_playlist("Fine", None, None, now_millis())
        .unwrap();
    let doomed = pending(&store).remove(0);
    let server = server();
    server.reject(&doomed.op_id, "payload failed validation");
    let engine = engine(&store, &server);

    let report = engine.sync().await.unwrap();
    assert_eq!(report.rejected, 1);
    assert_eq!(report.pushed, 1);
    assert!(pending(&store).is_empty());
    let quarantined = store.quarantined_ops(200).unwrap().remove(0);
    assert_eq!(quarantined.op_id, doomed.op_id);
    assert_eq!(
        quarantined.sync_direction(),
        Some(aoide_store::SyncDirection::Outbound)
    );
    assert!(quarantined.reason.contains("payload failed validation"));

    let pushes = server.push_count();
    engine.sync().await.unwrap();
    assert_eq!(server.push_count(), pushes);
    assert_eq!(store.quarantine_count().unwrap(), 1);
}

/// An op that would certainly bounce never costs a round trip.
#[tokio::test]
async fn an_op_that_would_certainly_bounce_is_quarantined_before_it_is_sent() {
    let store = store("phone");
    store
        .write_local(|tx| {
            aoide_store::Op::new("track", "t1", OpKind::Upsert, r#"{"id":"t1"}"#, 1_000)
                .with_id("bad-1")
                .insert(tx)
        })
        .unwrap();
    store
        .create_playlist("Fine", None, None, now_millis())
        .unwrap();
    let server = server();
    let report = engine(&store, &server).sync().await.unwrap();
    assert_eq!(report.quarantined_outbound, 1);
    assert_eq!(report.rejected, 0);
    assert_eq!(report.pushed, 1);
    assert_eq!(server.op_count("listener"), 1);
    assert!(
        store.quarantined_ops(200).unwrap()[0]
            .reason
            .contains("'track'")
    );
}

#[tokio::test]
async fn a_whole_batch_of_invalid_ops_still_lets_the_queue_drain() {
    let store = store("phone");
    store
        .write_local(|tx| {
            for index in 0..3_i64 {
                aoide_store::Op::new(
                    "track",
                    format!("t{index}"),
                    OpKind::Upsert,
                    r#"{"id":"t"}"#,
                    index + 1,
                )
                .with_id(format!("bad-{index}"))
                .insert(tx)?;
            }
            Ok(())
        })
        .unwrap();
    let server = server();
    let report = engine_with(
        &store,
        &server,
        SyncPolicy::default().with_push_batch_size(2),
    )
    .sync()
    .await
    .unwrap();
    assert_eq!(report.quarantined_outbound, 3);
    assert!(pending(&store).is_empty());
    assert_eq!(server.push_count(), 0);
}

/// A server that accepts nothing and rejects nothing would otherwise be retried forever.
#[tokio::test]
async fn a_push_that_changes_nothing_is_reported_rather_than_retried_forever() {
    let store = store("phone");
    store
        .create_playlist("Focus", None, None, now_millis())
        .unwrap();
    let server = server();
    server.accept_nothing();
    let error = engine(&store, &server).sync().await.unwrap_err();
    assert!(matches!(
        error,
        SyncError::PushStalled {
            pending: 1,
            accepted: 0,
            rejected: 0,
            unmatched_rejections: 0
        }
    ));
    assert_eq!(pending(&store).len(), 1);
}

// MARK: - Cursors

/// The push cursor is the head of the whole log; storing it as the read position
/// skips every op another device pushed below it.
#[tokio::test]
async fn the_push_cursor_is_never_mistaken_for_the_pull_cursor() {
    let store = store("phone");
    let server = server();
    server.seed("listener", seeded_playlists(10), "mac");
    // Stamped from the engine's own clock; see the Swift suite.
    store.create_playlist("Mine", None, None, 9_000).unwrap();

    let report = engine(&store, &server).sync().await.unwrap();
    assert_eq!(report.pushed, 1);
    assert_eq!(report.pulled, 11);
    // Ten from the mac; the eleventh is ours, losing to the row that wrote it.
    assert_eq!(report.applied, 10);
    assert_eq!(store.playlists().unwrap().len(), 11);
    assert_eq!(
        store.pull_cursor().unwrap(),
        Some(PullCursor(server.head()))
    );
}

#[tokio::test]
async fn an_interrupted_pull_resumes_from_the_last_batch_it_applied() {
    let store = store("phone");
    let server = server();
    server.seed("listener", seeded_playlists(10), "mac");
    server.fail_pull(2);
    let engine = engine_with(&store, &server, SyncPolicy::default().with_pull_limit(4));
    assert!(matches!(
        engine.sync().await,
        Err(SyncError::Transport(SidecarError::Network(_)))
    ));
    assert_eq!(store.playlists().unwrap().len(), 4);
    assert_eq!(store.pull_cursor().unwrap(), Some(PullCursor(4)));

    let report = engine.sync().await.unwrap();
    assert_eq!(report.applied, 6);
    assert_eq!(store.playlists().unwrap().len(), 10);
}

#[tokio::test]
async fn has_more_drives_the_pull_until_the_log_runs_out() {
    let store = store("phone");
    let server = server();
    server.seed("listener", seeded_playlists(9), "mac");
    let report = engine_with(&store, &server, SyncPolicy::default().with_pull_limit(4))
        .sync()
        .await
        .unwrap();
    assert_eq!(report.pulled, 9);
    assert_eq!(report.applied, 9);
    // Two full pages and a short one; `has_more` is exact.
    assert_eq!(server.pull_count(), 3);
}

/// Advancing on an empty response is the one way left to skip an op.
#[tokio::test]
async fn an_empty_pull_does_not_move_the_cursor() {
    let store = store("phone");
    let server = server();
    let report = engine(&store, &server).sync().await.unwrap();
    assert_eq!(report.pulled, 0);
    assert_eq!(store.pull_cursor().unwrap(), None);
}

// MARK: - Two devices

#[tokio::test]
async fn a_playlist_made_on_one_device_arrives_whole_on_the_other() {
    let server = server();
    let phone = store("phone");
    let mac = store("mac");
    let list = phone
        .create_playlist("Late Night", None, None, now_millis())
        .unwrap();
    let links: Vec<TrackLink> = (0..3)
        .map(|i| TrackLink::new(format!("t{i}"), format!("k{i}")))
        .collect();
    phone.add_tracks(&links, &list.id, now_millis()).unwrap();
    engine(&phone, &server).sync().await.unwrap();
    engine(&mac, &server).sync().await.unwrap();

    assert_eq!(names(&mac), ["Late Night"]);
    assert_eq!(item_ids(&mac, &list.id), ["t0", "t1", "t2"]);
    // And the merge produced nothing to push back.
    assert!(pending(&mac).is_empty());
}

#[tokio::test]
async fn an_edit_made_on_the_second_device_comes_back_to_the_first() {
    let server = server();
    let phone = store("phone");
    let mac = store("mac");
    let (phone_sync, mac_sync) = (engine(&phone, &server), engine(&mac, &server));
    let list = phone.create_playlist("Draft", None, None, 1_000).unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();

    mac.rename_playlist(&list.id, "Final", 5_000).unwrap();
    mac_sync.sync().await.unwrap();
    phone_sync.sync().await.unwrap();
    assert_eq!(phone.playlist(&list.id).unwrap().unwrap().name, "Final");
    assert_eq!(mac.playlist(&list.id).unwrap().unwrap().name, "Final");
}

#[tokio::test]
async fn two_devices_editing_one_playlist_converge() {
    let server = server();
    let phone = store("phone");
    let mac = store("mac");
    let (phone_sync, mac_sync) = (engine(&phone, &server), engine(&mac, &server));
    let list = phone.create_playlist("Shared", None, None, 1_000).unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();

    phone
        .rename_playlist(&list.id, "From phone", 5_000)
        .unwrap();
    mac.rename_playlist(&list.id, "From mac", 6_000).unwrap();
    phone_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();
    phone_sync.sync().await.unwrap();
    assert_eq!(phone.playlist(&list.id).unwrap().unwrap().name, "From mac");
    assert_eq!(mac.playlist(&list.id).unwrap().unwrap().name, "From mac");
}

/// Each insert writes one row; both survive.
#[tokio::test]
async fn two_devices_adding_tracks_at_once_both_keep_their_additions() {
    let server = server();
    let phone = store("phone");
    let mac = store("mac");
    let (phone_sync, mac_sync) = (engine(&phone, &server), engine(&mac, &server));
    let list = phone.create_playlist("Shared", None, None, 1_000).unwrap();
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

    let sorted = |store: &Store| {
        let mut ids = item_ids(store, &list.id);
        ids.sort();
        ids
    };
    assert_eq!(sorted(&phone), ["m1", "p1"]);
    assert_eq!(sorted(&mac), ["m1", "p1"]);
}

/// A second run that queues work is the ping-pong failure at the loop level.
#[tokio::test]
async fn syncing_again_with_nothing_new_does_nothing_at_all() {
    let server = server();
    let phone = store("phone");
    let mac = store("mac");
    phone
        .create_playlist("Focus", None, None, now_millis())
        .unwrap();
    engine(&phone, &server).sync().await.unwrap();
    engine(&mac, &server).sync().await.unwrap();

    let mac_sync = engine(&mac, &server);
    let second = mac_sync.sync().await.unwrap();
    assert_eq!(second.pushed, 0);
    assert_eq!(second.pulled, 0);
    assert!(second.is_empty());
    let logged = server.op_count("listener");
    mac_sync.sync().await.unwrap();
    assert_eq!(server.op_count("listener"), logged);
}

#[tokio::test]
async fn history_from_another_device_lands_once_however_often_it_is_pulled() {
    let server = server();
    let phone = store("phone");
    let mac = store("mac");
    for index in 0..4 {
        phone
            .append(
                PlayEvent::new(
                    format!("t{index}"),
                    format!("k{index}"),
                    1_000 + index,
                    "phone",
                )
                .finished(None, 200_000, true, false),
            )
            .unwrap();
    }
    engine(&phone, &server).sync().await.unwrap();
    let mac_sync = engine(&mac, &server);
    mac_sync.sync().await.unwrap();
    mac_sync.sync().await.unwrap();
    assert_eq!(super::count::<PlayEvent>(&mac), 4);
}

// MARK: - Running once at a time

/// The loop suspends at every network call, and a second caller could walk in.
#[tokio::test]
async fn two_syncs_cannot_run_at_once() {
    let store = store("phone");
    let server = server();
    server.set_pull_delay(Duration::from_millis(300));
    let engine = Arc::new(engine(&store, &server));

    let first = tokio::spawn({
        let engine = Arc::clone(&engine);
        async move { engine.sync().await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(matches!(
        engine.sync().await,
        Err(SyncError::AlreadyRunning)
    ));
    first.await.unwrap().unwrap();
    // And the refusal is not permanent.
    engine.sync().await.unwrap();
}

#[tokio::test]
async fn a_failed_sync_does_not_leave_the_engine_locked() {
    let store = store("phone");
    store
        .create_playlist("Focus", None, None, now_millis())
        .unwrap();
    let server = server();
    let engine = engine(&store, &server);
    server.go_offline_for_next_push();
    assert!(is_offline(&engine.sync().await.unwrap_err()));
    assert_eq!(engine.sync().await.unwrap().pushed, 1);
}

// MARK: - Reporting

#[tokio::test]
async fn the_engine_reports_where_it_got_to() {
    let store = store("phone");
    store.create_playlist("Focus", None, None, 9_000).unwrap();
    let server = server();
    let phases: Arc<Mutex<Vec<SyncPhase>>> = Arc::default();
    let engine = engine(&store, &server).on_phase_change({
        let phases = Arc::clone(&phases);
        move |phase| phases.lock().unwrap().push(phase.clone())
    });
    engine.sync().await.unwrap();

    let seen = phases.lock().unwrap().clone();
    assert!(seen.contains(&SyncPhase::Pushing { pushed: 1 }));
    // Our own op comes straight back and loses to the row that wrote it.
    assert!(seen.contains(&SyncPhase::Pulling { applied: 0 }));
    assert_eq!(seen.last(), Some(&SyncPhase::Idle));
    assert_eq!(engine.phase(), SyncPhase::Idle);
}

#[tokio::test]
async fn a_failure_is_reported_as_one_with_a_reason() {
    let store = store("phone");
    store
        .create_playlist("Focus", None, None, now_millis())
        .unwrap();
    let server = server();
    server.go_offline_for_next_push();
    let engine = engine(&store, &server);
    assert!(is_offline(&engine.sync().await.unwrap_err()));
    let SyncPhase::Failed { reason } = engine.phase() else {
        panic!("expected a failed phase")
    };
    assert!(reason.contains("offline"));
    assert_eq!(engine.last_report(), None);
}

#[tokio::test]
async fn the_last_report_survives_for_a_ui_to_read() {
    let store = store("phone");
    store
        .create_playlist("Focus", None, None, now_millis())
        .unwrap();
    let server = server();
    let engine = engine(&store, &server);
    engine.sync().await.unwrap();
    let report = engine.last_report().unwrap();
    assert_eq!(report.pushed, 1);
    assert!(!report.is_empty());
}

#[tokio::test]
async fn quarantined_ops_are_countable_and_discardable() {
    let store = store("phone");
    store
        .create_playlist("Doomed", None, None, now_millis())
        .unwrap();
    let doomed = pending(&store).remove(0);
    let server = server();
    server.reject(&doomed.op_id, "nope");
    engine(&store, &server).sync().await.unwrap();
    assert_eq!(store.quarantine_count().unwrap(), 1);
    store
        .discard_quarantined(std::slice::from_ref(&doomed.op_id))
        .unwrap();
    assert_eq!(store.quarantine_count().unwrap(), 0);
    // Discarding does not re-queue it.
    assert!(pending(&store).is_empty());
}

/// A page of nothing but unreadable entries is not an empty page.
#[tokio::test]
async fn a_batch_of_nothing_but_unreadable_ops_still_moves_the_cursor_on() {
    struct UnreadablePull;

    impl SyncTransport for UnreadablePull {
        async fn push(&self, request: &PushRequest) -> Result<PushResponse, SidecarError> {
            Ok(PushResponse {
                accepted: request.ops.iter().map(|op| op.op_id.clone()).collect(),
                ..Default::default()
            })
        }

        async fn pull(
            &self,
            since: Option<PullCursor>,
            _limit: usize,
        ) -> Result<PullResponse, SidecarError> {
            let unreadable_ops = if since.is_none() { 2 } else { 0 };
            Ok(PullResponse {
                ops: vec![],
                cursor: PullCursor(9),
                has_more: false,
                unreadable_ops,
            })
        }
    }

    let store = store("phone");
    let engine = SyncEngine::new(Arc::clone(&store), UnreadablePull).with_clock(|| 10_000);
    let report = engine.sync().await.unwrap();
    assert_eq!(report.unreadable, 2);
    assert_eq!(report.applied, 0);
    assert_eq!(store.pull_cursor().unwrap(), Some(PullCursor(9)));
}
