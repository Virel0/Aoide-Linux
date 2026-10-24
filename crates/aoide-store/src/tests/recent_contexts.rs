//! Port of `RecentContextsTests.swift` ("Recent contexts" and "Built-in playlists").

use std::collections::HashSet;

use super::store;
use crate::{
    BuiltInPlaylist, Millis, RecentContext, RecentContextKind, RecentContextList, now_millis,
};

fn album(id: &str, at: Millis) -> RecentContext {
    RecentContext::new(RecentContextKind::Album, id, format!("Album {id}"), at)
}

fn ids(list: &[RecentContext]) -> Vec<&str> {
    list.iter().map(|context| context.id.as_str()).collect()
}

#[test]
fn newest_first_and_opening_something_again_moves_it_to_the_front() {
    let mut list = RecentContextList::default();
    list.remember(album("a", 1));
    list.remember(album("b", 2));
    list.remember(album("a", 3));
    assert_eq!(ids(list.contexts()), ["a", "b"]);
    assert_eq!(list.contexts()[0].last_opened, 3);
}

#[test]
fn the_same_id_under_a_different_kind_is_a_different_thing() {
    let mut list = RecentContextList::default();
    list.remember(album("x", 1));
    list.remember(RecentContext::new(
        RecentContextKind::JellyfinPlaylist,
        "x",
        "List",
        2,
    ));
    assert_eq!(list.contexts().len(), 2);
}

#[test]
fn bounded_dropping_the_oldest() {
    let mut list = RecentContextList::default();
    for i in 0..RecentContextList::CAPACITY + 5 {
        list.remember(album(&i.to_string(), i as Millis));
    }
    assert_eq!(list.contexts().len(), RecentContextList::CAPACITY);
    assert_eq!(
        list.contexts()[0].id,
        (RecentContextList::CAPACITY + 4).to_string()
    );
    assert_eq!(list.contexts().last().unwrap().id, "5");
}

#[test]
fn latest_takes_from_the_front() {
    let mut list = RecentContextList::default();
    for i in 0..10 {
        list.remember(album(&i.to_string(), i));
    }
    assert_eq!(ids(list.latest(6)), ["9", "8", "7", "6", "5", "4"]);
    assert!(list.latest(0).is_empty());
}

// MARK: - Built-in playlists

#[test]
fn built_in_ids_are_fixed_and_never_collide() {
    let ids: Vec<&str> = BuiltInPlaylist::ALL.iter().map(|p| p.id()).collect();
    assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
    assert!(ids.iter().all(|id| id.starts_with("builtin:")));
}

/// Pinned as sorted-key JSON, so every client carries the same rules verbatim.
#[test]
fn built_in_rules_are_exactly_the_agreed_ones() {
    for playlist in BuiltInPlaylist::ALL {
        let expected = match playlist {
            BuiltInPlaylist::Rediscover => {
                r#"{"limit":40,"match":"all","rules":[{"field":"last_played","op":"notInTheLast","value":"-6m"},{"field":"play_count","op":"greaterThan","value":2}],"sort":{"dir":"desc","field":"play_count"}}"#
            }
            BuiltInPlaylist::HeavyRotation => {
                r#"{"limit":25,"match":"all","rules":[{"field":"last_played","op":"inTheLast","value":"-1m"},{"field":"play_count","op":"greaterThan","value":1}],"sort":{"dir":"desc","field":"play_count"}}"#
            }
        };
        assert_eq!(playlist.rules().to_json(), expected);
    }
}

#[test]
fn a_built_in_is_created_under_its_own_id_once_and_not_again_after_deletion() {
    let store = store();
    let built = BuiltInPlaylist::Rediscover;
    assert!(!store.playlist_row_exists(built.id()).unwrap());
    let created = store
        .create_playlist_with_id(
            built.id(),
            built.name(),
            None,
            Some(built.notes()),
            now_millis(),
        )
        .unwrap();
    assert_eq!(created.id, built.id());
    assert!(store.playlist_row_exists(built.id()).unwrap());
    store.delete_playlist(built.id(), now_millis()).unwrap();
    assert_eq!(store.playlist(built.id()).unwrap(), None);
    assert!(
        store.playlist_row_exists(built.id()).unwrap(),
        "a deleted built-in must not be recreated"
    );
}
