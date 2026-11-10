//! Song tiles: a row on an album's page remembered as its song, resumed in
//! its album, and which tiles can be queued from their menu.

use aoide_melete::Item;
use aoide_store::{PlaySource, RecentContext, RecentContextKind, ResumeTile};

use super::resume::{from_song, resume};
use super::{Msg, from_context, from_song_row, tile_collection};
use crate::actions::{Collection, PlayRequest};
use crate::app::Message;
use crate::playback::Enqueue;
use crate::playback::tracks::TrackMeta;
use crate::store_worker::StoreHandle;

fn item(id: &str, album: &str) -> Item {
    Item {
        id: id.into(),
        name: id.into(),
        album_id: Some(album.into()),
        has_cover: true,
        ..Item::default()
    }
}

#[test]
fn a_row_on_an_albums_page_is_remembered_as_its_song() {
    let mut tracks: Vec<TrackMeta> = ["t1", "t2"]
        .iter()
        .map(|id| TrackMeta::from_item(&item(id, "al1")))
        .collect();
    tracks[1].artist = "Other Artist".into();
    let row = Message::Play(PlayRequest::from(&tracks, 1, PlaySource::Album));
    let Message::Handoff(Msg::PlayFrom(boxed)) = from_song_row(row, "al1", "Low Tide") else {
        panic!("a song context");
    };
    let (request, song) = *boxed;
    assert!(matches!(request.how, Enqueue::Replace { start: 1, .. }));
    assert_eq!(song.key(), "song:t2");
    assert_eq!(song.name, "t2");
    assert_eq!(song.detail(), "Other Artist");
    assert_eq!(song.album_id.as_deref(), Some("al1"));
    assert_eq!(song.artwork_item_id.as_deref(), Some("al1"));

    // The page's own map leaves it a song: Play and Shuffle are not rows.
    let album = RecentContext::new(RecentContextKind::Album, "al1", "Low Tide", 1);
    let first = from_song_row(
        Message::Play(PlayRequest::from(&tracks, 0, PlaySource::Album)),
        "al1",
        "Low Tide",
    );
    let Message::Handoff(Msg::PlayFrom(boxed)) = from_context(first, &album) else {
        panic!("still a play from a context");
    };
    assert_eq!(boxed.1.key(), "song:t1");
    assert_eq!(boxed.1.detail(), "Low Tide", "no artist: the album's name");
    let shuffle = Message::Play(PlayRequest::shuffled(&tracks, PlaySource::Shuffle));
    assert!(matches!(
        from_song_row(shuffle, "al1", "Low Tide"),
        Message::Play(_)
    ));
    let next = Message::Play(PlayRequest {
        tracks,
        how: Enqueue::Next,
        source: PlaySource::Album,
    });
    assert!(matches!(
        from_song_row(next, "al1", "Low Tide"),
        Message::Play(_)
    ));
}

#[test]
fn a_song_resumes_its_album_from_that_song() {
    let mut album = [item("b", "al1"), item("a", "al1"), item("c", "al1")];
    for (n, item) in album.iter_mut().enumerate() {
        item.track_number = Some([2, 1, 3][n]);
    }
    let (tracks, start) = from_song(&album, "b").unwrap();
    let ids: Vec<&str> = tracks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["a", "b", "c"], "in the album's order");
    assert_eq!(start, 1);
    assert!(from_song(&album, "gone").is_none());
}

#[test]
fn albums_songs_and_aoide_playlists_can_be_queued_from_their_tile() {
    let of = |kind| tile_collection(&RecentContext::new(kind, "x", "X", 1));
    assert_eq!(
        of(RecentContextKind::Album),
        Some(Collection::Album("x".into()))
    );
    assert_eq!(
        of(RecentContextKind::Song),
        Some(Collection::Song("x".into()))
    );
    assert_eq!(
        of(RecentContextKind::LocalPlaylist),
        Some(Collection::Playlist("x".into()))
    );
    for kind in [
        RecentContextKind::Station,
        RecentContextKind::JellyfinPlaylist,
        RecentContextKind::Mix,
    ] {
        assert_eq!(of(kind), None, "{kind:?}");
    }
}

/// An album that can't be read plays the song alone.
#[tokio::test(flavor = "multi_thread")]
async fn a_song_whose_album_is_gone_plays_alone() {
    use aoide_melete::{Credentials, Device, MeleteClient, ServerAddress};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/music/albums/9"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/music/tracks"))
        .and(query_param("id", "5"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(crate::test_json::page(vec![
                crate::test_json::track(5, "Lantern"),
            ])),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = MeleteClient::new(
        ServerAddress::parse(&server.uri()).unwrap(),
        Device::new("Test", "device-1"),
    )
    .unwrap()
    .with_credentials(Credentials {
        user_id: "u1".into(),
        access_token: "tok".into(),
        username: "alice".into(),
    });
    let store = StoreHandle::in_memory("device-1").unwrap();
    let song = RecentContext::song("5", "Lantern", "9", 1);
    let found = resume(client, None, store, ResumeTile::Context(song.clone()))
        .await
        .unwrap();
    let ids: Vec<&str> = found.tracks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["5"]);
    assert_eq!(found.start, 0);
    assert_eq!(found.source, PlaySource::Album);
    assert_eq!(found.context, Some(song), "moved to the front again");
}
