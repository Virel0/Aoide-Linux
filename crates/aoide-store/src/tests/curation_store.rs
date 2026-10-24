//! Port of `CurationStoreTests.swift` ("Curation store").

use super::{all, pending, store};
use crate::{
    ContentKey, FractionalIndex, PlayEvent, PlaySource, Playlist, PlaylistItem, now_millis,
};

fn playlist(name: &str) -> Playlist {
    Playlist::new(name, FractionalIndex::initial(), 0, "")
}

/// The invariant the whole sync design rests on.
#[test]
fn saving_a_row_writes_its_op_in_the_same_breath() {
    let store = store();
    let saved = store
        .record(playlist("Late Night"), false, now_millis())
        .unwrap();

    let ops = pending(&store);
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].entity, "playlist");
    assert_eq!(ops[0].entity_id, saved.id);
    assert_eq!(ops[0].operation, "upsert");
}

#[test]
fn every_write_is_stamped_with_this_device_and_the_time() {
    let store = store();
    let saved = store
        .record(playlist("Focus"), false, 1_700_000_000_000)
        .unwrap();
    assert_eq!(saved.origin_device, "phone");
    assert_eq!(saved.updated_at, 1_700_000_000_000);
}

#[test]
fn deleting_soft_deletes_and_emits_a_delete_op() {
    let store = store();
    let saved = store
        .record(playlist("Temporary"), false, now_millis())
        .unwrap();
    store.record(saved, true, now_millis()).unwrap();

    let rows = all::<Playlist>(&store);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].deleted);
    assert_eq!(
        pending(&store)
            .iter()
            .map(|op| op.operation.as_str())
            .collect::<Vec<_>>(),
        ["upsert", "delete"]
    );
}

#[test]
fn acknowledged_ops_stop_being_pending_and_the_rest_remain() {
    let store = store();
    let first = store.record(playlist("One"), false, now_millis()).unwrap();
    store.record(playlist("Two"), false, now_millis()).unwrap();

    let ops = pending(&store);
    let first_op = ops.iter().find(|op| op.entity_id == first.id).unwrap();
    store
        .mark_synced(std::slice::from_ref(&first_op.op_id))
        .unwrap();

    let remaining = pending(&store);
    assert_eq!(remaining.len(), 1);
    assert_ne!(remaining[0].entity_id, first.id);
}

#[test]
fn pending_ops_come_back_oldest_first() {
    let store = store();
    for (index, name) in ["A", "B", "C"].iter().enumerate() {
        store
            .record(playlist(name), false, 1000 + index as i64)
            .unwrap();
    }
    assert_eq!(
        pending(&store)
            .iter()
            .map(|op| op.created_at)
            .collect::<Vec<_>>(),
        [1000, 1001, 1002]
    );
}

#[test]
fn history_is_appended_with_its_op_and_never_edited() {
    let store = store();
    let event = PlayEvent::new("track1", "k", 5_000, "phone")
        .with_source(PlaySource::Playlist)
        .finished(Some(8_000), 3_000, false, true);
    store.append(event).unwrap();

    let stored = all::<PlayEvent>(&store);
    assert_eq!(stored.len(), 1);
    assert!(stored[0].skipped);
    assert_eq!(stored[0].source.as_deref(), Some("playlist"));
    assert_eq!(pending(&store).len(), 1);
}

#[test]
fn the_sync_cursor_round_trips_and_overwrites() {
    let store = store();
    assert_eq!(store.sync_cursor().unwrap(), None);
    store.set_sync_cursor("100").unwrap();
    store.set_sync_cursor("250").unwrap();
    assert_eq!(store.sync_cursor().unwrap().as_deref(), Some("250"));
}

#[test]
fn playlist_items_order_by_their_fractional_position() {
    let store = store();
    let list = store
        .record(playlist("Ordered"), false, now_millis())
        .unwrap();

    let mut previous: Option<String> = None;
    for index in 0..5 {
        let position = FractionalIndex::between(previous.as_deref(), None);
        previous = Some(position.clone());
        let item = PlaylistItem::new(
            &list.id,
            format!("track{index}"),
            format!("k{index}"),
            position,
            0,
            "",
        );
        store.record(item, false, now_millis()).unwrap();
    }

    // Insert between the first two, exactly as a drag would.
    let ordered = |store: &crate::Store| {
        store
            .read(|conn| {
                <PlaylistItem as crate::Record>::fetch_where(conn, "1 ORDER BY position", [])
            })
            .unwrap()
    };
    let items = ordered(&store);
    let between = FractionalIndex::between(Some(&items[0].position), Some(&items[1].position));
    store
        .record(
            PlaylistItem::new(&list.id, "inserted", "ki", between, 0, ""),
            false,
            now_millis(),
        )
        .unwrap();

    let ids: Vec<String> = ordered(&store)
        .into_iter()
        .map(|item| item.jellyfin_id)
        .collect();
    assert_eq!(
        ids,
        ["track0", "inserted", "track1", "track2", "track3", "track4"]
    );
}

/// Rounded to the second and normalised: it matches the same recording after a rescan.
#[test]
fn the_content_key_survives_the_differences_a_rescan_introduces() {
    let a = ContentKey::make("Björk", "Post", "Army of Me", Some(234_100));
    let b = ContentKey::make("  bjork ", "post", "ARMY OF ME", Some(234_400));
    assert_eq!(a, b);
    let different = ContentKey::make("Björk", "Post", "Hyperballad", Some(234_100));
    assert_ne!(a, different);
}

/// Not in the Swift suite: a store on disk survives being closed and reopened, and
/// reopening runs no migration twice.
#[test]
fn a_store_on_disk_reopens_with_its_rows_and_its_schema() {
    let path = std::env::temp_dir().join(format!("aoide-store-{}.sqlite", crate::new_id()));
    {
        let store = crate::Store::open(&path, "phone").unwrap();
        store.create_playlist("Kept", None, None, 1_000).unwrap();
    }
    {
        let store = crate::Store::open(&path, "phone").unwrap();
        assert_eq!(
            store
                .playlists()
                .unwrap()
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["Kept"]
        );
        assert_eq!(pending(&store).len(), 1);
    }
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}
