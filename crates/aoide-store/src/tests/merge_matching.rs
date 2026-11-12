//! How a merge tells the same song apart: a changed id after a rescan, and an
//! entry whose track went missing.

use super::store;
use crate::{Calendar, TrackLink, UNRESOLVED_PREFIX};

#[test]
fn the_same_song_under_a_new_id_still_counts_as_there() {
    let store = store();
    let source = store.create_playlist("Old", None, None, 1).unwrap();
    let target = store.create_playlist("New", None, None, 1).unwrap();
    store
        .add_tracks(&[TrackLink::new("new-id", "same")], &source.id, 1)
        .unwrap();
    store
        .add_tracks(&[TrackLink::new("old-id", "same")], &target.id, 1)
        .unwrap();

    let report = store
        .merge_playlist(&source.id, &target.id, 2, Calendar::Utc)
        .unwrap();

    assert_eq!(report.added(), 0);
    assert_eq!(report.skipped, 1);
}

#[test]
fn a_missing_track_is_copied_with_its_marker_unless_the_target_has_it() {
    let store = store();
    let source = store.create_playlist("Old", None, None, 1).unwrap();
    let target = store.create_playlist("New", None, None, 1).unwrap();
    let gone = format!("{UNRESOLVED_PREFIX}7");
    store
        .add_tracks(
            &[
                TrackLink::new(&gone, "key-7"),
                TrackLink::new(format!("{UNRESOLVED_PREFIX}8"), "key-8"),
            ],
            &source.id,
            1,
        )
        .unwrap();
    // The target holds track 8 live, so the missing copy of it is a repeat.
    store
        .add_tracks(&[TrackLink::new("8", "key-8")], &target.id, 1)
        .unwrap();

    let report = store
        .merge_playlist(&source.id, &target.id, 2, Calendar::Utc)
        .unwrap();

    assert_eq!(report.tracks, 1);
    let copied = store.items(&target.id).unwrap();
    assert_eq!(
        copied.last().map(|item| item.jellyfin_id.as_str()),
        Some(gone.as_str())
    );
    assert!(copied.last().is_some_and(|item| item.is_unresolved()));
}
