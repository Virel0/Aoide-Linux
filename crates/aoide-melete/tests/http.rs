//! HTTP behaviour against a local mock server. Nothing here leaves 127.0.0.1.

use aoide_melete::{
    Credentials, Device, Error, ListQuery, MeleteClient, ServerAddress, Sort, StreamFormat,
};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const TOKEN: &str = "mlt_fixture-token-not-a-secret";

fn client_at(base: &str) -> MeleteClient {
    MeleteClient::new(
        ServerAddress::parse(base).unwrap(),
        Device::new("Test", "test-device"),
    )
    .unwrap()
}

fn signed_in(server: &MockServer) -> MeleteClient {
    client_at(&server.uri()).with_credentials(Credentials {
        user_id: "1".into(),
        access_token: TOKEN.into(),
        username: "alice".into(),
    })
}

fn fixture(name: &str) -> Value {
    let text = match name {
        "tracks" => include_str!("fixtures/tracks.json"),
        "album" => include_str!("fixtures/album.json"),
        "albums" => include_str!("fixtures/albums.json"),
        "artist" => include_str!("fixtures/artist.json"),
        "genres" => include_str!("fixtures/genres.json"),
        "home" => include_str!("fixtures/home.json"),
        "lyrics" => include_str!("fixtures/lyrics.json"),
        "play" => include_str!("fixtures/play.json"),
        other => panic!("no fixture {other}"),
    };
    serde_json::from_str(text).unwrap()
}

/// A page of `tracks.json`'s first track with each of `ids`.
fn tracks_with_ids(ids: &[i64]) -> Value {
    let template = fixture("tracks")["items"][0].clone();
    let items: Vec<Value> = ids
        .iter()
        .map(|id| {
            let mut track = template.clone();
            track["id"] = json!(id);
            track["title"] = json!(format!("Track {id}"));
            track
        })
        .collect();
    json!({ "items": items, "total": ids.len() })
}

fn query_value(request: &Request, name: &str) -> Option<String> {
    request
        .url
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

#[tokio::test]
async fn every_signed_in_request_carries_the_token_as_bearer() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/music/genres"))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("genres")))
        .expect(1)
        .mount(&server)
        .await;
    let genres = signed_in(&server).genres().await.unwrap();
    assert_eq!(genres[0].name, "Rock");
}

#[tokio::test]
async fn a_signed_out_request_sends_no_authorization() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/health"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"status":"ok","version":"0.1.0"})),
        )
        .mount(&server)
        .await;
    let info = client_at(&server.uri()).server_info().await.unwrap();
    assert_eq!(info.version, "0.1.0");
    let requests = server.received_requests().await.unwrap();
    assert!(requests[0].headers.get("authorization").is_none());
}

#[tokio::test]
async fn something_that_isnt_melete_is_named_as_such() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/health"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>router</html>"))
        .mount(&server)
        .await;
    let error = client_at(&server.uri()).server_info().await.unwrap_err();
    assert!(matches!(error, Error::NotMelete(_)), "{error:?}");

    let empty = MockServer::start().await;
    let error = client_at(&empty.uri()).server_info().await.unwrap_err();
    assert!(matches!(error, Error::NotMelete(_)), "a 404 too: {error:?}");
}

#[tokio::test]
async fn sign_in_sends_the_device_name_and_reads_the_session() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .and(body_json(json!({
            "username": "alice",
            "password": "pw",
            "device_name": "Test",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token": TOKEN,
            "user": {"id": 3, "username": "alice", "is_admin": false},
        })))
        .mount(&server)
        .await;
    let credentials = client_at(&server.uri())
        .sign_in("alice", "pw")
        .await
        .unwrap();
    assert_eq!(credentials.user_id, "3");
    assert_eq!(credentials.access_token, TOKEN);
}

#[tokio::test]
async fn two_factor_sign_in_finishes_with_the_code() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": "a code from your authenticator app is needed",
            "second_factor": "totp", "challenge": "m2f_abc", "expires_in": 300,
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login/second-factor"))
        .and(body_json(json!({"challenge": "m2f_abc", "code": "123456"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token": TOKEN,
            "user": {"id": 3, "username": "alice", "is_admin": false},
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login/second-factor"))
        .and(body_json(json!({"challenge": "m2f_abc", "code": "000000"})))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(json!({"error": "that code isn't right", "reason": "wrong_code"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login/second-factor"))
        .respond_with(ResponseTemplate::new(410).set_body_json(
            json!({"error": "the sign-in ran out; sign in with your password again"}),
        ))
        .mount(&server)
        .await;
    let client = client_at(&server.uri());
    let Err(Error::SecondFactorNeeded { challenge }) = client.sign_in("alice", "pw").await else {
        panic!("expected a challenge");
    };
    assert_eq!(challenge, "m2f_abc");
    assert_eq!(
        client.finish_sign_in(&challenge, "000000").await,
        Err(Error::Unauthorized)
    );
    // Spaces around the code are the authenticator's, not the code's.
    let credentials = client.finish_sign_in(&challenge, " 123456 ").await.unwrap();
    assert_eq!(credentials.access_token, TOKEN);
    assert!(matches!(
        client.finish_sign_in("m2f_old", "123456").await,
        Err(Error::Http { status: 410, .. })
    ));
}

#[tokio::test]
async fn a_wrong_password_is_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"error": "wrong"})))
        .mount(&server)
        .await;
    let error = client_at(&server.uri())
        .sign_in("a", "b")
        .await
        .unwrap_err();
    assert_eq!(error, Error::Unauthorized);
}

#[tokio::test]
async fn quick_connect_waits_then_signs_in_once_approved() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/quick-connect"))
        .and(body_json(json!({ "device_name": "Test" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "code": "482913", "secret": "s3cret", "expires_in": 300,
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/quick-connect/poll"))
        .and(body_json(json!({ "secret": "s3cret" })))
        .respond_with(ResponseTemplate::new(202))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/quick-connect/poll"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token": TOKEN,
            "user": {"id": 9, "username": "bea", "is_admin": false},
        })))
        .mount(&server)
        .await;

    let client = client_at(&server.uri());
    let code = client.start_quick_connect().await.unwrap();
    assert_eq!(code.code, "482913");
    assert_eq!(client.poll_quick_connect(&code.secret).await.unwrap(), None);
    let credentials = client
        .poll_quick_connect(&code.secret)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(credentials.username, "bea");
}

#[tokio::test]
async fn a_quick_connect_code_that_ran_out_is_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/quick-connect/poll"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"error": "no such code"})))
        .mount(&server)
        .await;
    let error = client_at(&server.uri())
        .poll_quick_connect("old")
        .await
        .unwrap_err();
    assert!(matches!(error, Error::NotFound { .. }), "{error:?}");
}

#[tokio::test]
async fn a_list_query_sends_its_filters_and_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/music/albums"))
        .and(query_param("q", "first"))
        .and(query_param("genre", "Rock"))
        .and(query_param("artist_id", "2,5"))
        .and(query_param("sort", "year"))
        .and(query_param("offset", "40"))
        .and(query_param("limit", "20"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("albums")))
        .expect(1)
        .mount(&server)
        .await;
    let page = signed_in(&server)
        .albums(&ListQuery {
            search_term: Some(" first ".into()),
            genre: Some("Rock".into()),
            artist_ids: vec!["2".into(), "5".into()],
            sort: Sort::Year,
            start_index: 40,
            limit: Some(20),
        })
        .await
        .unwrap();
    assert_eq!(page.start_index, 40);
    assert_eq!(page.total, 2);
}

#[tokio::test]
async fn a_song_search_matches_songs_alone_and_an_album_search_doesnt_say_so() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/music/tracks"))
        .and(query_param("q", "first"))
        .and(query_param("q_in", "track"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("tracks")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/music/albums"))
        .and(query_param("q", "first"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("albums")))
        .expect(1)
        .mount(&server)
        .await;
    let client = signed_in(&server);
    let query = ListQuery {
        search_term: Some("first".into()),
        limit: Some(20),
        ..ListQuery::default()
    };
    client.songs(&query).await.unwrap();
    client.albums(&query).await.unwrap();
    let albums_asked = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == "/api/v1/music/albums")
        .all(|r| !r.url.query().unwrap_or_default().contains("q_in"));
    assert!(albums_asked);
}

#[tokio::test]
async fn an_unlimited_list_is_read_page_by_page() {
    let server = MockServer::start().await;
    let first: Vec<i64> = (1..=500).collect();
    let second: Vec<i64> = (501..=520).collect();
    let mut body = tracks_with_ids(&first);
    body["total"] = json!(520);
    Mock::given(method("GET"))
        .and(path("/api/v1/music/tracks"))
        .and(query_param("offset", "0"))
        .and(query_param("limit", "500"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .expect(1)
        .mount(&server)
        .await;
    let mut body = tracks_with_ids(&second);
    body["total"] = json!(520);
    Mock::given(method("GET"))
        .and(path("/api/v1/music/tracks"))
        .and(query_param("offset", "500"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .expect(1)
        .mount(&server)
        .await;
    let page = signed_in(&server)
        .songs(&ListQuery::default())
        .await
        .unwrap();
    assert_eq!(page.items.len(), 520);
    assert_eq!(page.items[519].id, "520");
}

#[tokio::test]
async fn tracks_by_id_come_back_in_the_order_asked_100_at_a_time() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/music/tracks"))
        .respond_with(|request: &Request| {
            let ids: Vec<i64> = query_value(request, "id")
                .unwrap()
                .split(',')
                .map(|id| id.parse().unwrap())
                .filter(|id| *id != 7) // gone from the library
                .rev() // the server's own order
                .collect();
            ResponseTemplate::new(200).set_body_json(tracks_with_ids(&ids))
        })
        .expect(2)
        .mount(&server)
        .await;
    let ids: Vec<String> = (1..=150).map(|n| n.to_string()).collect();
    let mut asked = ids.clone();
    asked.push("3".into());
    let items = signed_in(&server).items_by_ids(&asked).await.unwrap();
    let got: Vec<&str> = items.iter().map(|item| item.id.as_str()).collect();
    let mut expected: Vec<&str> = ids
        .iter()
        .map(String::as_str)
        .filter(|id| *id != "7")
        .collect();
    expected.push("3");
    assert_eq!(got, expected);
}

#[tokio::test]
async fn an_album_gives_its_tracks_in_order() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/music/albums/2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("album")))
        .mount(&server)
        .await;
    let (album, tracks) = signed_in(&server).album("2").await.unwrap();
    assert_eq!(album.name, "First Album");
    assert!(!album.liked, "a server that doesn't say");
    assert_eq!(album.kind, aoide_melete::ItemKind::Album);
    let titles: Vec<&str> = tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(titles, ["Opening", "Middle", "Closing"]);
}

#[tokio::test]
async fn an_artists_albums_are_their_own_then_the_ones_they_appear_on() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/music/artists/2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("artist")))
        .mount(&server)
        .await;
    let albums = signed_in(&server).artist_albums("2").await.unwrap();
    assert_eq!(albums[0].name, "First Album");
}

#[tokio::test]
async fn home_has_its_three_shelves() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/music/home"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("home")))
        .mount(&server)
        .await;
    let home = signed_in(&server).home().await.unwrap();
    assert!(!home.recently_added.is_empty());
}

#[tokio::test]
async fn a_stream_link_is_joined_beneath_a_proxy_prefix() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/melete/api/v1/music/tracks/2/play"))
        .and(body_json(json!({ "format": "aac", "bitrate_kbps": 256 })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "stream_url": "/api/v1/music/stream/abc123",
            "expires_at": 1, "format": "aac", "content_type": "audio/mp4",
            "duration_seconds": 20.0, "gapless": null, "spatial": null,
        })))
        .mount(&server)
        .await;
    let client = client_at(&format!("{}/melete", server.uri())).with_credentials(Credentials {
        user_id: "1".into(),
        access_token: TOKEN.into(),
        username: "alice".into(),
    });
    let stream = client.play("2", StreamFormat::Aac(256)).await.unwrap();
    assert_eq!(
        stream.url.as_str(),
        format!("{}/melete/api/v1/music/stream/abc123", server.uri())
    );
    assert_eq!(stream.content_type, "audio/mp4");
}

#[tokio::test]
async fn a_stream_link_to_elsewhere_is_refused() {
    let server = MockServer::start().await;
    let mut answer = fixture("play");
    answer["stream_url"] = json!("https://elsewhere.example/steal");
    Mock::given(method("POST"))
        .and(path("/api/v1/music/tracks/2/play"))
        .respond_with(ResponseTemplate::new(200).set_body_json(answer))
        .mount(&server)
        .await;
    let error = signed_in(&server)
        .play("2", StreamFormat::Original)
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Decoding { .. }), "{error:?}");
}

#[tokio::test]
async fn lyrics_or_none() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/music/tracks/2/lyrics"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("lyrics")))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/music/tracks/3/lyrics"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_json(json!({"error": "no lyrics are known for this track"})),
        )
        .mount(&server)
        .await;
    let client = signed_in(&server);
    assert_eq!(client.lyrics("2").await.unwrap().unwrap().lines.len(), 2);
    assert_eq!(client.lyrics("3").await.unwrap(), None);
}

#[tokio::test]
async fn liking_puts_and_unliking_deletes() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/music/tracks/4/like"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/music/tracks/4/like"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let client = signed_in(&server);
    client.set_liked("4", true).await.unwrap();
    client.set_liked("4", false).await.unwrap();
}

#[tokio::test]
async fn albums_are_liked_listed_and_marked_on_their_page() {
    let server = MockServer::start().await;
    for verb in ["PUT", "DELETE"] {
        Mock::given(method(verb))
            .and(path("/api/v1/music/albums/2/like"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
    }
    let mut album = fixture("album");
    album["liked"] = json!(true);
    Mock::given(method("GET"))
        .and(path("/api/v1/music/albums/2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(album.clone()))
        .mount(&server)
        .await;
    album.as_object_mut().unwrap().remove("track_list");
    Mock::given(method("GET"))
        .and(path("/api/v1/music/likes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "tracks": [],
            "albums": [
                {"liked_at": 2, "content_key": "test artist|first album", "album": album},
                {"liked_at": 1, "content_key": "gone|album", "album": null},
            ],
        })))
        .mount(&server)
        .await;
    let client = signed_in(&server);
    client.set_album_liked("2", true).await.unwrap();
    client.set_album_liked("2", false).await.unwrap();
    assert!(client.album("2").await.unwrap().0.liked);
    let likes = client.likes().await.unwrap();
    let albums: Vec<(&str, bool)> = likes
        .albums
        .iter()
        .map(|a| (a.name.as_str(), a.liked))
        .collect();
    assert_eq!(albums, [("First Album", true)]);
    assert!(likes.songs.is_empty());
}

#[tokio::test]
async fn a_rescan_already_running_says_why() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/admin/system/tasks/library/run"))
        .respond_with(
            ResponseTemplate::new(409).set_body_json(json!({"error": "it is running now"})),
        )
        .mount(&server)
        .await;
    let error = signed_in(&server).refresh_library().await.unwrap_err();
    assert_eq!(
        error,
        Error::Http {
            status: 409,
            path: "api/v1/admin/system/tasks/library/run".into(),
            detail: Some("it is running now".into()),
        }
    );
}

#[tokio::test]
async fn the_user_directory_names_everyone() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/directory"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!([{"id": 1, "username": "alice"}, {"id": 2, "username": "robin"}]),
            ),
        )
        .mount(&server)
        .await;
    let users = signed_in(&server).user_directory().await.unwrap();
    assert_eq!(users[1].id, "2");
    assert_eq!(users[1].name, "robin");
}
