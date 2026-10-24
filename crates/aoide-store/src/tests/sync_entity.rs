//! Port of `SyncEntityTests.swift` ("Sync entity names").

use std::collections::HashSet;

use super::{pending, store};
use crate::record::Record;
use crate::{
    Folder, Like, PlayEvent, Playlist, PlaylistItem, QueueState, SyncEntity, TrackFlags, TrackLink,
    TrackRef, now_millis,
};

/// Pinned literally: a test derived from the enum would agree with any typo.
#[test]
fn wire_names_are_the_servers_allow_list_exactly() {
    assert_eq!(SyncEntity::Playlist.wire_name(), "playlists");
    assert_eq!(SyncEntity::PlaylistItem.wire_name(), "playlist_items");
    assert_eq!(SyncEntity::Folder.wire_name(), "folders");
    assert_eq!(SyncEntity::Like.wire_name(), "likes");
    assert_eq!(SyncEntity::PlayEvent.wire_name(), "play_events");
    assert_eq!(SyncEntity::QueueState.wire_name(), "queue_state");
    assert_eq!(SyncEntity::TrackFlags.wire_name(), "track_flags");
}

#[test]
fn local_names_are_the_record_types_table_names() {
    assert_eq!(SyncEntity::Playlist.local_table(), Playlist::TABLE);
    assert_eq!(SyncEntity::PlaylistItem.local_table(), PlaylistItem::TABLE);
    assert_eq!(SyncEntity::Folder.local_table(), Folder::TABLE);
    assert_eq!(SyncEntity::Like.local_table(), Like::TABLE);
    assert_eq!(SyncEntity::PlayEvent.local_table(), PlayEvent::TABLE);
    assert_eq!(SyncEntity::QueueState.local_table(), QueueState::TABLE);
    assert_eq!(SyncEntity::TrackFlags.local_table(), TrackFlags::TABLE);
}

#[test]
fn every_entity_survives_the_round_trip_both_ways() {
    for entity in SyncEntity::ALL {
        assert_eq!(SyncEntity::from_wire_name(entity.wire_name()), Some(entity));
        assert_eq!(
            SyncEntity::from_local_table(entity.local_table()),
            Some(entity)
        );
    }
}

#[test]
fn no_two_entities_share_a_name() {
    let wire: HashSet<_> = SyncEntity::ALL.iter().map(|e| e.wire_name()).collect();
    let local: HashSet<_> = SyncEntity::ALL.iter().map(|e| e.local_table()).collect();
    assert_eq!(wire.len(), SyncEntity::ALL.len());
    assert_eq!(local.len(), SyncEntity::ALL.len());
}

/// The track cache is per device and the server refuses it; it cannot be named.
#[test]
fn the_track_cache_cannot_be_named_at_all() {
    assert_eq!(SyncEntity::from_local_table(TrackRef::TABLE), None);
    assert_eq!(SyncEntity::from_wire_name("tracks"), None);
    assert_eq!(SyncEntity::from_wire_name("track"), None);
}

#[test]
fn an_unknown_name_is_none_rather_than_a_trap() {
    assert_eq!(SyncEntity::from_wire_name("downloads"), None);
    assert_eq!(SyncEntity::from_local_table("download"), None);
    assert_eq!(SyncEntity::from_wire_name(""), None);
    // Case-sensitive on the server, so here too.
    assert_eq!(SyncEntity::from_wire_name("Playlists"), None);
}

#[test]
fn queue_state_is_identified_by_its_device_everything_else_by_its_id() {
    assert_eq!(SyncEntity::QueueState.identity_key(), "deviceId");
    for entity in SyncEntity::ALL
        .into_iter()
        .filter(|e| *e != SyncEntity::QueueState)
    {
        assert_eq!(entity.identity_key(), "id");
    }
}

#[test]
fn only_play_events_are_append_only() {
    assert!(SyncEntity::PlayEvent.is_append_only());
    for entity in SyncEntity::ALL
        .into_iter()
        .filter(|e| *e != SyncEntity::PlayEvent)
    {
        assert!(!entity.is_append_only());
    }
}

#[test]
fn ops_the_store_records_name_entities_the_mapping_recognises() {
    let store = store();
    let now = now_millis();
    let list = store.create_playlist("Focus", None, None, now).unwrap();
    store
        .add_tracks(&[TrackLink::new("t1", "k1")], &list.id, now)
        .unwrap();
    store.create_folder("Mixes", None, now).unwrap();
    store
        .record(Like::new("t1", "k1", true, 1, "phone"), false, now)
        .unwrap();
    store
        .append(PlayEvent::new("t1", "k1", 1, "phone"))
        .unwrap();
    store
        .save_queue("iPhone", &["t1".to_owned()], 0, 0, now)
        .unwrap();

    let names: HashSet<String> = pending(&store).into_iter().map(|op| op.entity).collect();
    assert_eq!(names.len(), 6);
    for name in names {
        assert!(
            SyncEntity::from_local_table(&name).is_some(),
            "unmapped entity '{name}'"
        );
    }
}
