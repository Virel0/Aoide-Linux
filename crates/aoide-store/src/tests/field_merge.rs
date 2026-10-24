//! Port of `FieldMergeTests.swift` ("Per-field merging").

use super::store;
use crate::field_stamps::merged;
use crate::{
    FieldStamps, Folder, Millis, Playlist, PlaylistItem, SyncRecord, TrackFlags, now_millis,
};

fn playlist(id: &str, name: &str, notes: Option<&str>, at: Millis, device: &str) -> Playlist {
    let mut list = Playlist::new(name, "V", at, device);
    list.id = id.to_owned();
    list.notes = notes.map(str::to_owned);
    list
}

/// Rename on the phone, describe on the laptop, neither knowing about the other.
#[test]
fn two_devices_editing_different_fields_both_win() {
    let mut phone = playlist("p1", "Renamed", Some("original"), 200, "phone");
    let mut laptop = playlist("p1", "Original", Some("described"), 100, "laptop");
    phone.field_updated_at = Some(FieldStamps::from_pairs([("name", 200), ("notes", 50)]));
    laptop.field_updated_at = Some(FieldStamps::from_pairs([("name", 50), ("notes", 100)]));

    let result = merged(&phone, &laptop, &phone).unwrap();
    assert_eq!(result.name, "Renamed");
    assert_eq!(result.notes.as_deref(), Some("described"));
}

#[test]
fn a_row_level_loser_still_contributes_the_field_it_changed_last() {
    let mut older = playlist("p1", "Newer Name", None, 100, "a");
    older.field_updated_at = Some(FieldStamps::from_pairs([("name", 100), ("notes", 10)]));
    let mut newer = playlist("p1", "Older Name", Some("kept"), 200, "b");
    newer.field_updated_at = Some(FieldStamps::from_pairs([("name", 10), ("notes", 200)]));

    let result = merged(&older, &newer, &newer).unwrap();
    assert_eq!(result.name, "Newer Name");
    assert_eq!(result.notes.as_deref(), Some("kept"));
}

/// Ops written before stamps existed must keep working against devices that have them.
#[test]
fn a_row_with_no_stamps_falls_back_to_its_row_timestamp() {
    let old = playlist("p1", "From Before", Some("old notes"), 300, "a");
    assert_eq!(old.field_updated_at, None);
    let mut recent = playlist("p1", "Newer", None, 100, "b");
    recent.field_updated_at = Some(FieldStamps::from_pairs([("name", 100)]));

    let result = merged(&old, &recent, &old).unwrap();
    assert_eq!(result.name, "From Before");
    assert_eq!(result.notes.as_deref(), Some("old notes"));
}

#[test]
fn a_tie_goes_to_the_row_level_winner_so_every_device_agrees() {
    let mut a = playlist("p1", "A", None, 100, "aaa");
    a.field_updated_at = Some(FieldStamps::from_pairs([("name", 100)]));
    let mut b = playlist("p1", "B", None, 100, "bbb");
    b.field_updated_at = Some(FieldStamps::from_pairs([("name", 100)]));

    assert_eq!(merged(&a, &b, &b).unwrap().name, "B");
    assert_eq!(merged(&b, &a, &b).unwrap().name, "B");
}

#[test]
fn writing_a_change_stamps_only_the_fields_that_actually_changed() {
    let store = store();
    let created = store
        .create_playlist("First", None, None, now_millis())
        .unwrap();
    let created_stamps = created.field_updated_at.clone().unwrap();

    let mut renamed = created.clone();
    renamed.name = "Second".into();
    let saved = store
        .record(renamed, false, created.updated_at + 1000)
        .unwrap();
    let stamps = saved.field_updated_at.unwrap();

    assert_eq!(stamps.time("name", 0), created.updated_at + 1000);
    // sortIndex did not change, so its stamp must not move.
    assert_eq!(
        stamps.time("sortIndex", 0),
        created_stamps.time("sortIndex", 0)
    );
}

/// A delete is about the row, not any field.
#[test]
fn a_delete_stamps_no_field() {
    let store = store();
    let created = store
        .create_playlist("Temporary", None, None, now_millis())
        .unwrap();
    let before = created.field_updated_at.clone().unwrap().time("name", 0);
    let deleted = store
        .record(created.clone(), true, created.updated_at + 5000)
        .unwrap();
    assert!(deleted.deleted);
    assert_eq!(deleted.field_updated_at.unwrap().time("name", 0), before);
}

/// Merging a queue field by field could pair one device's position with another's list.
#[test]
fn queue_state_is_deliberately_not_field_merged() {
    // `QueueState` is not a `SyncRecord` at all; items merge per row.
    const {
        assert!(!<PlaylistItem as SyncRecord>::FIELD_STAMPED);
        assert!(<Playlist as SyncRecord>::FIELD_STAMPED);
        assert!(<Folder as SyncRecord>::FIELD_STAMPED);
        assert!(<TrackFlags as SyncRecord>::FIELD_STAMPED);
    }
}
