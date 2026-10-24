//! Port of `TrackCacheTests.swift` ("The track cache and the index that fills it").

use super::{pending, store};
use crate::{
    Calendar, Millis, PlayEvent, PlaySource, RelinkReport, SmartRules, TrackLink, TrackRef,
};

fn track(id: &str, key: &str, seen_at: Millis) -> TrackRef {
    let mut row = TrackRef::new(id, key, id, "Artist", "Album", seen_at);
    row.duration_ms = Some(200_000);
    row
}

#[test]
fn a_pass_drops_only_what_it_did_not_see() {
    let store = store();
    store
        .cache_tracks(&[track("kept", "k1", 100), track("retired", "k2", 100)])
        .unwrap();
    store.cache_tracks(&[track("kept", "k1", 250)]).unwrap();
    assert_eq!(store.prune_tracks(200).unwrap(), 1);
    assert_eq!(store.cached_track_count().unwrap(), 1);
    let found = store
        .cached_tracks(&["kept".to_owned(), "retired".to_owned()])
        .unwrap();
    assert_eq!(found.keys().collect::<Vec<_>>(), ["kept"]);
}

#[test]
fn pruning_against_a_cutoff_nothing_predates_removes_nothing() {
    let store = store();
    store
        .cache_tracks(&[track("a", "k1", 300), track("b", "k2", 300)])
        .unwrap();
    assert_eq!(store.prune_tracks(300).unwrap(), 0);
    assert_eq!(store.cached_track_count().unwrap(), 2);
}

/// A hard delete is right only because this table is local and carries no op.
#[test]
fn pruning_writes_no_op() {
    let store = store();
    store.cache_tracks(&[track("gone", "k", 1)]).unwrap();
    let before = pending(&store).len();
    assert_eq!(store.prune_tracks(2).unwrap(), 1);
    assert_eq!(pending(&store).len(), before);
}

/// Without the prune, the retired ids still answer "live" and nothing is repaired.
#[test]
fn a_rescan_is_only_repaired_once_the_retired_ids_leave_the_cache() {
    let store = store();
    let list = store.create_playlist("Road", None, None, 0).unwrap();
    store
        .cache_tracks(&[track("old-1", "k1", 100), track("old-2", "k2", 100)])
        .unwrap();
    store
        .add_tracks(
            &[TrackLink::new("old-1", "k1"), TrackLink::new("old-2", "k2")],
            &list.id,
            100,
        )
        .unwrap();
    store
        .cache_tracks(&[track("new-1", "k1", 250), track("new-2", "k2", 250)])
        .unwrap();

    assert_eq!(
        store
            .relink_items_against_track_cache(300)
            .unwrap()
            .changed(),
        0
    );
    let ids = |store: &crate::Store| -> Vec<String> {
        store
            .items(&list.id)
            .unwrap()
            .into_iter()
            .map(|i| i.jellyfin_id)
            .collect()
    };
    assert_eq!(ids(&store), ["old-1", "old-2"]);

    assert_eq!(store.prune_tracks(200).unwrap(), 2);
    let report = store.relink_items_against_track_cache(300).unwrap();
    assert_eq!(
        report,
        RelinkReport {
            checked: 2,
            relinked: 2,
            recovered: 0,
            unresolved: 0
        }
    );
    assert_eq!(ids(&store), ["new-1", "new-2"]);
    assert!(
        store
            .items(&list.id)
            .unwrap()
            .iter()
            .all(|i| !i.is_unresolved())
    );
}

#[test]
fn a_doubled_cache_doubles_every_smart_playlist_until_it_is_pruned() {
    let store = store();
    store.cache_tracks(&[track("old", "k", 100)]).unwrap();
    store
        .append(
            PlayEvent::new("old", "k", 150, "phone")
                .with_source(PlaySource::Album)
                .finished(Some(150 + 200_000), 200_000, true, false),
        )
        .unwrap();
    store.cache_tracks(&[track("new", "k", 250)]).unwrap();

    let everything = SmartRules::everything();
    let mut both = store.evaluate(&everything, 300, Calendar::Local).unwrap();
    both.sort();
    assert_eq!(both, ["new", "old"]);
    assert_eq!(store.prune_tracks(200).unwrap(), 1);
    assert_eq!(
        store.evaluate(&everything, 300, Calendar::Local).unwrap(),
        ["new"]
    );
}

/// The recorder writes a row for every song started, so a non-empty cache is not an
/// index that has run.
#[test]
fn a_cache_with_rows_in_it_has_still_never_been_indexed() {
    let store = store();
    store.cache_tracks(&[track("played-once", "k", 5)]).unwrap();
    assert_eq!(store.cached_track_count().unwrap(), 1);
    assert_eq!(store.library_indexed_at().unwrap(), None);
    store.record_library_index(1_700_000_000_000).unwrap();
    assert_eq!(store.library_indexed_at().unwrap(), Some(1_700_000_000_000));
}

#[test]
fn the_mark_is_overwritten_not_appended_to() {
    let store = store();
    store.record_library_index(1).unwrap();
    store.record_library_index(2).unwrap();
    assert_eq!(store.library_indexed_at().unwrap(), Some(2));
}

#[test]
fn the_index_mark_and_the_sync_cursor_are_separate_keys() {
    let store = store();
    store.set_sync_cursor("cursor-42").unwrap();
    store.record_library_index(9).unwrap();
    assert_eq!(store.sync_cursor().unwrap().as_deref(), Some("cursor-42"));
    assert_eq!(store.library_indexed_at().unwrap(), Some(9));
}
