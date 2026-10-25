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

// MARK: - Kinds, servers and the file (desktop `recent-contexts.test.ts`)

mod grid {
    use crate::{
        QueueFreshness, RESUME_GRID_LIMIT, RecentContext, RecentContextKind, RecentContextList,
        RecentContexts, ResumableQueue, ResumeTile, StationSeed, new_id, resume_tiles,
    };

    use super::album;

    const NOW: i64 = 1_000_000;

    fn handoff(age_seconds: i64) -> ResumableQueue {
        ResumableQueue {
            device_id: "phone-2".to_owned(),
            device_name: "Phone".to_owned(),
            track_ids: vec!["t1".to_owned(), "t2".to_owned()],
            position: 1,
            elapsed_ms: 0,
            age_ms: age_seconds * 1000,
            observed_at: NOW,
            freshness: QueueFreshness::Server,
        }
    }

    fn kinds(tiles: &[ResumeTile]) -> Vec<&'static str> {
        tiles
            .iter()
            .map(|tile| match tile {
                ResumeTile::Handoff(_) => "handoff",
                ResumeTile::Context(_) => "context",
            })
            .collect()
    }

    fn recent() -> Vec<RecentContext> {
        vec![album("a", NOW - 60_000), album("b", NOW - 120_000)]
    }

    #[test]
    fn the_grid_shows_six_at_most_and_nothing_for_nothing() {
        assert_eq!(RESUME_GRID_LIMIT, 6);
        const { assert!(RecentContextList::CAPACITY > RESUME_GRID_LIMIT) };
        let many: Vec<RecentContext> = (0..10).map(|i| album(&i.to_string(), i)).collect();
        assert_eq!(resume_tiles(&many, None, RESUME_GRID_LIMIT).len(), 6);
        assert!(resume_tiles(&[], None, RESUME_GRID_LIMIT).is_empty());
        assert_eq!(
            kinds(&resume_tiles(&recent(), None, 6)),
            ["context", "context"]
        );
    }

    /// Another device that played thirty seconds ago beats an album a minute ago.
    #[test]
    fn another_device_leads_when_it_played_more_recently_than_anything_here() {
        let tiles = resume_tiles(&recent(), Some(&handoff(30)), 6);
        assert_eq!(kinds(&tiles), ["handoff", "context", "context"]);
        assert_eq!(tiles[0].title(), "Pick up from Phone");
        assert_eq!(tiles[0].detail(), "2 tracks");
        assert_eq!(tiles[0].artwork_item_id(), Some("t2"));
    }

    #[test]
    fn another_device_is_left_out_when_this_one_played_since_or_on_a_tie() {
        for age in [90, 60] {
            let tiles = resume_tiles(&recent(), Some(&handoff(age)), 6);
            assert_eq!(kinds(&tiles), ["context", "context"], "age {age}");
        }
    }

    #[test]
    fn another_device_leads_when_nothing_was_played_here() {
        let tiles = resume_tiles(&[], Some(&handoff(3_600)), 6);
        assert_eq!(kinds(&tiles), ["handoff"]);
    }

    #[test]
    fn still_six_at_most_with_a_handoff_in_front() {
        let many: Vec<RecentContext> = (0..10).map(|i| album(&i.to_string(), i)).collect();
        let tiles = resume_tiles(&many, Some(&handoff(0)), 6);
        assert_eq!(tiles.len(), 6);
        assert_eq!(kinds(&tiles)[0], "handoff");
        let ResumeTile::Context(first) = &tiles[1] else {
            panic!("a context");
        };
        assert_eq!(first.id, "9", "newest first");
    }

    #[test]
    fn tiles_say_what_they_are() {
        let tile = ResumeTile::Context(album("x", 1));
        assert_eq!(tile.detail(), "Album");
        assert_eq!(tile.artwork_item_id(), Some("x"), "an album draws itself");
        let mut playlist = RecentContext::new(RecentContextKind::LocalPlaylist, "p", "Late", 1);
        playlist.smart = Some(true);
        assert_eq!(ResumeTile::Context(playlist.clone()).detail(), "Playlist");
        assert_eq!(ResumeTile::Context(playlist).artwork_item_id(), None);
        let mut subtitled = album("y", 1);
        subtitled.subtitle = Some("Radiohead".to_owned());
        assert_eq!(ResumeTile::Context(subtitled).detail(), "Radiohead");
    }

    /// A mix is its description; a station remembers what seeded it.
    #[test]
    fn mixes_and_stations_carry_what_resumes_them() {
        let mix = RecentContext::mix("  rainy sunday jazz ", 5);
        assert_eq!(mix.kind, RecentContextKind::Mix);
        assert_eq!(mix.id, "rainy sunday jazz");
        assert_eq!(mix.name, "rainy sunday jazz");
        let station = RecentContext::station(StationSeed::Artist, "artist-1", "Björk", 6);
        assert_eq!(station.seed, Some(StationSeed::Artist));
        assert_eq!(station.key(), "station:artist-1");
    }

    /// The phone's and the desktop's spelling of an Aoide playlist are one kind.
    #[test]
    fn the_desktops_spelling_of_an_aoide_playlist_reads_as_a_local_playlist() {
        let read: RecentContext = serde_json::from_str(
            r#"{"kind":"aoidePlaylist","id":"p","name":"Late","smart":true,"lastOpened":3}"#,
        )
        .unwrap();
        assert_eq!(read.kind, RecentContextKind::LocalPlaylist);
        assert_eq!(read.smart, Some(true));
        let written = serde_json::to_string(&read).unwrap();
        assert!(written.contains(r#""kind":"localPlaylist""#), "{written}");
    }

    fn ids(all: &RecentContexts, server: &str) -> Vec<String> {
        all.list(server).iter().map(|c| c.id.clone()).collect()
    }

    /// An album id means nothing on another server.
    #[test]
    fn each_server_keeps_its_own_list() {
        let mut all = RecentContexts::default();
        all.remember("server-a", album("1", 1));
        all.remember("server-a", album("2", 2));
        all.remember("server-b", album("9", 3));
        assert_eq!(ids(&all, "server-a"), ["2", "1"]);
        assert_eq!(ids(&all, "server-b"), ["9"]);
        assert!(all.list("server-c").is_empty());
        assert_eq!(all.latest("server-a", 1).len(), 1);

        all.forget("server-a", &album("2", 0).key());
        assert_eq!(ids(&all, "server-a"), ["1"]);
        all.forget_server("server-b");
        assert!(all.list("server-b").is_empty());
    }

    #[test]
    fn a_context_with_nothing_to_resume_or_say_is_not_kept() {
        let mut all = RecentContexts::default();
        all.remember(
            "s",
            RecentContext::new(RecentContextKind::Album, "", "Name", 1),
        );
        all.remember(
            "s",
            RecentContext::new(RecentContextKind::Album, "id", " ", 1),
        );
        all.remember("", album("x", 1));
        assert_eq!(all, RecentContexts::default());
    }

    #[test]
    fn the_list_survives_a_restart_and_a_bad_file_is_an_empty_one() {
        let path = std::env::temp_dir().join(format!("aoide-recents-{}.json", new_id()));
        let mut all = RecentContexts::default();
        all.remember("s", album("a", 1));
        all.remember(
            "s",
            RecentContext::station(StationSeed::Album, "al", "Kid A", 2),
        );
        all.save(&path).unwrap();
        assert_eq!(RecentContexts::load(&path), all);

        std::fs::write(&path, b"not json").unwrap();
        assert_eq!(RecentContexts::load(&path), RecentContexts::default());
        std::fs::remove_file(&path).unwrap();
        assert_eq!(RecentContexts::load(&path), RecentContexts::default());
    }
}

// MARK: - Built-ins created once (desktop `playlists.test.ts` "built-in playlists")

mod ensured {
    use super::super::{pending, store};
    use crate::{
        BuiltInPlaylist, Calendar, Millis, PlayEvent, PlaySource, Record, Store, TrackRef,
    };

    #[test]
    fn both_are_created_under_their_fixed_ids_with_exactly_the_pinned_rules() {
        let store = store();
        let created = store.ensure_built_in_playlists(1_000).unwrap();
        assert_eq!(created, BuiltInPlaylist::ALL);
        for built in BuiltInPlaylist::ALL {
            let row = store.playlist(built.id()).unwrap().unwrap();
            assert_eq!(row.name, built.name());
            assert_eq!(row.notes.as_deref(), Some(built.notes()));
            assert!(row.is_smart);
            assert_eq!(
                row.smart_rules.as_deref(),
                Some(built.rules().to_json().as_str())
            );
            assert_eq!(row.smart_rule_set().unwrap(), Some(built.rules()));
        }
    }

    #[test]
    fn they_reach_the_other_devices_as_two_playlist_ops_each() {
        let store = store();
        store.ensure_built_in_playlists(1_000).unwrap();
        let ops = pending(&store);
        assert_eq!(ops.len(), BuiltInPlaylist::ALL.len() * 2);
        assert!(ops.iter().all(|op| op.entity == "playlist"));
        let mut ids: Vec<&str> = ops.iter().map(|op| op.entity_id.as_str()).collect();
        ids.dedup();
        assert_eq!(ids, ["builtin:rediscover", "builtin:heavy-rotation"]);
    }

    #[test]
    fn nothing_is_created_the_second_time() {
        let store = store();
        store.ensure_built_in_playlists(1_000).unwrap();
        let ops: Vec<String> = pending(&store).into_iter().map(|op| op.op_id).collect();
        store.mark_synced(&ops).unwrap();
        assert!(store.ensure_built_in_playlists(2_000).unwrap().is_empty());
        assert!(pending(&store).is_empty());
        assert_eq!(store.playlists().unwrap().len(), BuiltInPlaylist::ALL.len());
    }

    /// A built-in removed on any device stays removed on all of them.
    #[test]
    fn a_deleted_built_in_stays_deleted() {
        let store = store();
        store.ensure_built_in_playlists(1_000).unwrap();
        store
            .delete_playlist(BuiltInPlaylist::Rediscover.id(), 1_500)
            .unwrap();
        let ops: Vec<String> = pending(&store).into_iter().map(|op| op.op_id).collect();
        store.mark_synced(&ops).unwrap();
        assert!(store.ensure_built_in_playlists(2_000).unwrap().is_empty());
        assert!(pending(&store).is_empty());
        assert_eq!(
            store.playlist(BuiltInPlaylist::Rediscover.id()).unwrap(),
            None
        );
    }

    /// One the phone made and sync brought is the same row, not a twin.
    #[test]
    fn one_another_device_created_is_not_made_again() {
        let phone = store();
        phone.ensure_built_in_playlists(1_000).unwrap();
        let desk = Store::in_memory("desk").unwrap();
        let row = phone
            .playlist(BuiltInPlaylist::HeavyRotation.id())
            .unwrap()
            .unwrap();
        desk.write_local(|tx| row.insert(tx)).unwrap();
        assert_eq!(
            desk.ensure_built_in_playlists(2_000).unwrap(),
            [BuiltInPlaylist::Rediscover]
        );
        assert_eq!(desk.playlists().unwrap().len(), 2);
    }

    const NOW: Millis = 1_700_000_000_000;
    const DAY: Millis = 86_400_000;

    fn listen(store: &Store, track: &str, days_ago: i64) {
        let started = NOW - days_ago * DAY;
        store
            .append(
                PlayEvent::new(track, track, started, "phone")
                    .with_source(PlaySource::Album)
                    .finished(Some(started + 200_000), 200_000, true, false),
            )
            .unwrap();
    }

    /// The rules do what their names promise, evaluated here rather than synced as
    /// a snapshot.
    #[test]
    fn rediscover_finds_old_favourites_and_heavy_rotation_this_months() {
        let store = store();
        for id in ["old-favourite", "this-month", "once", "never"] {
            let mut row = TrackRef::new(id, id, id, "Artist", "Album", NOW);
            row.duration_ms = Some(200_000);
            store.cache_tracks(&[row]).unwrap();
        }
        for days in [300, 290, 280] {
            listen(&store, "old-favourite", days);
        }
        for days in [2, 9] {
            listen(&store, "this-month", days);
        }
        listen(&store, "once", 400);
        store.ensure_built_in_playlists(NOW).unwrap();

        let resolve = |built: BuiltInPlaylist| {
            let row = store.playlist(built.id()).unwrap().unwrap();
            store.resolve_smart(&row, NOW, Calendar::Utc).unwrap()
        };
        assert_eq!(resolve(BuiltInPlaylist::Rediscover), ["old-favourite"]);
        assert_eq!(resolve(BuiltInPlaylist::HeavyRotation), ["this-month"]);
    }
}
