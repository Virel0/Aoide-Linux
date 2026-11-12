//! Merging one playlist into another: order, repeats, waiting songs, a smart
//! source, and what is refused.

use super::{links, pending, store};
use crate::{
    Calendar, Field, Match, MergeReport, Operator, PendingSong, PlaylistError, PlaylistItem,
    SmartRule, SmartRules, Sort, SortDirection, Store, StoreError, TrackRef,
};

const NOW: i64 = 1_700_000_000_000;

/// Each entry as its track id, or `wait:` and its request (or title).
fn shape(store: &Store, playlist: &str) -> Vec<String> {
    store
        .items(playlist)
        .unwrap()
        .iter()
        .map(|item: &PlaylistItem| match &item.pending {
            Some(song) => match song.request_id {
                Some(id) => format!("wait:{id}"),
                None => format!("wait:{}", song.title),
            },
            None => item.jellyfin_id.clone(),
        })
        .collect()
}

fn waiting(request: Option<i64>, title: &str, artist: &str) -> PendingSong {
    PendingSong {
        request_id: request,
        title: title.to_owned(),
        artist: Some(artist.to_owned()),
        album: None,
        duration_ms: None,
        why: request.is_none().then(|| "Not found".to_owned()),
    }
}

#[test]
fn songs_go_on_the_end_in_the_sources_order_and_repeats_are_left_out() {
    let store = store();
    let source = store.create_playlist("Old", None, None, 1).unwrap();
    let target = store.create_playlist("New", None, None, 1).unwrap();
    store
        .add_tracks(&links(&["c", "a", "d", "c", "b", "e"]), &source.id, 1)
        .unwrap();
    store
        .add_tracks(&links(&["a", "b"]), &target.id, 1)
        .unwrap();

    let report = store
        .merge_playlist(&source.id, &target.id, 2, Calendar::Utc)
        .unwrap();
    assert_eq!(
        report,
        MergeReport {
            tracks: 3,
            waiting: 0,
            skipped: 3
        }
    );
    assert_eq!(shape(&store, &target.id), ["a", "b", "c", "d", "e"]);
    assert_eq!(shape(&store, &source.id), ["c", "a", "d", "c", "b", "e"]);
    // The content key comes along, so the copy relinks as the original would.
    let copied = store.items(&target.id).unwrap();
    assert_eq!(copied[2].content_key, "key-c");
}

#[test]
fn every_entry_added_is_an_op_to_push_and_the_source_is_untouched() {
    let store = store();
    let source = store.create_playlist("Old", None, None, 1).unwrap();
    let target = store.create_playlist("New", None, None, 1).unwrap();
    store
        .add_tracks(&links(&["a", "b", "c"]), &source.id, 1)
        .unwrap();
    store.add_tracks(&links(&["b"]), &target.id, 1).unwrap();
    let before = pending(&store).len();

    let report = store
        .merge_playlist(&source.id, &target.id, 2, Calendar::Utc)
        .unwrap();
    let ops = pending(&store);
    assert_eq!(ops.len(), before + report.added());
    assert!(ops[before..].iter().all(|op| op.entity == "playlistItem"));
    assert!(store.playlist(&source.id).unwrap().is_some(), "kept");
}

#[test]
fn waiting_songs_come_along_in_place_unless_the_target_waits_on_them() {
    let store = store();
    let source = store.create_playlist("Old", None, None, 1).unwrap();
    let target = store.create_playlist("New", None, None, 1).unwrap();
    store.add_tracks(&links(&["x"]), &source.id, 1).unwrap();
    store
        .add_waiting(
            &[
                waiting(Some(5), "Airbag", "Radiohead"),
                waiting(Some(6), "Lucky", "Radiohead"),
            ],
            &source.id,
            1,
        )
        .unwrap();
    store.add_tracks(&links(&["y"]), &source.id, 1).unwrap();
    store
        .add_waiting(
            &[
                waiting(None, "Lost", "Nobody"),
                // The same song, written another way, with no request to tell them apart.
                waiting(None, " lost", "NOBODY"),
            ],
            &source.id,
            1,
        )
        .unwrap();
    store
        .add_waiting(&[waiting(Some(5), "Airbag", "Radiohead")], &target.id, 1)
        .unwrap();

    let report = store
        .merge_playlist(&source.id, &target.id, 2, Calendar::Utc)
        .unwrap();
    assert_eq!((report.tracks, report.waiting, report.skipped), (2, 2, 2));
    assert_eq!(
        shape(&store, &target.id),
        ["wait:5", "x", "wait:6", "y", "wait:Lost"]
    );
    let lost = store.items(&target.id).unwrap().pop().unwrap();
    assert!(lost.is_waiting());
    assert_eq!(lost.pending.unwrap().why.as_deref(), Some("Not found"));
}

#[test]
fn a_smart_source_gives_what_its_rules_match_now() {
    let store = store();
    let rows =
        ["b", "a", "c"].map(|id| TrackRef::new(id, format!("key-{id}"), id, "Art", "Al", NOW));
    store.cache_tracks(&rows).unwrap();
    let by_title = SmartRules::new(
        Match::All,
        vec![SmartRule::new(Field::Title, Operator::IsNot, "c").unwrap()],
        Some(Sort::new(Field::Title, SortDirection::Asc)),
        None,
    )
    .unwrap();
    let smart = store
        .create_smart_playlist("Smart", None, &by_title, 1)
        .unwrap();
    let target = store.create_playlist("New", None, None, 1).unwrap();
    store.add_tracks(&links(&["b"]), &target.id, 1).unwrap();

    let report = store
        .merge_playlist(&smart.id, &target.id, NOW, Calendar::Utc)
        .unwrap();
    assert_eq!((report.tracks, report.waiting, report.skipped), (1, 0, 1));
    assert_eq!(shape(&store, &target.id), ["b", "a"]);
    assert_eq!(store.items(&target.id).unwrap()[1].content_key, "key-a");

    // A copy, not a link: what the rules match later doesn't change it.
    store
        .cache_tracks(&[TrackRef::new("0", "key-0", "0", "Art", "Al", NOW)])
        .unwrap();
    assert_eq!(shape(&store, &target.id), ["b", "a"]);
}

#[test]
fn a_smart_target_or_the_source_itself_is_refused_and_nothing_is_written() {
    let store = store();
    let source = store.create_playlist("Old", None, None, 1).unwrap();
    store.add_tracks(&links(&["a"]), &source.id, 1).unwrap();
    let rules = SmartRules::new(Match::All, vec![], None, None).unwrap();
    let smart = store
        .create_smart_playlist("Smart", None, &rules, 1)
        .unwrap();
    let before = pending(&store).len();

    let refused = |target: &str| store.merge_playlist(&source.id, target, 2, Calendar::Utc);
    assert!(matches!(
        refused(&smart.id),
        Err(StoreError::Playlist(PlaylistError::SmartTarget(id))) if id == smart.id
    ));
    assert!(matches!(
        refused(&source.id),
        Err(StoreError::Playlist(PlaylistError::MergeIntoItself))
    ));
    assert!(matches!(
        refused("gone"),
        Err(StoreError::Playlist(PlaylistError::NoSuchPlaylist(_)))
    ));
    assert_eq!(pending(&store).len(), before);
}

#[test]
fn nothing_new_adds_nothing() {
    let store = store();
    let source = store.create_playlist("Old", None, None, 1).unwrap();
    let target = store.create_playlist("New", None, None, 1).unwrap();
    store
        .add_tracks(&links(&["a", "b"]), &source.id, 1)
        .unwrap();
    store
        .add_tracks(&links(&["b", "a", "z"]), &target.id, 1)
        .unwrap();
    let before = pending(&store).len();

    let report = store
        .merge_playlist(&source.id, &target.id, 2, Calendar::Utc)
        .unwrap();
    assert_eq!(report.added(), 0);
    assert_eq!(report.skipped, 2);
    assert_eq!(pending(&store).len(), before);
}
