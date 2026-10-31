//! Asking for music against a local mock server. Nothing here leaves 127.0.0.1.

use aoide_melete::{
    Credentials, Device, Error, MeleteClient, RequestKind, RequestState, ServerAddress,
};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

fn album(in_library: bool, requested: Option<i64>) -> Value {
    json!({
        "kind": "album", "tmdb_id": null, "mb_artist_id": "a-1",
        "mb_release_group_id": "rg-1", "mb_recording_id": null,
        "title": "Low Tide", "year": 2019, "artist": "The Quiet Engines",
        "detail": "Album", "in_library": in_library, "wanted": false,
        "requested": requested, "poster": null,
    })
}

fn request(state: &str, progress: Value) -> Value {
    json!({
        "id": 9, "user_id": 1, "username": "alice", "kind": "album", "service": "music",
        "tmdb_id": null, "season": null, "mb_artist_id": "a-1",
        "mb_release_group_id": "rg-1", "mb_recording_id": null, "title": "Low Tide",
        "year": 2019, "artist": "The Quiet Engines", "poster": null, "state": state,
        "reason": null, "created_at": 1_790_000_000, "decided_at": null,
        "decided_by": null, "wanted_id": null, "progress": progress, "seasons": null,
        "search_started": false,
    })
}

#[tokio::test]
async fn a_search_sends_its_kind_and_words_and_reads_what_is_askable() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/requests/search"))
        .and(query_param("kind", "album"))
        .and(query_param("q", "low tide"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([album(false, None), album(true, None)])),
        )
        .mount(&server)
        .await;
    let found = signed_in(&server)
        .request_search(RequestKind::Album, " low tide ")
        .await
        .unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].mb_release_group_id.as_deref(), Some("rg-1"));
    assert!(found[0].askable());
    assert!(!found[1].askable(), "in the library already");
}

#[tokio::test]
async fn asking_sends_only_the_id_of_its_kind() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/requests"))
        .and(body_json(
            json!({"kind": "album", "mb_release_group_id": "rg-1"}),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(request("requested", Value::Null)))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/requests"))
        .respond_with(
            ResponseTemplate::new(409)
                .set_body_json(json!({"error": "you've asked for it already"})),
        )
        .mount(&server)
        .await;
    let client = signed_in(&server);
    let found: aoide_melete::Candidate = serde_json::from_value(album(false, None)).unwrap();
    let asked = client.ask_for(&found).await.unwrap();
    assert_eq!((asked.id, asked.state), (9, RequestState::Requested));
    let mut artist = found.clone();
    artist.kind = RequestKind::Artist;
    assert!(matches!(
        client.ask_for(&artist).await,
        Err(Error::Http { status: 409, .. })
    ));
}

#[tokio::test]
async fn the_list_is_music_only_and_reads_progress() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/requests"))
        .and(query_param("service", "music"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "total": 1,
            "items": [request("approved", json!({
                "kind": "album", "wanted_id": 3, "title": "Low Tide by The Quiet Engines",
                "step": "downloading", "percent": 42.5, "message": "Downloading",
                "download_id": 7, "source": "torrent", "updated_at": 1_790_000_100,
                "copied": null,
            }))],
        })))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/requests/9"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let client = signed_in(&server);
    let requests = client.music_requests().await.unwrap();
    let progress = requests[0].progress.as_ref().unwrap();
    assert_eq!(
        (requests[0].state, progress.step.as_str(), progress.percent),
        (RequestState::Approved, "downloading", Some(42.5))
    );
    client.withdraw_request(9).await.unwrap();
}
