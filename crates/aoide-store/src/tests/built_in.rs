//! Creating and repairing the built-in smart playlists (`ensure_built_in_playlists`):
//! the iOS `BuiltInPlaylistStoreTests.swift`, and what reaches another device.

use serde_json::Value;

use super::{apply_one, pending, pulled_raw, store};
use crate::{BuiltInPlaylist, Playlist, Store, SyncEntity};

#[test]
fn each_is_made_with_its_rules_stamped_after_the_row() {
    let store = store();
    assert_eq!(
        store.ensure_built_in_playlists(1_000).unwrap(),
        BuiltInPlaylist::ALL
    );
    let stamps: Vec<i64> = pending(&store).iter().map(|op| op.created_at).collect();
    assert_eq!(
        stamps,
        [1_000, 1_001, 1_002, 1_003],
        "the rules a millisecond after each row"
    );

    for built in BuiltInPlaylist::ALL {
        let list = store.playlist(built.id()).unwrap().unwrap();
        assert!(list.is_smart);
        assert_eq!(list.smart_rule_set().unwrap(), Some(built.rules()));
    }
    assert!(store.ensure_built_in_playlists(2_000).unwrap().is_empty());
}

/// What two writes at one instant left on the other devices: a row under the
/// built-in's id, made plain because its rules lost the merge's tie.
#[test]
fn one_that_lost_its_rules_gets_them_back() {
    let store = store();
    let plain = Playlist::new("Heavy Rotation", "W".to_owned(), 500, "desk");
    let plain = Playlist {
        id: BuiltInPlaylist::HeavyRotation.id().to_owned(),
        ..plain
    };
    store.record(plain, false, 500).unwrap();
    assert!(
        store
            .ensure_built_in_playlists(1_000)
            .unwrap()
            .contains(&BuiltInPlaylist::HeavyRotation)
    );
    let list = store
        .playlist(BuiltInPlaylist::HeavyRotation.id())
        .unwrap()
        .unwrap();
    assert!(list.is_smart);
    assert_eq!(
        list.smart_rule_set().unwrap(),
        Some(BuiltInPlaylist::HeavyRotation.rules())
    );
}

#[test]
fn one_deleted_stays_deleted() {
    let store = store();
    store.ensure_built_in_playlists(1_000).unwrap();
    let list = store
        .playlist(BuiltInPlaylist::Rediscover.id())
        .unwrap()
        .unwrap();
    store.record(list, true, 2_000).unwrap();
    assert!(store.ensure_built_in_playlists(3_000).unwrap().is_empty());
    assert!(
        store
            .playlist(BuiltInPlaylist::Rediscover.id())
            .unwrap()
            .is_none()
    );
}

/// The bug itself: another device takes this one's writes and must end up with the
/// smart playlist, not a plain one.
#[test]
fn another_device_receives_them_smart() {
    let desk = Store::in_memory("desk").unwrap();
    desk.ensure_built_in_playlists(1_000).unwrap();
    let phone = store();
    for (seq, op) in pending(&desk).into_iter().enumerate() {
        let payload: Value = serde_json::from_str(&op.payload).unwrap();
        let seq = i64::try_from(seq).unwrap() + 1;
        let entity = SyncEntity::from_local_table(&op.entity)
            .unwrap()
            .wire_name();
        apply_one(
            &phone,
            pulled_raw(entity, &op.entity_id, payload, &op.operation, seq),
        );
    }
    for built in BuiltInPlaylist::ALL {
        let list = phone.playlist(built.id()).unwrap().unwrap();
        assert!(list.is_smart, "{}", built.id());
        assert_eq!(list.smart_rule_set().unwrap(), Some(built.rules()));
    }
}

#[test]
fn a_smart_playlist_is_one_write() {
    let store = store();
    let rules = BuiltInPlaylist::HeavyRotation.rules();
    let list = store
        .create_smart_playlist("A mix: late night", Some("A mix"), &rules, 1_000)
        .unwrap();
    assert!(list.is_smart);
    assert_eq!(pending(&store).len(), 1);
}
