//! Asking for music against a local mock server. Nothing here leaves 127.0.0.1.

use aoide_melete::{
    CoverSize, Credentials, Device, Error, MeleteClient, PosterCredit, RequestKind, RequestState,
    ServerAddress,
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
async fn a_result_carries_its_picture_and_a_portraits_credit() {
    let server = MockServer::start().await;
    let mut cover = album(false, None);
    cover["poster"] = json!("/api/v1/music/release-groups/rg-1/cover");
    let artist = json!({
        "kind": "artist", "mb_artist_id": "a-1", "mb_release_group_id": null,
        "mb_recording_id": null, "title": "The Quiet Engines", "year": null, "artist": null,
        "detail": "Group, GB", "in_library": false, "wanted": false, "requested": null,
        "poster": "/api/v1/music/artist-portraits/7",
        "poster_credit": {
            "author": "A. Photographer", "licence": "CC BY-SA 4.0",
            "licence_url": "https://creativecommons.org/licenses/by-sa/4.0/",
            "source_url": "https://commons.wikimedia.org/wiki/File:Engines.jpg",
        },
        "closeness": 100,
    });
    // A server before 0.8.0 sends neither field.
    let mut older = album(false, None);
    older.as_object_mut().unwrap().remove("poster");
    let mut sparse = artist.clone();
    sparse["poster_credit"] = json!({ "author": null, "licence": "CC0" });
    Mock::given(method("GET"))
        .and(path("/api/v1/requests/search"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([cover, artist, older, sparse])),
        )
        .mount(&server)
        .await;
    let client = signed_in(&server);
    let found = client
        .request_search(RequestKind::Artist, "engines")
        .await
        .unwrap();
    assert_eq!(
        found[0].poster.as_deref(),
        Some("/api/v1/music/release-groups/rg-1/cover")
    );
    assert_eq!(found[0].poster_credit, None);
    assert_eq!(
        found[1].poster_credit,
        Some(PosterCredit {
            author: Some("A. Photographer".into()),
            licence: Some("CC BY-SA 4.0".into()),
            licence_url: Some("https://creativecommons.org/licenses/by-sa/4.0/".into()),
            source_url: Some("https://commons.wikimedia.org/wiki/File:Engines.jpg".into()),
        })
    );
    assert_eq!((&found[2].poster, &found[2].poster_credit), (&None, &None));
    assert_eq!(
        found[3].poster_credit,
        Some(PosterCredit {
            licence: Some("CC0".into()),
            ..PosterCredit::default()
        })
    );
    let url = client
        .poster_url(found[1].poster.as_deref().unwrap(), CoverSize::W160)
        .unwrap();
    assert_eq!(
        url.as_str(),
        format!("{}/api/v1/music/artist-portraits/7?size=160", server.uri())
    );
    assert_eq!(
        client.poster_url("https://elsewhere.example/a.jpg", CoverSize::W160),
        None
    );
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

#[tokio::test]
async fn the_feed_reads_each_requests_event_and_ends_with_the_stream() {
    let server = MockServer::start().await;
    let mut copying = request(
        "approved",
        json!({
            "kind": "album", "wanted_id": 3, "title": "Low Tide by The Quiet Engines",
            "step": "transferring", "percent": null, "message": "Copying to the server",
            "download_id": 7, "source": "soulseek", "updated_at": 1_790_000_200,
            "copied": {"bytes": 50, "total": 200, "bytes_per_second": 10, "seconds_left": 15},
        }),
    );
    copying["decided_at"] = json!(1_790_000_050);
    copying["decided_by"] = json!("admin");
    let mut movie = request("requested", Value::Null);
    movie["kind"] = json!("movie");
    let body = format!(
        ": keep-alive\n\nevent: sync\ndata: {{}}\n\nevent: requests\ndata: {}\n\n",
        json!([copying, movie])
    );
    Mock::given(method("GET"))
        .and(path("/api/v1/requests/events"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    let mut feed = signed_in(&server).follow_requests().await.unwrap();
    let requests = feed.next().await.unwrap().unwrap();
    assert_eq!(requests.len(), 1, "the movie is left out");
    assert_eq!(requests[0].decided_by.as_deref(), Some("admin"));
    let progress = requests[0].progress.as_ref().unwrap();
    assert_eq!(progress.source.as_deref(), Some("soulseek"));
    assert_eq!(progress.copied.as_ref().unwrap().seconds_left, Some(15));
    assert_eq!(feed.next().await.unwrap(), None);
}

#[tokio::test]
async fn a_refused_feed_is_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/requests/events"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"error": "sign in"})))
        .mount(&server)
        .await;
    assert!(signed_in(&server).follow_requests().await.is_err());
}

fn import_summary() -> Value {
    json!({
        "id": 4, "user_id": 1, "username": "alice", "name": "Road trip", "state": "done",
        "message": null, "created_at": 1_790_000_000, "finished_at": 1_790_000_060,
        "tracks": 3, "asked": 1, "already": 1, "unmatched": 1, "waiting": 1,
    })
}

#[tokio::test]
async fn a_playlist_is_imported_listed_read_and_removed() {
    let server = MockServer::start().await;
    let mut queued = import_summary();
    queued["state"] = json!("queued");
    Mock::given(method("POST"))
        .and(path("/api/v1/requests/imports"))
        .and(body_json(
            json!({"name": "Road trip", "csv": "Track Name\nCreep\n"}),
        ))
        .respond_with(ResponseTemplate::new(202).set_body_json(queued))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/requests/imports"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([import_summary()])))
        .mount(&server)
        .await;
    let mut one = import_summary();
    one["items"] = json!([
        {"position": 1, "title": "Creep", "artist": "Radiohead", "album": null, "isrc": null,
         "state": "asked", "why": null, "request_id": 9},
        {"position": 2, "title": "Nowhere Song", "artist": null, "album": null, "isrc": null,
         "state": "unmatched", "why": "no recording fits", "request_id": null},
    ]);
    Mock::given(method("GET"))
        .and(path("/api/v1/requests/imports/4"))
        .respond_with(ResponseTemplate::new(200).set_body_json(one))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/requests/imports/4"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"taken_back": 1})))
        .expect(1)
        .mount(&server)
        .await;
    let client = signed_in(&server);
    let started = client
        .import_playlist("Road trip", "Track Name\nCreep\n")
        .await
        .unwrap();
    assert_eq!((started.id, started.state.as_str()), (4, "queued"));
    let list = client.playlist_imports().await.unwrap();
    assert_eq!((list[0].tracks, list[0].unmatched), (3, 1));
    let read = client.playlist_import(4).await.unwrap();
    assert_eq!(read.summary.name, "Road trip");
    assert_eq!(read.items[0].request_id, Some(9));
    assert_eq!(read.items[1].why.as_deref(), Some("no recording fits"));
    client.remove_playlist_import(4).await.unwrap();
}

#[tokio::test]
async fn a_playlist_imported_as_a_playlist_names_the_playlist_melete_made() {
    let server = MockServer::start().await;
    let mut queued = import_summary();
    queued["state"] = json!("queued");
    queued["in_library"] = json!(1);
    queued["playlist_id"] = json!("0B8A1E8B-B3DF-45A7-88E0-EA53F78393C3");
    Mock::given(method("POST"))
        .and(path("/api/v1/requests/imports"))
        .and(body_json(json!({
            "name": "Road trip", "csv": "Track Name\nCreep\n", "make_playlist": true,
        })))
        .respond_with(ResponseTemplate::new(202).set_body_json(queued))
        .expect(1)
        .mount(&server)
        .await;
    let client = signed_in(&server);
    let started = client
        .import_as_playlist("Road trip", "Track Name\nCreep\n")
        .await
        .unwrap();
    assert_eq!(started.in_library, 1);
    assert_eq!(
        started.playlist_id.as_deref(),
        Some("0B8A1E8B-B3DF-45A7-88E0-EA53F78393C3")
    );
}

/// A Melete before 0.4.0 refuses the field it doesn't know, and says nothing of a
/// playlist in a summary it does send.
#[tokio::test]
async fn an_older_server_refuses_make_playlist_and_its_summaries_still_read() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/requests/imports"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(json!({"error": "unknown field make_playlist"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/requests/imports"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([import_summary()])))
        .mount(&server)
        .await;
    let client = signed_in(&server);
    let refused = client
        .import_as_playlist("Road trip", "Track Name\nCreep\n")
        .await;
    assert!(
        matches!(refused, Err(Error::Http { status: 400, .. })),
        "{refused:?}"
    );
    let list = client.playlist_imports().await.unwrap();
    assert_eq!(
        (list[0].in_library, list[0].playlist_id.as_deref()),
        (0, None)
    );
}
