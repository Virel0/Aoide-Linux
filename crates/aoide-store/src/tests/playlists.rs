//! Port of `PlaylistsTests.swift`: "Playlist and folder editing", "Importing from
//! Jellyfin", "Playlist covers", "Cover blobs", "Import for export adoption" and
//! "Covers that reach the server".

use std::collections::HashSet;

use super::{all, links, pending, store};
use crate::{
    Folder, Millis, Playlist, PlaylistError, PlaylistItem, Record, RelinkReport, Store, StoreError,
    TrackLink, TrackRef, now_millis,
};

fn cache(store: &Store, pairs: &[(&str, &str)]) {
    store
        .write_local(|tx| {
            for (id, key) in pairs {
                TrackRef::new(*id, *key, *id, "a", "b", 0).insert(tx)?;
            }
            Ok(())
        })
        .unwrap();
}

fn ids(items: &[PlaylistItem]) -> Vec<&str> {
    items.iter().map(|item| item.jellyfin_id.as_str()).collect()
}

fn names(lists: &[Playlist]) -> Vec<&str> {
    lists.iter().map(|list| list.name.as_str()).collect()
}

fn folder_names(folders: &[Folder]) -> Vec<&str> {
    folders.iter().map(|folder| folder.name.as_str()).collect()
}

fn is_playlist_error(error: StoreError) -> bool {
    matches!(error, StoreError::Playlist(_))
}

// MARK: - The one that matters

/// If a drag rewrote its neighbours, two devices reordering at once would conflict.
#[test]
fn dragging_a_track_rewrites_exactly_one_row() {
    let store = store();
    let list = store.create_playlist("Set", None, None, 1_000).unwrap();
    let added = store
        .add_tracks(&links(&["a", "b", "c", "d", "e"]), &list.id, 1_000)
        .unwrap();
    let ops_before = pending(&store).len();

    let moved = store.move_item(&added[4].id, &list.id, 1, 2_000).unwrap();
    let after = store.items(&list.id).unwrap();
    assert_eq!(ids(&after), ["a", "e", "b", "c", "d"]);
    for row in after.iter().filter(|row| row.id != moved.id) {
        assert_eq!(row.updated_at, 1_000);
        assert_eq!(
            Some(&row.position),
            added.iter().find(|a| a.id == row.id).map(|a| &a.position)
        );
    }
    let ops = pending(&store);
    assert_eq!(ops.len(), ops_before + 1);
    assert_eq!(ops.last().unwrap().entity_id, moved.id);
}

#[test]
fn a_drag_that_lands_where_it_started_writes_nothing_at_all() {
    let store = store();
    let list = store.create_playlist("Still", None, None, 0).unwrap();
    let added = store
        .add_tracks(&links(&["a", "b", "c"]), &list.id, 0)
        .unwrap();
    let ops_before = pending(&store).len();
    store.move_item(&added[1].id, &list.id, 1, 5_000).unwrap();
    assert_eq!(pending(&store).len(), ops_before);
    let items = store.items(&list.id).unwrap();
    assert_eq!(ids(&items), ["a", "b", "c"]);
    assert_eq!(items[1].updated_at, 0);
}

// MARK: - Ordering

#[test]
fn a_track_can_be_dragged_to_the_front_the_end_and_the_middle() {
    let store = store();
    let list = store.create_playlist("Edges", None, None, 0).unwrap();
    let added = store
        .add_tracks(&links(&["a", "b", "c", "d"]), &list.id, 0)
        .unwrap();
    store.move_item(&added[2].id, &list.id, 0, 1).unwrap();
    assert_eq!(ids(&store.items(&list.id).unwrap()), ["c", "a", "b", "d"]);
    store.move_item(&added[0].id, &list.id, 3, 2).unwrap();
    assert_eq!(ids(&store.items(&list.id).unwrap()), ["c", "b", "d", "a"]);
    store.move_item(&added[3].id, &list.id, 1, 3).unwrap();
    assert_eq!(ids(&store.items(&list.id).unwrap()), ["c", "d", "b", "a"]);
}

#[test]
fn order_survives_two_hundred_drags_without_keys_colliding() {
    let store = store();
    let list = store.create_playlist("Churn", None, None, 0).unwrap();
    let mut expected: Vec<String> = (0..12).map(|i| format!("t{i}")).collect();
    let seed_links: Vec<TrackLink> = expected
        .iter()
        .map(|id| TrackLink::new(id, format!("key-{id}")))
        .collect();
    store.add_tracks(&seed_links, &list.id, 0).unwrap();

    // The Swift suite's own generator, so the same two hundred drags run here.
    let mut seed: u64 = 0x5EED;
    let mut random = |bound: usize| {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((seed >> 33) % bound as u64) as usize
    };
    for step in 0..200 {
        let current = store.items(&list.id).unwrap();
        let from = random(current.len());
        let to = random(current.len());
        store
            .move_item(&current[from].id, &list.id, to, 1_000 + step)
            .unwrap();
        let taken = expected.remove(from);
        expected.insert(to, taken);
        assert_eq!(ids(&store.items(&list.id).unwrap()), expected);
    }
    let last = store.items(&list.id).unwrap();
    assert_eq!(
        last.iter()
            .map(|i| &i.position)
            .collect::<HashSet<_>>()
            .len(),
        last.len()
    );
}

#[test]
fn tracks_are_appended_in_the_order_given_duplicates_included() {
    let store = store();
    let list = store.create_playlist("Repeats", None, None, 0).unwrap();
    store.add_tracks(&links(&["a", "b"]), &list.id, 0).unwrap();
    store.add_tracks(&links(&["a", "c"]), &list.id, 1).unwrap();
    assert_eq!(ids(&store.items(&list.id).unwrap()), ["a", "b", "a", "c"]);
    assert_eq!(store.item_count(&list.id).unwrap(), 4);
}

#[test]
fn inserting_a_run_puts_it_in_order_at_the_requested_index() {
    let store = store();
    let list = store.create_playlist("Insert", None, None, 0).unwrap();
    store
        .add_tracks(&links(&["a", "b", "c"]), &list.id, 0)
        .unwrap();
    store
        .insert_tracks(&links(&["x", "y"]), &list.id, 1, 1)
        .unwrap();
    assert_eq!(
        ids(&store.items(&list.id).unwrap()),
        ["a", "x", "y", "b", "c"]
    );
    store.insert_tracks(&links(&["z"]), &list.id, 0, 2).unwrap();
    store
        .insert_tracks(&links(&["w"]), &list.id, 99, 3)
        .unwrap();
    assert_eq!(
        ids(&store.items(&list.id).unwrap()),
        ["z", "a", "x", "y", "b", "c", "w"]
    );
}

#[test]
fn a_removed_track_leaves_the_listing_and_the_count_but_keeps_its_row() {
    let store = store();
    let list = store.create_playlist("Trim", None, None, 0).unwrap();
    let added = store
        .add_tracks(&links(&["a", "b", "c"]), &list.id, 0)
        .unwrap();
    store.remove_item(&added[1].id, 1).unwrap();
    assert_eq!(ids(&store.items(&list.id).unwrap()), ["a", "c"]);
    assert_eq!(store.item_count(&list.id).unwrap(), 2);
    let rows = all::<PlaylistItem>(&store);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows.iter().filter(|r| r.deleted).count(), 1);
}

#[test]
fn removing_twice_does_not_emit_a_second_delete_op() {
    let store = store();
    let list = store.create_playlist("Trim", None, None, 0).unwrap();
    let added = store.add_tracks(&links(&["a"]), &list.id, 0).unwrap();
    store.remove_item(&added[0].id, 1).unwrap();
    let ops = pending(&store).len();
    store.remove_item(&added[0].id, 2).unwrap();
    assert_eq!(pending(&store).len(), ops);
}

// MARK: - Playlists

#[test]
fn deleting_a_playlist_takes_its_items_out_of_every_listing() {
    let store = store();
    let list = store.create_playlist("Gone", None, None, 0).unwrap();
    let other = store.create_playlist("Kept", None, None, 0).unwrap();
    store
        .add_tracks(&links(&["a", "b", "c"]), &list.id, 0)
        .unwrap();
    store.add_tracks(&links(&["d"]), &other.id, 0).unwrap();

    store.delete_playlist(&list.id, 10).unwrap();
    assert_eq!(
        store
            .playlists()
            .unwrap()
            .iter()
            .map(|p| p.id.as_str())
            .collect::<Vec<_>>(),
        [other.id.as_str()]
    );
    assert!(store.items(&list.id).unwrap().is_empty());
    assert_eq!(store.item_count(&list.id).unwrap(), 0);
    assert_eq!(store.items(&other.id).unwrap().len(), 1);
    let deletes = pending(&store)
        .into_iter()
        .filter(|op| op.entity == "playlistItem" && op.operation == "delete")
        .count();
    assert_eq!(deletes, 3);
}

/// Orphans would still match on content key and be rewritten by the next relink.
#[test]
fn no_item_of_a_deleted_playlist_survives_a_later_relink() {
    let store = store();
    let list = store.create_playlist("Gone", None, None, 0).unwrap();
    store.add_tracks(&links(&["old"]), &list.id, 0).unwrap();
    store.delete_playlist(&list.id, 1).unwrap();
    cache(&store, &[("new", "key-old")]);
    let report = store.relink_items_against_track_cache(2).unwrap();
    assert_eq!(report.checked, 0);
    assert_eq!(report.changed(), 0);
}

#[test]
fn deleting_twice_does_not_emit_a_second_op() {
    let store = store();
    let list = store.create_playlist("Gone", None, None, 0).unwrap();
    store.delete_playlist(&list.id, 1).unwrap();
    let ops = pending(&store).len();
    store.delete_playlist(&list.id, 2).unwrap();
    assert_eq!(pending(&store).len(), ops);
}

#[test]
fn renaming_leaves_the_other_fields_alone() {
    let store = store();
    let list = store
        .create_playlist("Old", None, Some("for driving"), 0)
        .unwrap();
    let renamed = store.rename_playlist(&list.id, "New", 5).unwrap();
    assert_eq!(renamed.name, "New");
    assert_eq!(renamed.notes.as_deref(), Some("for driving"));
    assert_eq!(renamed.sort_index, list.sort_index);
    assert_eq!(renamed.updated_at, 5);

    let cleared = store.set_notes(None, &list.id, 6).unwrap();
    assert_eq!(cleared.name, "New");
    assert_eq!(cleared.notes, None);
}

#[test]
fn playlists_list_in_sort_order_and_filter_by_folder() {
    let store = store();
    let folder = store.create_folder("Moods", None, 0).unwrap();
    let first = store.create_playlist("One", None, None, 0).unwrap();
    let second = store
        .create_playlist("Two", Some(&folder.id), None, 0)
        .unwrap();
    let third = store.create_playlist("Three", None, None, 0).unwrap();

    assert_eq!(names(&store.playlists().unwrap()), ["One", "Two", "Three"]);
    let in_folder: Vec<String> = store
        .playlists_in_folder(Some(&folder.id))
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(in_folder, std::slice::from_ref(&second.id));
    let at_root: Vec<String> = store
        .playlists_in_folder(None)
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(at_root, [first.id.clone(), third.id.clone()]);

    store.move_playlist(&first.id, Some(&folder.id), 1).unwrap();
    assert_eq!(
        names(&store.playlists_in_folder(Some(&folder.id)).unwrap()),
        ["Two", "One"]
    );
    let at_root: Vec<String> = store
        .playlists_in_folder(None)
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(at_root, [third.id]);
}

#[test]
fn a_playlist_can_be_reordered_to_the_top_and_back_down() {
    let store = store();
    let a = store.create_playlist("A", None, None, 0).unwrap();
    let b = store.create_playlist("B", None, None, 0).unwrap();
    let c = store.create_playlist("C", None, None, 0).unwrap();
    store.reorder_playlist(&c.id, None, Some(&a.id), 1).unwrap();
    assert_eq!(names(&store.playlists().unwrap()), ["C", "A", "B"]);
    store
        .reorder_playlist(&c.id, Some(&a.id), Some(&b.id), 2)
        .unwrap();
    assert_eq!(names(&store.playlists().unwrap()), ["A", "C", "B"]);
    store.reorder_playlist(&a.id, Some(&b.id), None, 3).unwrap();
    assert_eq!(names(&store.playlists().unwrap()), ["C", "B", "A"]);
}

/// The index only asserts on this in debug builds, so the check has to be real.
#[test]
fn neighbours_the_wrong_way_round_are_refused_not_bisected() {
    let store = store();
    let a = store.create_playlist("A", None, None, 0).unwrap();
    let b = store.create_playlist("B", None, None, 0).unwrap();
    let c = store.create_playlist("C", None, None, 0).unwrap();
    assert!(matches!(
        store.reorder_playlist(&c.id, Some(&b.id), Some(&a.id), 1),
        Err(StoreError::Playlist(PlaylistError::BadNeighbours))
    ));
    assert!(matches!(
        store.reorder_playlist(&b.id, Some(&b.id), None, 1),
        Err(StoreError::Playlist(PlaylistError::BadNeighbours))
    ));
    assert_eq!(names(&store.playlists().unwrap()), ["A", "B", "C"]);
}

#[test]
fn editing_something_that_was_deleted_is_refused() {
    let store = store();
    let list = store.create_playlist("Gone", None, None, 0).unwrap();
    store.delete_playlist(&list.id, 1).unwrap();
    assert!(is_playlist_error(
        store.rename_playlist(&list.id, "Back", 2).unwrap_err()
    ));
    assert!(is_playlist_error(
        store.add_tracks(&links(&["a"]), &list.id, 2).unwrap_err()
    ));
    assert_eq!(store.playlist(&list.id).unwrap(), None);
}

// MARK: - Folders

/// A folder that is its own ancestor makes every tree walk run forever.
#[test]
fn a_folder_cannot_be_moved_under_itself_or_its_own_descendant() {
    let store = store();
    let root = store.create_folder("Root", None, 0).unwrap();
    let child = store.create_folder("Child", Some(&root.id), 0).unwrap();
    let grandchild = store
        .create_folder("Grandchild", Some(&child.id), 0)
        .unwrap();
    for target in [&root.id, &child.id, &grandchild.id] {
        assert!(is_playlist_error(
            store.move_folder(&root.id, Some(target), 1).unwrap_err()
        ));
    }
    assert_eq!(store.folder(&root.id).unwrap().unwrap().parent_id, None);

    let sibling = store.create_folder("Sibling", None, 0).unwrap();
    store
        .move_folder(&sibling.id, Some(&grandchild.id), 2)
        .unwrap();
    assert_eq!(
        store.folder(&sibling.id).unwrap().unwrap().parent_id,
        Some(grandchild.id)
    );
}

/// Per-field merging can join two legal moves into a cycle.
#[test]
fn the_ancestry_walk_terminates_on_a_cycle_that_arrived_from_sync() {
    let store = store();
    let a = store.create_folder("A", None, 0).unwrap();
    let b = store.create_folder("B", Some(&a.id), 0).unwrap();
    store
        .write_local(|tx| {
            tx.execute(
                "UPDATE folder SET parentId = ? WHERE id = ?",
                [&b.id, &a.id],
            )
            .map(|_| ())
        })
        .unwrap();
    let c = store.create_folder("C", None, 1).unwrap();
    assert!(is_playlist_error(
        store.move_folder(&c.id, Some(&b.id), 2).unwrap_err()
    ));
}

/// A folder is filing, not content.
#[test]
fn deleting_a_folder_promotes_its_contents_one_level_rather_than_losing_them() {
    let store = store();
    let outer = store.create_folder("Outer", None, 0).unwrap();
    let inner = store.create_folder("Inner", Some(&outer.id), 0).unwrap();
    let list = store
        .create_playlist("Kept", Some(&inner.id), None, 0)
        .unwrap();
    store.add_tracks(&links(&["a"]), &list.id, 0).unwrap();

    store.delete_folder(&inner.id, 10).unwrap();
    assert_eq!(store.folder(&inner.id).unwrap(), None);
    assert_eq!(
        store.playlist(&list.id).unwrap().unwrap().folder_id,
        Some(outer.id.clone())
    );
    assert_eq!(
        store.playlists_in_folder(Some(&outer.id)).unwrap()[0].id,
        list.id
    );
    assert_eq!(store.items(&list.id).unwrap().len(), 1);

    store.delete_folder(&outer.id, 11).unwrap();
    assert_eq!(store.playlist(&list.id).unwrap().unwrap().folder_id, None);
    assert_eq!(store.playlists_in_folder(None).unwrap()[0].id, list.id);
    assert!(store.folders().unwrap().is_empty());
}

#[test]
fn folders_list_rename_and_reorder() {
    let store = store();
    let a = store.create_folder("A", None, 0).unwrap();
    let b = store.create_folder("B", None, 0).unwrap();
    let nested = store.create_folder("Nested", Some(&a.id), 0).unwrap();
    assert_eq!(
        folder_names(&store.folders().unwrap()),
        ["A", "B", "Nested"]
    );
    assert_eq!(
        folder_names(&store.folders_in_parent(None).unwrap()),
        ["A", "B"]
    );
    assert_eq!(
        store.folders_in_parent(Some(&a.id)).unwrap()[0].id,
        nested.id
    );

    store.rename_folder(&b.id, "Renamed", 1).unwrap();
    assert_eq!(store.folder(&b.id).unwrap().unwrap().name, "Renamed");
    store.reorder_folder(&b.id, None, Some(&a.id), 2).unwrap();
    assert_eq!(
        folder_names(&store.folders_in_parent(None).unwrap()),
        ["Renamed", "A"]
    );
}

#[test]
fn creating_inside_a_folder_that_does_not_exist_is_refused() {
    let store = store();
    assert!(is_playlist_error(
        store
            .create_playlist("Orphan", Some("nope"), None, 0)
            .unwrap_err()
    ));
    assert!(is_playlist_error(
        store.create_folder("Orphan", Some("nope"), 0).unwrap_err()
    ));
}

// MARK: - Relinking

#[test]
fn a_rescan_that_mints_new_ids_is_repaired_by_content_key() {
    let store = store();
    let list = store.create_playlist("Road", None, None, 0).unwrap();
    store
        .add_tracks(
            &[TrackLink::new("old-1", "k1"), TrackLink::new("old-2", "k2")],
            &list.id,
            0,
        )
        .unwrap();
    cache(&store, &[("new-1", "k1"), ("new-2", "k2")]);
    let report = store.relink_items_against_track_cache(10).unwrap();
    assert_eq!(
        report,
        RelinkReport {
            checked: 2,
            relinked: 2,
            recovered: 0,
            unresolved: 0
        }
    );
    let items = store.items(&list.id).unwrap();
    assert_eq!(ids(&items), ["new-1", "new-2"]);
    assert!(items.iter().all(|i| !i.is_unresolved()));
}

/// The drive may simply be unmounted: marked, never deleted.
#[test]
fn a_track_that_matches_nothing_is_marked_never_deleted() {
    let store = store();
    let list = store.create_playlist("Road", None, None, 0).unwrap();
    store
        .add_tracks(
            &[TrackLink::new("old-1", "k1"), TrackLink::new("old-2", "k2")],
            &list.id,
            0,
        )
        .unwrap();
    cache(&store, &[("new-1", "k1")]);
    let report = store.relink_items_against_track_cache(10).unwrap();
    assert_eq!(
        report,
        RelinkReport {
            checked: 2,
            relinked: 1,
            recovered: 0,
            unresolved: 1
        }
    );

    let items = store.items(&list.id).unwrap();
    assert_eq!(items.len(), 2);
    let missing = items.iter().find(|i| i.is_unresolved()).unwrap();
    assert_eq!(missing.linked_jellyfin_id(), "old-2");
    assert_eq!(missing.content_key, "k2");

    let ops = pending(&store).len();
    assert_eq!(
        store
            .relink_items_against_track_cache(11)
            .unwrap()
            .changed(),
        0
    );
    assert_eq!(pending(&store).len(), ops);
}

#[test]
fn a_marked_track_is_unmarked_when_the_drive_comes_back() {
    let store = store();
    let list = store.create_playlist("Road", None, None, 0).unwrap();
    store
        .add_tracks(&[TrackLink::new("old-1", "k1")], &list.id, 0)
        .unwrap();
    cache(&store, &[("unrelated", "other")]);
    assert_eq!(
        store
            .relink_items_against_track_cache(1)
            .unwrap()
            .unresolved,
        1
    );
    cache(&store, &[("old-1", "k1")]);
    let report = store.relink_items_against_track_cache(2).unwrap();
    assert_eq!(
        report,
        RelinkReport {
            checked: 1,
            relinked: 0,
            recovered: 1,
            unresolved: 0
        }
    );
    let item = store.items(&list.id).unwrap().remove(0);
    assert!(!item.is_unresolved());
    assert_eq!(item.jellyfin_id, "old-1");
}

/// The first launch has an empty cache and a full library; a naive relink marks all.
#[test]
fn an_unscanned_library_does_not_mark_the_whole_collection_missing() {
    let store = store();
    let list = store.create_playlist("Road", None, None, 0).unwrap();
    store.add_tracks(&links(&["a", "b"]), &list.id, 0).unwrap();
    let ops = pending(&store).len();
    assert_eq!(
        store.relink_items_against_track_cache(10).unwrap(),
        RelinkReport::default()
    );
    assert_eq!(pending(&store).len(), ops);
    assert!(
        store
            .items(&list.id)
            .unwrap()
            .iter()
            .all(|i| !i.is_unresolved())
    );
}

#[test]
fn relinking_rewrites_the_row_and_its_op_so_the_repair_syncs() {
    let store = store();
    let list = store.create_playlist("Road", None, None, 0).unwrap();
    let added = store
        .add_tracks(&[TrackLink::new("old-1", "k1")], &list.id, 0)
        .unwrap();
    cache(&store, &[("new-1", "k1")]);
    store.relink_items_against_track_cache(10).unwrap();
    let op = pending(&store).pop().unwrap();
    assert_eq!(op.entity, "playlistItem");
    assert_eq!(op.entity_id, added[0].id);
    assert_eq!(op.operation, "upsert");
    assert!(op.payload.contains("new-1"));
    // A relink changes identity, never order.
    assert_eq!(
        store.items(&list.id).unwrap()[0].position,
        added[0].position
    );
}

#[test]
fn relinking_leaves_an_id_that_still_resolves_completely_alone() {
    let store = store();
    let list = store.create_playlist("Road", None, None, 0).unwrap();
    store
        .add_tracks(&[TrackLink::new("live", "k1")], &list.id, 0)
        .unwrap();
    cache(&store, &[("live", "k1"), ("duplicate", "k1")]);
    assert_eq!(
        store
            .relink_items_against_track_cache(10)
            .unwrap()
            .changed(),
        0
    );
    assert_eq!(store.items(&list.id).unwrap()[0].updated_at, 0);
}

// MARK: - Playlist covers

fn cache_album(store: &Store, track: &str, album: &str) {
    let mut row = TrackRef::new(track, format!("key-{track}"), track, "Artist", album, 1);
    row.album_id = Some(album.to_owned());
    store.cache_tracks(&[row]).unwrap();
}

#[test]
fn choosing_a_cover_syncs_as_an_id_and_nothing_else() {
    let store = store();
    let list = store
        .create_playlist("Covered", None, None, now_millis())
        .unwrap();
    let saved = store
        .set_artwork(Some("album-7"), &list.id, now_millis())
        .unwrap()
        .unwrap();
    assert_eq!(saved.artwork_item_id.as_deref(), Some("album-7"));
    let op = pending(&store)
        .into_iter()
        .rev()
        .find(|op| op.entity_id == list.id)
        .unwrap();
    assert!(op.payload.contains("album-7"));
    assert!(op.payload.len() < 2_000);
}

#[test]
fn clearing_a_cover_goes_back_to_the_mosaic() {
    let store = store();
    let list = store
        .create_playlist("Covered", None, None, now_millis())
        .unwrap();
    store
        .set_artwork(Some("album-7"), &list.id, now_millis())
        .unwrap();
    let cleared = store
        .set_artwork(None, &list.id, now_millis())
        .unwrap()
        .unwrap();
    assert_eq!(cleared.artwork_item_id, None);
}

#[test]
fn the_mosaic_draws_from_distinct_albums_in_playing_order() {
    let store = store();
    let list = store
        .create_playlist("Mixed", None, None, now_millis())
        .unwrap();
    let pairs = [("t1", "a1"), ("t2", "a1"), ("t3", "a2"), ("t4", "a3")];
    for (track, album) in pairs {
        cache_album(&store, track, album);
    }
    let tracks: Vec<TrackLink> = pairs
        .iter()
        .map(|(t, _)| TrackLink::new(*t, format!("key-{t}")))
        .collect();
    store.add_tracks(&tracks, &list.id, now_millis()).unwrap();
    let covers = store
        .cover_albums(std::slice::from_ref(&list.id), 4)
        .unwrap();
    assert_eq!(covers[&list.id], ["a1", "a2", "a3"]);
}

#[test]
fn a_removed_song_stops_contributing_its_album() {
    let store = store();
    let list = store
        .create_playlist("Trimmed", None, None, now_millis())
        .unwrap();
    cache_album(&store, "t1", "a1");
    cache_album(&store, "t2", "a2");
    let items = store
        .add_tracks(
            &[
                TrackLink::new("t1", "key-t1"),
                TrackLink::new("t2", "key-t2"),
            ],
            &list.id,
            now_millis(),
        )
        .unwrap();
    store.remove_item(&items[0].id, now_millis()).unwrap();
    assert_eq!(
        store
            .cover_albums(std::slice::from_ref(&list.id), 4)
            .unwrap()[&list.id],
        ["a2"]
    );
}

#[test]
fn covers_for_many_playlists_come_back_in_one_call() {
    let store = store();
    cache_album(&store, "t1", "a1");
    let mut playlist_ids = Vec::new();
    for index in 0..3 {
        let list = store
            .create_playlist(&format!("List {index}"), None, None, now_millis())
            .unwrap();
        store
            .add_tracks(&[TrackLink::new("t1", "key-t1")], &list.id, now_millis())
            .unwrap();
        playlist_ids.push(list.id);
    }
    let covers = store.cover_albums(&playlist_ids, 4).unwrap();
    assert_eq!(covers.len(), 3);
    assert!(covers.values().all(|albums| albums == &["a1"]));
}

// MARK: - Cover blobs

const BYTES: &[u8] = b"not really a jpeg, but bytes are bytes";
const DAY: Millis = 24 * 60 * 60 * 1000;

fn with_image(store: &Store, name: &str, hash: &str, mime: Option<&str>) -> Playlist {
    let list = store
        .create_playlist(name, None, None, now_millis())
        .unwrap();
    let mut row = store.playlist(&list.id).unwrap().unwrap();
    row.image_hash = Some(hash.to_owned());
    row.image_mime = mime.map(str::to_owned);
    store.record(row, false, now_millis()).unwrap()
}

#[test]
fn the_hash_is_the_lower_case_hex_the_server_addresses_by() {
    let hash = Store::image_hash(BYTES);
    assert_eq!(hash.len(), 64);
    assert_eq!(hash, hash.to_lowercase());
    assert_eq!(Store::image_hash(BYTES), hash, "same bytes, same name");
}

/// An op naming a hash the server does not hold leaves a cover nobody can fetch.
#[test]
fn an_op_naming_an_unuploaded_cover_holds_that_cover_back_for_upload() {
    let store = store();
    let hash = Store::image_hash(BYTES);
    store
        .register_image(&hash, "image/jpeg", BYTES.len() as i64, now_millis())
        .unwrap();
    with_image(&store, "Photographed", &hash, None);
    assert_eq!(
        store
            .images_needed_before_push()
            .unwrap()
            .iter()
            .map(|b| b.hash.as_str())
            .collect::<Vec<_>>(),
        [hash.as_str()]
    );
    store.mark_image_uploaded(&hash, now_millis()).unwrap();
    assert!(store.images_needed_before_push().unwrap().is_empty());
}

#[test]
fn a_cover_nothing_references_is_not_queued_for_upload() {
    let store = store();
    let hash = Store::image_hash(BYTES);
    store
        .register_image(&hash, "image/jpeg", BYTES.len() as i64, now_millis())
        .unwrap();
    store
        .create_playlist("No Photo", None, None, now_millis())
        .unwrap();
    assert!(store.images_needed_before_push().unwrap().is_empty());
}

#[test]
fn registering_the_same_bytes_twice_keeps_the_upload_mark() {
    let store = store();
    let hash = Store::image_hash(BYTES);
    store
        .register_image(&hash, "image/jpeg", BYTES.len() as i64, now_millis())
        .unwrap();
    store.mark_image_uploaded(&hash, 500).unwrap();
    store
        .register_image(&hash, "image/jpeg", BYTES.len() as i64, now_millis())
        .unwrap();
    assert_eq!(store.image(&hash).unwrap().unwrap().uploaded_at, Some(500));
}

#[test]
fn a_cover_no_playlist_names_is_reported_as_orphaned_not_deleted() {
    let store = store();
    let hash = Store::image_hash(BYTES);
    store
        .register_image(&hash, "image/jpeg", BYTES.len() as i64, 100)
        .unwrap();
    let well_past = 100 + 31 * DAY;
    assert_eq!(store.orphaned_images(30, well_past).unwrap().len(), 1);
    with_image(&store, "Uses It", &hash, None);
    assert!(store.orphaned_images(30, well_past).unwrap().is_empty());
    assert!(
        store.image(&hash).unwrap().is_some(),
        "reported, never reclaimed"
    );
}

#[test]
fn a_freshly_orphaned_cover_is_left_alone_until_the_grace_period_is_up() {
    let store = store();
    let hash = Store::image_hash(BYTES);
    store
        .register_image(&hash, "image/jpeg", BYTES.len() as i64, 0)
        .unwrap();
    assert!(store.orphaned_images(30, 29 * DAY).unwrap().is_empty());
    assert_eq!(store.orphaned_images(30, 31 * DAY).unwrap().len(), 1);
}

#[test]
fn a_shorter_grace_than_the_configured_one_is_raised_not_honoured() {
    let store = store();
    let hash = Store::image_hash(BYTES);
    store
        .register_image(&hash, "image/jpeg", BYTES.len() as i64, 0)
        .unwrap();
    assert!(store.orphaned_images(1, 5 * DAY).unwrap().is_empty());
    assert!(store.orphaned_images(0, 5 * DAY).unwrap().is_empty());
    assert!(store.orphaned_images(90, 31 * DAY).unwrap().is_empty());
}

#[test]
fn a_cover_shared_by_two_playlists_survives_losing_one_of_them() {
    let store = store();
    let hash = Store::image_hash(BYTES);
    store
        .register_image(&hash, "image/jpeg", BYTES.len() as i64, 0)
        .unwrap();
    let first = with_image(&store, "First", &hash, None);
    let second = with_image(&store, "Second", &hash, None);
    store.delete_playlist(&first.id, now_millis()).unwrap();
    assert!(
        store.orphaned_images(30, 31 * DAY).unwrap().is_empty(),
        "the other playlist still uses it"
    );
    store.delete_playlist(&second.id, now_millis()).unwrap();
    assert_eq!(store.orphaned_images(30, 31 * DAY).unwrap().len(), 1);
}

#[test]
fn forgetting_a_blob_is_a_separate_explicit_step() {
    let store = store();
    let hash = Store::image_hash(BYTES);
    store
        .register_image(&hash, "image/jpeg", BYTES.len() as i64, 0)
        .unwrap();
    let orphans: Vec<String> = store
        .orphaned_images(30, 31 * DAY)
        .unwrap()
        .into_iter()
        .map(|b| b.hash)
        .collect();
    assert!(
        store.image(&hash).unwrap().is_some(),
        "reporting removes nothing"
    );
    assert_eq!(store.forget_images(&orphans).unwrap(), 1);
    assert_eq!(store.image(&hash).unwrap(), None);
}

#[test]
fn a_cover_adds_only_a_hash_to_the_op_never_bytes() {
    let store = store();
    let hash = Store::image_hash(BYTES);
    store
        .register_image(&hash, "image/jpeg", 250_000, now_millis())
        .unwrap();
    let list = with_image(&store, "Photographed", &hash, Some("image/jpeg"));
    let op = pending(&store)
        .into_iter()
        .rev()
        .find(|op| op.entity_id == list.id)
        .unwrap();
    assert!(op.payload.contains(&hash));
    assert!(
        op.payload.len() < 2_000,
        "a quarter-megabyte cover must not be in here"
    );
}

// MARK: - Covers that reach the server

#[test]
fn a_cover_chosen_as_an_album_carries_no_hash_until_it_is_published() {
    let store = store();
    let list = store
        .create_playlist("Chosen", None, None, now_millis())
        .unwrap();
    store
        .set_artwork(Some("album-1"), &list.id, now_millis())
        .unwrap();
    let row = store.playlist(&list.id).unwrap().unwrap();
    assert_eq!(row.artwork_item_id.as_deref(), Some("album-1"));
    assert_eq!(row.image_hash, None);
    assert!(store.images_needed_before_push().unwrap().is_empty());
}

#[test]
fn publishing_gives_it_bytes_a_hash_and_a_place_in_the_upload_queue() {
    let store = store();
    let list = store
        .create_playlist("Chosen", None, None, now_millis())
        .unwrap();
    store
        .set_artwork(Some("album-1"), &list.id, now_millis())
        .unwrap();
    let hash = Store::image_hash(b"cover bytes");
    store
        .register_image(&hash, "image/jpeg", 11, now_millis())
        .unwrap();
    let mut row = store.playlist(&list.id).unwrap().unwrap();
    row.image_hash = Some(hash.clone());
    row.image_mime = Some("image/jpeg".into());
    store.record(row, false, now_millis()).unwrap();
    assert_eq!(store.images_needed_before_push().unwrap()[0].hash, hash);
    assert_eq!(
        store
            .playlist(&list.id)
            .unwrap()
            .unwrap()
            .artwork_item_id
            .as_deref(),
        Some("album-1")
    );
}

#[test]
fn a_playlist_with_only_a_mosaic_has_nothing_to_publish() {
    let store = store();
    let list = store
        .create_playlist("Derived", None, None, now_millis())
        .unwrap();
    let row = store.playlist(&list.id).unwrap().unwrap();
    assert_eq!(row.artwork_item_id, None);
    assert_eq!(row.image_hash, None);
}
