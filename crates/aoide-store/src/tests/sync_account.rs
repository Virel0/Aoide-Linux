//! Port of `SyncAccountTests.swift` ("Account ownership and queue depth" and
//! "Unclaimed stores"). The `pendingOpCounts()` stream test is not ported: this crate
//! has no change-observation stream (see the crate report).

use super::{count, pending, store};
use crate::{
    AccountClaim, Like, Op, OpKind, PlayEvent, Playlist, PlaylistItem, PullCursor, QueueState,
    Record, Store, SyncFault, TrackFlags, TrackLink, TrackRef, now_millis,
};

fn stocked(store: &Store) {
    let now = now_millis();
    let list = store
        .create_playlist("Late Night", None, None, now)
        .unwrap();
    store
        .add_tracks(&[TrackLink::new("t0", "k0")], &list.id, now)
        .unwrap();
    store
        .record(Like::new("t0", "k0", true, 1, "phone"), false, now)
        .unwrap();
    store
        .append(PlayEvent::new("t0", "k0", 1, "phone"))
        .unwrap();
    store
        .save_queue("iPhone", &["t0".to_owned()], 0, 0, now)
        .unwrap();
}

// MARK: - Queue depth

#[test]
fn the_pending_count_matches_the_pending_rows() {
    let store = store();
    assert_eq!(store.pending_op_count().unwrap(), 0);
    stocked(&store);
    let rows = store.pending_ops(1_000, &Default::default()).unwrap().len() as i64;
    assert_eq!(store.pending_op_count().unwrap(), rows);
}

#[test]
fn acknowledged_ops_stop_being_counted() {
    let store = store();
    stocked(&store);
    let ids: Vec<String> = pending(&store).into_iter().map(|op| op.op_id).collect();
    store.mark_synced(&ids).unwrap();
    assert_eq!(store.pending_op_count().unwrap(), 0);
}

// MARK: - Account ownership

/// It asks, and the data is untouched until it is answered.
#[test]
fn an_unclaimed_store_with_data_asks_and_touches_nothing_until_answered() {
    let store = store();
    stocked(&store);
    let pending_count = store.pending_op_count().unwrap();

    assert_eq!(
        store.adopt_account("user-a").unwrap(),
        AccountClaim::UnknownOwner
    );
    assert_eq!(store.account_id().unwrap(), None);
    assert_eq!(store.pending_op_count().unwrap(), pending_count);

    store.keep_and_claim("user-a").unwrap();
    assert_eq!(store.account_id().unwrap().as_deref(), Some("user-a"));
    assert_eq!(store.pending_op_count().unwrap(), pending_count);
}

/// Signing out and back in as the same person is the ordinary case.
#[test]
fn re_adopting_the_same_account_keeps_unpushed_work() {
    let store = store();
    store.adopt_account("user-a").unwrap();
    stocked(&store);
    let pending_count = store.pending_op_count().unwrap();

    assert_eq!(
        store.adopt_account("user-a").unwrap(),
        AccountClaim::Unchanged
    );
    assert_eq!(store.pending_op_count().unwrap(), pending_count);
    assert_eq!(count::<Playlist>(&store), 1);
}

#[test]
fn a_different_account_erases_everything_it_inherited() {
    let store = store();
    store.adopt_account("user-a").unwrap();
    stocked(&store);
    store
        .set_not_interested(true, "t0", "k0", now_millis())
        .unwrap();
    store
        .quarantine(
            &Op::new("playlist", "x", OpKind::Upsert, "{}", 1),
            &SyncFault::DeleteWithoutFlag,
            1,
        )
        .unwrap();

    assert_eq!(store.adopt_account("user-b").unwrap(), AccountClaim::Erased);
    assert_eq!(store.account_id().unwrap().as_deref(), Some("user-b"));
    assert_eq!(store.pending_op_count().unwrap(), 0);
    assert_eq!(store.quarantine_count().unwrap(), 0);
    let remaining = [
        count::<Playlist>(&store),
        count::<PlaylistItem>(&store),
        count::<Like>(&store),
        count::<PlayEvent>(&store),
        count::<QueueState>(&store),
        count::<TrackRef>(&store),
        // Not in the Swift list, which predates taste flags: they are this account's
        // too, and the next person's smart playlists must not hide what they flagged.
        count::<TrackFlags>(&store),
    ];
    assert_eq!(remaining, [0, 0, 0, 0, 0, 0, 0]);
}

/// Sequences are per user: a cursor carried into a second account skips its first ops.
#[test]
fn a_different_account_forgets_the_pull_cursor() {
    let store = store();
    store.adopt_account("user-a").unwrap();
    store.set_sync_cursor("4000").unwrap();
    store.adopt_account("user-b").unwrap();
    assert_eq!(store.pull_cursor().unwrap(), None);
}

#[test]
fn a_different_account_forgets_that_the_library_was_ever_indexed() {
    let store = store();
    store.adopt_account("user-a").unwrap();
    stocked(&store);
    store.record_library_index(5_000).unwrap();
    assert_eq!(store.library_indexed_at().unwrap(), Some(5_000));

    store.adopt_account("user-b").unwrap();
    assert_eq!(store.library_indexed_at().unwrap(), None);
    assert_eq!(count::<TrackRef>(&store), 0);
}

#[test]
fn the_same_account_keeps_its_place_in_the_log() {
    let store = store();
    store.adopt_account("user-a").unwrap();
    store.set_sync_cursor("4000").unwrap();
    store.adopt_account("user-a").unwrap();
    assert_eq!(store.pull_cursor().unwrap(), Some(PullCursor(4000)));
}

// MARK: - Unclaimed stores

fn seed(store: &Store) {
    store
        .create_playlist("From Before", None, None, now_millis())
        .unwrap();
}

#[test]
fn an_unclaimed_store_holding_data_asks_rather_than_guessing() {
    let store = store();
    seed(&store);
    assert_eq!(
        store.adopt_account("user-a").unwrap(),
        AccountClaim::UnknownOwner
    );
    assert_eq!(store.account_id().unwrap(), None);
}

#[test]
fn an_unclaimed_but_empty_store_is_claimed_without_asking() {
    let store = store();
    assert_eq!(
        store.adopt_account("user-a").unwrap(),
        AccountClaim::Unchanged
    );
    assert_eq!(store.account_id().unwrap().as_deref(), Some("user-a"));
}

#[test]
fn keeping_claims_the_store_and_leaves_the_data_alone() {
    let store = store();
    seed(&store);
    store.adopt_account("user-a").unwrap();
    store.keep_and_claim("user-a").unwrap();
    assert_eq!(store.account_id().unwrap().as_deref(), Some("user-a"));
    assert_eq!(store.playlists().unwrap().len(), 1);
    assert!(!pending(&store).is_empty());
}

#[test]
fn starting_fresh_erases_everything_including_the_pending_ops() {
    let store = store();
    seed(&store);
    store.adopt_account("user-a").unwrap();
    store.erase_and_claim("user-a").unwrap();
    assert_eq!(store.account_id().unwrap().as_deref(), Some("user-a"));
    assert!(store.playlists().unwrap().is_empty());
    assert!(pending(&store).is_empty());
}

#[test]
fn a_store_holding_only_a_track_cache_is_not_worth_asking_about() {
    let store = store();
    store
        .write_local(|tx| TrackRef::new("t1", "k", "Song", "Artist", "Album", 1).insert(tx))
        .unwrap();
    assert_eq!(
        store.adopt_account("user-a").unwrap(),
        AccountClaim::Unchanged
    );
}

#[test]
fn a_store_claimed_by_somebody_else_is_still_erased_without_asking() {
    let store = store();
    store.adopt_account("user-a").unwrap();
    seed(&store);
    assert_eq!(store.adopt_account("user-b").unwrap(), AccountClaim::Erased);
    assert!(store.playlists().unwrap().is_empty());
}
