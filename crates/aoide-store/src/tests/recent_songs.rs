//! Port of `RecentContextsTests.swift`'s song tests: a song picked from an album
//! shares the album's place, and lists saved before songs were kept still read.

use crate::{RecentContext, RecentContextKind, RecentContextList, ResumeTile, StationSeed};

fn album(id: &str, at: crate::Millis) -> RecentContext {
    RecentContext::new(RecentContextKind::Album, id, format!("Album {id}"), at)
}

fn keys(list: &RecentContextList) -> Vec<String> {
    list.contexts().iter().map(RecentContext::key).collect()
}

/// One album is one tile: played whole, or from one of its songs, whichever was last.
#[test]
fn an_album_and_a_song_picked_from_it_share_one_place_the_latest_ones() {
    let mut list = RecentContextList::default();
    list.remember(album("a", 1));
    list.remember(RecentContext::song("s1", "Lantern", "a", 2));
    assert_eq!(keys(&list), ["song:s1"]);
    list.remember(RecentContext::song("s2", "Saltwater", "a", 3));
    list.remember(RecentContext::song("s9", "Elsewhere", "b", 4));
    assert_eq!(keys(&list), ["song:s9", "song:s2"]);
    list.remember(album("a", 5));
    assert_eq!(keys(&list), ["album:a", "song:s9"]);
    // A song whose album isn't known stands alone.
    list.remember(RecentContext::new(
        RecentContextKind::Song,
        "s0",
        "Loose",
        6,
    ));
    assert_eq!(list.contexts().len(), 3);
}

/// A station seeded from an album is not that album: both keep their tiles.
#[test]
fn a_station_from_an_album_does_not_take_the_albums_place() {
    let mut list = RecentContextList::default();
    list.remember(album("a", 1));
    list.remember(RecentContext::station(
        StationSeed::Album,
        "a",
        "Album a Radio",
        2,
    ));
    list.remember(RecentContext::song("s1", "Lantern", "a", 3));
    assert_eq!(keys(&list), ["song:s1", "station:a"]);
}

/// Lists written before songs were kept have no `albumId`, and still read.
#[test]
fn a_list_saved_before_songs_and_their_albums_were_kept_still_reads() {
    let saved = r#"{"contexts":[{"kind":"album","id":"a","name":"Album a","subtitle":"An Artist","lastOpened":1}]}"#;
    let list: RecentContextList = serde_json::from_str(saved).unwrap();
    let first = &list.contexts()[0];
    assert_eq!(first.kind, RecentContextKind::Album);
    assert_eq!(first.album_id, None);
    assert_eq!(first.subtitle.as_deref(), Some("An Artist"));
    let written = serde_json::to_string(&list).unwrap();
    assert!(!written.contains("albumId"), "{written}");
}

/// A song is written with its album, under the phone's spelling, and read back whole.
#[test]
fn a_song_keeps_its_album_on_disk() {
    let song = RecentContext::song("s1", "Lantern", "a", 2);
    let written = serde_json::to_string(&song).unwrap();
    assert!(written.contains(r#""kind":"song""#), "{written}");
    assert!(written.contains(r#""albumId":"a""#), "{written}");
    let read: RecentContext = serde_json::from_str(&written).unwrap();
    assert_eq!(read, song);
}

/// A song tile names the song and its artist, and draws its album's cover.
#[test]
fn a_song_tile_says_the_song_and_draws_its_album() {
    let mut song = RecentContext::song("s1", "Lantern", "al1", 1);
    assert_eq!(ResumeTile::Context(song.clone()).detail(), "Song");
    song.subtitle = Some("Other Artist".to_owned());
    let tile = ResumeTile::Context(song.clone());
    assert_eq!(tile.title(), "Lantern");
    assert_eq!(tile.detail(), "Other Artist");
    assert_eq!(tile.artwork_item_id(), Some("al1"), "the album's, not s1's");
    song.artwork_item_id = None;
    assert_eq!(ResumeTile::Context(song).artwork_item_id(), Some("al1"));
}
