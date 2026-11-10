//! For You against a local mock server. Nothing here leaves 127.0.0.1.

use aoide_melete::{
    BecauseKind, Credentials, Device, Error, ForYouState, MeleteClient, RequestKind, ServerAddress,
    Suggestion,
};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const FOR_YOU: &str = "/api/v1/music/for-you";

fn signed_in(server: &MockServer) -> MeleteClient {
    MeleteClient::new(
        ServerAddress::parse(&server.uri()).unwrap(),
        Device::new("Test", "test-device"),
    )
    .unwrap()
    .with_credentials(Credentials {
        user_id: "1".into(),
        access_token: "mlt_fixture-token-not-a-secret".into(),
        username: "alice".into(),
    })
}

fn track() -> Value {
    json!({
        "id": 7, "title": "Harbour Lights", "artist": "The Quiet Engines", "album_id": 3,
        "album": "Low Tide", "album_artist": "The Quiet Engines", "track_number": 2,
        "disc_number": 1, "year": 2019, "duration_seconds": 201.0, "genres": [],
        "codec": "flac", "bitrate_kbps": 900, "sample_rate": 44100, "bit_depth": 16,
        "channels": 2, "spatial": null, "isrc": null, "mb_recording_id": null,
        "mb_track_id": null, "mb_release_id": null, "mb_release_group_id": null,
        "mb_artist_id": null, "gapless": null, "liked": false,
        "artists": [{"id": 4, "name": "The Quiet Engines", "mb_artist_id": null}],
    })
}

fn song() -> Value {
    json!({
        "kind": "song", "mb_recording_id": "rec-1", "title": "Paper Boats",
        "artist": "Northern Static", "mb_artist_ids": ["art-1", "art-2"],
        "release": "Weather Report", "poster": "/api/v1/music/release-groups/rg-1/cover",
    })
}

fn because_song() -> Value {
    json!({"kind": "song", "title": "Harbour Lights", "artist": "The Quiet Engines"})
}

fn because_artist() -> Value {
    json!({"kind": "artist", "title": null, "artist": "The Quiet Engines"})
}

async fn answering(body: Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(FOR_YOU))
        .and(query_param("limit", "40"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn each_list_state_is_read() {
    for (word, state) in [
        ("off", ForYouState::Off),
        ("no_history", ForYouState::NoHistory),
        ("waiting", ForYouState::Waiting),
        ("ready", ForYouState::Ready),
    ] {
        let server = answering(json!({"state": word, "made_at": null, "items": []})).await;
        let list = signed_in(&server).for_you(40).await.unwrap().unwrap();
        assert_eq!(list.state, state, "{word}");
        assert!(list.items.is_empty());
        assert_eq!(list.made_at, None);
    }
}

#[tokio::test]
async fn each_item_state_is_read_with_what_it_came_from() {
    let server = answering(json!({
        "state": "ready", "made_at": 1_790_000_000,
        "items": [
            {"state": "playable", "track": track(), "song": null, "request_id": null,
             "because": because_artist()},
            {"state": "requested", "track": null, "song": song(), "request_id": 12,
             "because": because_song()},
            {"state": "requestable", "track": null, "song": song(), "request_id": null,
             "because": because_song()},
        ],
    }))
    .await;
    let list = signed_in(&server).for_you(40).await.unwrap().unwrap();
    assert_eq!(list.state, ForYouState::Ready);
    assert_eq!(list.made_at, Some(1_790_000_000));
    assert_eq!(list.items.len(), 3);

    let Suggestion::Playable(track) = &list.items[0].suggestion else {
        panic!("not playable: {:?}", list.items[0]);
    };
    assert_eq!(track.id, "7");
    assert_eq!(track.name, "Harbour Lights");
    assert_eq!(track.album_id.as_deref(), Some("3"));
    assert_eq!(list.items[0].because.kind, BecauseKind::Artist);
    assert_eq!(list.items[0].because.seed(), "The Quiet Engines");

    let Suggestion::Requested { song, request_id } = &list.items[1].suggestion else {
        panic!("not requested: {:?}", list.items[1]);
    };
    assert_eq!(*request_id, Some(12));
    assert_eq!(song.mb_recording_id, "rec-1");
    assert_eq!(song.mb_artist_ids, ["art-1", "art-2"]);
    assert_eq!(song.release.as_deref(), Some("Weather Report"));
    assert_eq!(list.items[1].because.kind, BecauseKind::Song);
    assert_eq!(list.items[1].because.seed(), "Harbour Lights");

    let Suggestion::Requestable(song) = &list.items[2].suggestion else {
        panic!("not requestable: {:?}", list.items[2]);
    };
    assert_eq!(song.title, "Paper Boats");
    assert_eq!(
        song.poster.as_deref(),
        Some("/api/v1/music/release-groups/rg-1/cover")
    );
}

#[tokio::test]
async fn a_server_without_for_you_answers_none() {
    let server = MockServer::start().await;
    assert_eq!(signed_in(&server).for_you(40).await.unwrap(), None);
}

#[tokio::test]
async fn another_failure_is_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(FOR_YOU))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let error = signed_in(&server).for_you(40).await.unwrap_err();
    assert!(
        matches!(error, Error::Http { status: 500, .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn absent_optional_fields_read_as_none() {
    let server = answering(json!({
        "state": "ready",
        "items": [
            {"state": "requestable",
             "song": {"kind": "song", "mb_recording_id": "rec-2", "title": "Static",
                      "artist": "Northern Static"},
             "because": {"kind": "artist", "artist": "Northern Static"}},
            {"state": "requested",
             "song": {"kind": "song", "mb_recording_id": "rec-3", "title": "Fog",
                      "artist": "Northern Static", "mb_artist_ids": null},
             "because": {"kind": "song", "title": "Static", "artist": "Northern Static"}},
        ],
    }))
    .await;
    let list = signed_in(&server).for_you(40).await.unwrap().unwrap();
    assert_eq!(list.made_at, None);
    let Suggestion::Requestable(song) = &list.items[0].suggestion else {
        panic!("not requestable: {:?}", list.items[0]);
    };
    assert!(song.mb_artist_ids.is_empty());
    assert_eq!(song.release, None);
    assert_eq!(song.poster, None);
    assert_eq!(list.items[0].because.title, None);
    assert_eq!(list.items[0].because.seed(), "Northern Static");
    let Suggestion::Requested { song, request_id } = &list.items[1].suggestion else {
        panic!("not requested: {:?}", list.items[1]);
    };
    assert_eq!(*request_id, None);
    assert!(song.mb_artist_ids.is_empty());
}

#[tokio::test]
async fn an_item_without_what_its_state_needs_is_left_out() {
    let server = answering(json!({
        "state": "ready", "made_at": 1,
        "items": [
            {"state": "playable", "track": null, "song": null, "request_id": null,
             "because": because_song()},
            {"state": "requestable", "track": null, "song": null, "request_id": null,
             "because": because_song()},
            {"state": "requestable", "track": null, "song": song(), "request_id": null,
             "because": because_song()},
        ],
    }))
    .await;
    let list = signed_in(&server).for_you(40).await.unwrap().unwrap();
    assert_eq!(list.items.len(), 1);
    assert!(matches!(
        list.items[0].suggestion,
        Suggestion::Requestable(_)
    ));
}

#[tokio::test]
async fn a_song_is_asked_for_by_its_recording() {
    let server = answering(json!({
        "state": "ready", "made_at": 1,
        "items": [{"state": "requestable", "track": null, "song": song(),
                   "request_id": null, "because": because_song()}],
    }))
    .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/requests"))
        .and(body_json(
            json!({"kind": "song", "mb_recording_id": "rec-1"}),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": 31, "user_id": 1, "username": "alice", "kind": "song", "service": "music",
            "tmdb_id": null, "season": null, "mb_artist_id": "art-1",
            "mb_release_group_id": null, "mb_recording_id": "rec-1", "title": "Paper Boats",
            "year": null, "artist": "Northern Static", "poster": null, "state": "requested",
            "reason": null, "created_at": 1_790_000_000, "decided_at": null,
            "decided_by": null, "wanted_id": null, "progress": null, "seasons": null,
            "search_started": false,
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = signed_in(&server);
    let list = client.for_you(40).await.unwrap().unwrap();
    let Suggestion::Requestable(song) = &list.items[0].suggestion else {
        panic!("not requestable: {:?}", list.items[0]);
    };
    let candidate = song.candidate();
    assert_eq!(candidate.kind, RequestKind::Song);
    assert!(candidate.askable());
    let request = client.ask_for(&candidate).await.unwrap();
    assert_eq!(request.id, 31);
    assert_eq!(request.kind, RequestKind::Song);
}
