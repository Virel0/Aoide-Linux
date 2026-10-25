//! HTTP behaviour against a local mock server. Nothing here leaves 127.0.0.1.

use std::time::Duration;

use aoide_jellyfin::{
    Credentials, Device, Error, ItemKind, ItemRefresh, JellyfinClient, ListQuery, MixSeed,
    PlaylistEdit, ServerAddress, Sort, SortOrder,
};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const USER: &str = "5ca1ab1e5ca1ab1e5ca1ab1e5ca1ab1e";
const TOKEN: &str = "fixture-token-not-a-secret";

fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn signed_out_header() -> String {
    format!(
        "MediaBrowser Client=\"Aoide\", Device=\"Test\", DeviceId=\"test-device\", Version=\"{}\"",
        version()
    )
}

fn signed_in_header() -> String {
    format!("{}, Token=\"{TOKEN}\"", signed_out_header())
}

fn client_at(base: &str) -> JellyfinClient {
    JellyfinClient::new(
        ServerAddress::parse(base).unwrap(),
        Device::new("Test", "test-device"),
    )
    .unwrap()
}

fn signed_in(server: &MockServer) -> JellyfinClient {
    client_at(&server.uri()).with_credentials(Credentials {
        user_id: USER.into(),
        access_token: TOKEN.into(),
        username: "alice".into(),
    })
}

fn fixture(name: &str) -> Value {
    let text = match name {
        "album_tracks" => include_str!("fixtures/album_tracks.json"),
        "user_views" => include_str!("fixtures/user_views.json"),
        "lyrics_enhanced" => include_str!("fixtures/lyrics_enhanced.json"),
        "system_info_public" => include_str!("fixtures/system_info_public.json"),
        "authenticate_by_name" => include_str!("fixtures/authenticate_by_name.json"),
        other => panic!("no fixture {other}"),
    };
    serde_json::from_str(text).unwrap()
}

fn item(id: &str, name: &str, kind: &str) -> Value {
    json!({ "Id": id, "Name": name, "Type": kind })
}

fn page(items: Vec<Value>) -> Value {
    json!({ "Items": items, "TotalRecordCount": items.len(), "StartIndex": 0 })
}

/// wiremock's own `header` matcher splits values at commas, and the MediaBrowser
/// header is full of them, so compare the raw value.
fn exact_header(name: &'static str, expected: String) -> impl Fn(&Request) -> bool {
    move |request: &Request| {
        request
            .headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == expected)
    }
}

fn query_value(request: &Request, name: &str) -> Option<String> {
    request
        .url
        .query_pairs()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
}

// MARK: - Server check and sign-in

#[tokio::test]
async fn a_jellyfin_server_answers_the_public_info_probe() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/System/Info/Public"))
        .and(exact_header("Authorization", signed_out_header()))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("system_info_public")))
        .expect(1)
        .mount(&server)
        .await;

    let info = client_at(&server.uri()).public_system_info().await.unwrap();
    assert_eq!(info.version, "10.11.2");
    assert_eq!(info.server_name.as_deref(), Some("living-room"));
}

#[tokio::test]
async fn something_that_isnt_jellyfin_is_named_as_such() {
    for response in [
        ResponseTemplate::new(200).set_body_string("<html><body>Router login</body></html>"),
        ResponseTemplate::new(200).set_body_json(json!({ "status": "ok" })),
        ResponseTemplate::new(404),
    ] {
        let server = MockServer::start().await;
        Mock::given(path("/System/Info/Public"))
            .respond_with(response)
            .mount(&server)
            .await;
        assert_eq!(
            client_at(&server.uri()).public_system_info().await,
            Err(Error::NotJellyfin("127.0.0.1".into()))
        );
    }
}

#[tokio::test]
async fn a_server_error_on_the_probe_is_not_mistaken_for_another_service() {
    let server = MockServer::start().await;
    Mock::given(path("/System/Info/Public"))
        .respond_with(ResponseTemplate::new(503).set_body_string("Server is starting"))
        .mount(&server)
        .await;
    assert!(matches!(
        client_at(&server.uri()).public_system_info().await,
        Err(Error::Http { status: 503, .. })
    ));
}

#[tokio::test]
async fn requests_join_beneath_a_reverse_proxy_subpath() {
    let server = MockServer::start().await;
    Mock::given(path("/jellyfin/System/Info/Public"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("system_info_public")))
        .expect(1)
        .mount(&server)
        .await;
    let client = client_at(&format!("{}/jellyfin/", server.uri()));
    assert!(client.public_system_info().await.is_ok());
}

#[tokio::test]
async fn signing_in_posts_a_pascal_case_body_and_returns_credentials() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/Users/AuthenticateByName"))
        .and(exact_header("Authorization", signed_out_header()))
        .and(exact_header("X-Emby-Authorization", signed_out_header()))
        .and(body_json(
            json!({ "Username": "alice", "Pw": "correct horse" }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("authenticate_by_name")))
        .expect(1)
        .mount(&server)
        .await;

    let credentials = client_at(&server.uri())
        .authenticate_by_name("alice", "correct horse")
        .await
        .unwrap();
    assert_eq!(credentials.user_id, USER);
    assert_eq!(credentials.access_token, TOKEN);
    assert_eq!(credentials.username, "alice");
}

#[tokio::test]
async fn a_wrong_password_is_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(path("/Users/AuthenticateByName"))
        .respond_with(ResponseTemplate::new(401).set_body_string("Error processing request."))
        .mount(&server)
        .await;
    assert_eq!(
        client_at(&server.uri())
            .authenticate_by_name("alice", "wrong")
            .await,
        Err(Error::Unauthorized)
    );
}

#[tokio::test]
async fn signed_in_requests_carry_the_token_in_both_headers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/Sessions/Logout"))
        .and(exact_header("Authorization", signed_in_header()))
        .and(exact_header("X-Emby-Authorization", signed_in_header()))
        .and(header("Content-Length", "0"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    signed_in(&server).sign_out().await.unwrap();
}

// MARK: - Error mapping over the wire

#[tokio::test]
async fn statuses_map_to_typed_errors() {
    let server = MockServer::start().await;
    Mock::given(path("/UserViews"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    Mock::given(path("/Items/gone/InstantMix"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(path("/Items"))
        .respond_with(ResponseTemplate::new(500).set_body_string(format!(
            "System.InvalidOperationException: nope\n{}",
            "at ".repeat(1_000)
        )))
        .mount(&server)
        .await;

    let client = signed_in(&server);
    assert_eq!(client.music_libraries().await, Err(Error::Unauthorized));
    assert_eq!(
        client.instant_mix("gone", 50).await,
        Err(Error::NotFound {
            path: "Items/gone/InstantMix".into()
        })
    );
    let Err(Error::Http {
        status,
        path,
        detail,
    }) = client.albums(&ListQuery::default()).await
    else {
        panic!("expected Error::Http");
    };
    assert_eq!((status, path.as_str()), (500, "Items"));
    let detail = detail.unwrap();
    assert!(detail.starts_with("System.InvalidOperationException: nope"));
    assert!(detail.len() <= 400);
}

#[tokio::test]
async fn a_body_that_isnt_the_expected_json_is_a_decoding_error() {
    let server = MockServer::start().await;
    Mock::given(path("/Items"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;
    assert!(matches!(
        signed_in(&server).songs(&ListQuery::default()).await,
        Err(Error::Decoding { path, .. }) if path == "Items"
    ));
}

#[tokio::test]
async fn a_refused_connection_is_a_transport_error() {
    // Bind and release a port so nothing is listening on it.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let result = client_at(&format!("http://127.0.0.1:{port}"))
        .public_system_info()
        .await;
    assert!(matches!(result, Err(Error::Transport(_))), "{result:?}");
}

#[tokio::test]
async fn library_calls_need_credentials_and_send_nothing_without_them() {
    let server = MockServer::start().await;
    let client = client_at(&server.uri());
    assert_eq!(client.music_libraries().await, Err(Error::NotSignedIn));
    assert_eq!(client.album_tracks("a").await, Err(Error::NotSignedIn));
    assert_eq!(client.items_by_ids(&["a"]).await, Err(Error::NotSignedIn));
    assert!(server.received_requests().await.unwrap().is_empty());
}

// MARK: - Library

#[tokio::test]
async fn music_libraries_are_the_music_views_only() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/UserViews"))
        .and(query_param("userId", USER))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("user_views")))
        .mount(&server)
        .await;
    let libraries = signed_in(&server).music_libraries().await.unwrap();
    assert_eq!(libraries.len(), 1);
    assert_eq!(libraries[0].name, "Music");
}

#[tokio::test]
async fn album_pages_carry_paging_sorting_and_scope() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("userId", USER))
        .and(query_param("includeItemTypes", "MusicAlbum"))
        .and(query_param("recursive", "true"))
        .and(query_param("parentId", "lib1"))
        .and(query_param("sortBy", "DateCreated"))
        .and(query_param("sortOrder", "Descending"))
        .and(query_param("startIndex", "200"))
        .and(query_param("limit", "100"))
        .and(query_param(
            "fields",
            "Genres,AlbumArtist,Artists,ParentId,ChildCount,PrimaryImageAspectRatio",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "Items": [item("al1", "Low Tide Radio", "MusicAlbum")],
            "TotalRecordCount": 412,
            "StartIndex": 200
        })))
        .expect(1)
        .mount(&server)
        .await;

    let query = ListQuery {
        parent_id: Some("lib1".into()),
        sort: Sort::DateAdded,
        order: SortOrder::Descending,
        start_index: 200,
        limit: Some(100),
        ..ListQuery::default()
    };
    let page = signed_in(&server).albums(&query).await.unwrap();
    assert_eq!(page.total_record_count, 412);
    assert_eq!(page.start_index, 200);
    assert_eq!(page.items[0].kind, ItemKind::MusicAlbum);
}

#[tokio::test]
async fn albums_can_be_filtered_by_genre() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(query_param("includeItemTypes", "MusicAlbum"))
        .and(query_param("genreIds", "g1,g2"))
        .and(query_param("parentId", "lib1"))
        .and(query_param("sortBy", "AlbumArtist,SortName"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(vec![item(
            "al1",
            "Blue",
            "MusicAlbum",
        )])))
        .expect(1)
        .mount(&server)
        .await;
    // Without genres the filter is not sent at all.
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(|request: &Request| query_value(request, "genreIds").is_none())
        .respond_with(ResponseTemplate::new(200).set_body_json(page(vec![])))
        .expect(1)
        .mount(&server)
        .await;

    let client = signed_in(&server);
    let query = ListQuery {
        parent_id: Some("lib1".into()),
        genre_ids: vec!["g1".into(), "g2".into()],
        sort: Sort::Artist,
        ..ListQuery::default()
    };
    let albums = client.albums(&query).await.unwrap();
    assert_eq!(albums.items[0].name, "Blue");
    assert!(
        client
            .albums(&ListQuery::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
}

#[tokio::test]
async fn each_list_goes_to_its_own_endpoint() {
    let server = MockServer::start().await;
    for (route, kind) in [
        ("/Artists", "MusicArtist"),
        ("/Artists/AlbumArtists", "MusicArtist"),
        ("/MusicGenres", "MusicGenre"),
    ] {
        Mock::given(method("GET"))
            .and(path(route))
            .and(query_param("sortBy", "SortName"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(page(vec![item(route, route, kind)])),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    for kinds in ["Audio", "Playlist"] {
        Mock::given(method("GET"))
            .and(path("/Items"))
            .and(query_param("includeItemTypes", kinds))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(page(vec![item(kinds, kinds, kinds)])),
            )
            .expect(1)
            .mount(&server)
            .await;
    }

    let client = signed_in(&server);
    let q = ListQuery::default();
    assert_eq!(client.artists(&q).await.unwrap().items[0].id, "/Artists");
    assert_eq!(
        client.album_artists(&q).await.unwrap().items[0].id,
        "/Artists/AlbumArtists"
    );
    assert_eq!(client.genres(&q).await.unwrap().items[0].id, "/MusicGenres");
    assert_eq!(client.songs(&q).await.unwrap().items[0].id, "Audio");
    assert_eq!(client.playlists(&q).await.unwrap().items[0].id, "Playlist");
}

#[tokio::test]
async fn album_tracks_are_asked_for_in_disc_and_track_order() {
    let server = MockServer::start().await;
    Mock::given(path("/Items"))
        .and(query_param("parentId", "b0a1c2d3e4f5061728394a5b6c7d8e9f"))
        .and(query_param("includeItemTypes", "Audio"))
        .and(query_param(
            "sortBy",
            "ParentIndexNumber,IndexNumber,SortName",
        ))
        .and(query_param("sortOrder", "Ascending"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("album_tracks")))
        .expect(1)
        .mount(&server)
        .await;
    let tracks = signed_in(&server)
        .album_tracks("b0a1c2d3e4f5061728394a5b6c7d8e9f")
        .await
        .unwrap();
    let order: Vec<_> = tracks
        .iter()
        .map(|t| (t.parent_index_number, t.index_number))
        .collect();
    assert_eq!(
        order,
        [(Some(1), Some(1)), (Some(1), Some(2)), (Some(2), Some(1))]
    );
}

#[tokio::test]
async fn an_artists_albums_are_newest_first() {
    let server = MockServer::start().await;
    Mock::given(path("/Items"))
        .and(query_param("albumArtistIds", "artist1"))
        .and(query_param("includeItemTypes", "MusicAlbum"))
        .and(query_param(
            "sortBy",
            "PremiereDate,ProductionYear,SortName",
        ))
        .and(query_param("sortOrder", "Descending"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(vec![item(
            "al1",
            "A",
            "MusicAlbum",
        )])))
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        signed_in(&server)
            .artist_albums("artist1")
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn items_by_ids_chunk_and_come_back_in_the_order_asked() {
    let server = MockServer::start().await;
    // Answers in reverse order and leaves out "id-042", as a server does for an
    // item deleted since the playlist was saved.
    Mock::given(method("GET"))
        .and(path("/Items"))
        .respond_with(|request: &Request| {
            let ids = query_value(request, "ids").unwrap_or_default();
            let items: Vec<Value> = ids
                .split(',')
                .rev()
                .filter(|id| *id != "id-042")
                .map(|id| item(id, id, "Audio"))
                .collect();
            ResponseTemplate::new(200).set_body_json(page(items))
        })
        .expect(2)
        .mount(&server)
        .await;

    let mut ids: Vec<String> = (0..150).map(|n| format!("id-{n:03}")).collect();
    ids.push("id-007".into()); // asked for twice, as a playlist can hold a song twice
    let found = signed_in(&server).items_by_ids(&ids).await.unwrap();

    let expected: Vec<&str> = ids
        .iter()
        .map(String::as_str)
        .filter(|id| *id != "id-042")
        .collect();
    assert_eq!(
        found.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
        expected
    );

    let requests = server.received_requests().await.unwrap();
    let sizes: Vec<usize> = requests
        .iter()
        .map(|r| query_value(r, "ids").unwrap().split(',').count())
        .collect();
    assert_eq!(sizes, [100, 51]);
    for request in &requests {
        assert_eq!(query_value(request, "userId").as_deref(), Some(USER));
    }
}

#[tokio::test]
async fn no_ids_means_no_request() {
    let server = MockServer::start().await;
    let none: [&str; 0] = [];
    assert_eq!(signed_in(&server).items_by_ids(&none).await, Ok(vec![]));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn search_asks_for_each_kind_and_keeps_them_apart() {
    let server = MockServer::start().await;
    for (kinds, id) in [("Audio", "song1"), ("MusicAlbum", "album1")] {
        Mock::given(path("/Items"))
            .and(query_param("includeItemTypes", kinds))
            .and(query_param("searchTerm", "tide"))
            .and(query_param("parentId", "lib1"))
            .and(query_param("limit", "20"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(vec![item(id, id, kinds)])))
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(path("/Artists/AlbumArtists"))
        .and(query_param("searchTerm", "tide"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(vec![item(
            "artist1",
            "A",
            "MusicArtist",
        )])))
        .expect(1)
        .mount(&server)
        .await;

    let results = signed_in(&server)
        .search("  tide ", Some("lib1"), 20)
        .await
        .unwrap();
    assert_eq!(results.songs[0].id, "song1");
    assert_eq!(results.albums[0].id, "album1");
    assert_eq!(results.artists[0].id, "artist1");
}

#[tokio::test]
async fn a_blank_search_sends_nothing() {
    let server = MockServer::start().await;
    let results = signed_in(&server).search("   ", None, 20).await.unwrap();
    assert!(results.songs.is_empty() && results.albums.is_empty() && results.artists.is_empty());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn instant_mix_and_playlist_items() {
    let server = MockServer::start().await;
    Mock::given(path("/Items/seed1/InstantMix"))
        .and(query_param("userId", USER))
        .and(query_param("limit", "50"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(page(vec![item("m1", "M", "Audio")])),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/Playlists/pl1/Items"))
        .and(query_param("userId", USER))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(vec![
            item("p2", "Second", "Audio"),
            item("p1", "First", "Audio"),
        ])))
        .expect(1)
        .mount(&server)
        .await;
    let client = signed_in(&server);
    assert_eq!(client.instant_mix("seed1", 50).await.unwrap()[0].id, "m1");
    let entries = client.playlist_items("pl1").await.unwrap();
    assert_eq!(
        entries.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
        ["p2", "p1"]
    );
}

#[tokio::test]
async fn favourites_are_set_with_post_and_cleared_with_delete() {
    let server = MockServer::start().await;
    let route = format!("/Users/{USER}/FavoriteItems/song1");
    Mock::given(method("POST"))
        .and(path(route.as_str()))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "IsFavorite": true, "PlayCount": 2 })),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(route.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "IsFavorite": false })))
        .expect(1)
        .mount(&server)
        .await;
    let client = signed_in(&server);
    let set = client.set_favorite("song1", true).await.unwrap();
    assert!(set.is_favorite);
    assert_eq!(set.play_count, 2);
    assert!(
        !client
            .set_favorite("song1", false)
            .await
            .unwrap()
            .is_favorite
    );
}

#[tokio::test]
async fn recently_added_drops_tracks_with_no_album() {
    let server = MockServer::start().await;
    Mock::given(path("/Items/Latest"))
        .and(query_param("includeItemTypes", "Audio"))
        .and(query_param("groupItems", "true"))
        .and(query_param("parentId", "lib1"))
        .and(query_param("limit", "20"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            item("al1", "An Album", "MusicAlbum"),
            item("loose1", "A loose track", "Audio"),
            item("al2", "Another", "MusicAlbum"),
        ])))
        .expect(1)
        .mount(&server)
        .await;
    let albums = signed_in(&server)
        .recently_added(Some("lib1"), 20)
        .await
        .unwrap();
    assert_eq!(
        albums.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(),
        ["al1", "al2"]
    );
}

#[tokio::test]
async fn recently_and_most_played_are_played_tracks_newest_or_most_first() {
    let server = MockServer::start().await;
    for (sort, id) in [("DatePlayed", "recent"), ("PlayCount", "most")] {
        Mock::given(path("/Items"))
            .and(query_param("includeItemTypes", "Audio"))
            .and(query_param("filters", "IsPlayed"))
            .and(query_param("sortBy", sort))
            .and(query_param("sortOrder", "Descending"))
            .and(query_param("limit", "25"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(page(vec![item(id, id, "Audio")])),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    let client = signed_in(&server);
    assert_eq!(
        client.recently_played(None, 25).await.unwrap()[0].id,
        "recent"
    );
    assert_eq!(client.most_played(None, 25).await.unwrap()[0].id, "most");
}

// MARK: - Lyrics

#[tokio::test]
async fn lyrics_decode_and_a_404_means_none() {
    let server = MockServer::start().await;
    Mock::given(path("/Audio/song1/Lyrics"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("lyrics_enhanced")))
        .mount(&server)
        .await;
    Mock::given(path("/Audio/song2/Lyrics"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(path("/Audio/song3/Lyrics"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "Lyrics": [] })))
        .mount(&server)
        .await;
    Mock::given(path("/Audio/song4/Lyrics"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let client = signed_in(&server);
    let lyrics = client.lyrics("song1").await.unwrap().unwrap();
    assert!(lyrics.is_synced());
    assert!(lyrics.has_word_timing());
    assert_eq!(lyrics.lines[1].start, Some(Duration::from_millis(4_200)));
    assert_eq!(client.lyrics("song2").await, Ok(None));
    assert_eq!(client.lyrics("song3").await, Ok(None));
    assert!(matches!(
        client.lyrics("song4").await,
        Err(Error::Http { status: 500, .. })
    ));
}

// MARK: - Playback reporting

#[tokio::test]
async fn playback_reports_carry_the_position_in_ticks() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/Sessions/Playing"))
        .and(body_json(json!({
            "ItemId": "song1", "PositionTicks": 0, "IsPaused": false,
            "CanSeek": true, "PlayMethod": "Transcode"
        })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/Sessions/Playing/Progress"))
        .and(body_json(json!({
            "ItemId": "song1", "PositionTicks": 125_000_000, "IsPaused": true,
            "PlayMethod": "Transcode", "EventName": "Pause"
        })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/Sessions/Playing/Stopped"))
        .and(exact_header("Authorization", signed_in_header()))
        .and(body_json(
            json!({ "ItemId": "song1", "PositionTicks": 2_435_000_000_i64 }),
        ))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let client = signed_in(&server);
    client
        .report_playback_started("song1", Duration::ZERO)
        .await
        .unwrap();
    client
        .report_playback_progress("song1", Duration::from_millis(12_500), true)
        .await
        .unwrap();
    client
        .report_playback_stopped("song1", Duration::from_millis(243_500))
        .await
        .unwrap();
}

// MARK: - Media URLs against the server

#[tokio::test]
async fn the_stream_url_authenticates_with_apikey_against_a_server_that_refuses_api_key() {
    let server = MockServer::start().await;
    // A 10.12-style server: legacy `api_key` is off, so only `ApiKey` is accepted.
    Mock::given(method("GET"))
        .and(path("/Audio/song1/universal"))
        .respond_with(|request: &Request| {
            let legacy = query_value(request, "api_key").is_some();
            match query_value(request, "ApiKey") {
                Some(key) if key == TOKEN && !legacy => {
                    ResponseTemplate::new(200).set_body_bytes(b"fLaC".to_vec())
                }
                _ => ResponseTemplate::new(401),
            }
        })
        .expect(1)
        .mount(&server)
        .await;

    let url = signed_in(&server)
        .stream_url("song1", aoide_jellyfin::StreamQuality::Lossless)
        .unwrap();
    // A plain GET with no headers, the way a media player fetches it.
    let response = reqwest::get(url).await.unwrap();
    assert_eq!(response.status().as_u16(), 200);
}

// MARK: - Jellyfin playlists

#[tokio::test]
async fn creating_a_playlist_sends_name_ids_and_owner_in_the_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/Playlists"))
        .and(exact_header("Authorization", signed_in_header()))
        .and(body_json(json!({
            "Name": "Road trip",
            "Ids": ["s1", "s2"],
            "UserId": USER,
            "MediaType": "Audio",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "Id": "pl9" })))
        .expect(1)
        .mount(&server)
        .await;
    let id = signed_in(&server)
        .create_playlist("Road trip", &["s1", "s2"])
        .await
        .unwrap();
    assert_eq!(id, "pl9");
}

#[tokio::test]
async fn editing_a_playlist_sends_only_what_changed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/Playlists/pl1"))
        .and(body_json(json!({ "Name": "Renamed" })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/Playlists/pl1"))
        .and(body_json(json!({ "Name": "Both", "IsPublic": true })))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let client = signed_in(&server);
    let rename = PlaylistEdit {
        name: Some("Renamed".into()),
        ..PlaylistEdit::default()
    };
    client.edit_playlist("pl1", &rename).await.unwrap();
    let both = PlaylistEdit {
        name: Some("Both".into()),
        is_public: Some(true),
    };
    client.edit_playlist("pl1", &both).await.unwrap();
    // An empty edit is not a request.
    client
        .edit_playlist("pl1", &PlaylistEdit::default())
        .await
        .unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn deleting_a_playlist_checks_it_is_one_first() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Playlists/pl1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "OpenAccess": false, "Shares": [], "ItemIds": [] })),
        )
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/Items/pl1"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    // A song's id: not a playlist, so the server's playlist lookup is a 404.
    Mock::given(method("GET"))
        .and(path("/Playlists/song1"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/Items/song1"))
        .respond_with(ResponseTemplate::new(204))
        .expect(0)
        .mount(&server)
        .await;

    let client = signed_in(&server);
    client.delete_playlist("pl1").await.unwrap();
    assert_eq!(
        client.delete_playlist("song1").await,
        Err(Error::NotFound {
            path: "Playlists/song1".into()
        })
    );
}

#[tokio::test]
async fn playlist_entries_carry_their_entry_ids() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/Playlists/pl1/Items"))
        .and(query_param("userId", USER))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(vec![
            json!({ "Id": "s1", "Name": "One", "Type": "Audio", "PlaylistItemId": "e1" }),
            json!({ "Id": "s1", "Name": "One", "Type": "Audio", "PlaylistItemId": "e2" }),
            // A server that leaves the entry id out: the item id stands in.
            json!({ "Id": "s3", "Name": "Three", "Type": "Audio" }),
        ])))
        .expect(1)
        .mount(&server)
        .await;
    let entries = signed_in(&server).playlist_entries("pl1").await.unwrap();
    let pairs: Vec<_> = entries
        .iter()
        .map(|e| {
            (
                e.entry_id.as_str(),
                e.item.id.as_str(),
                e.item.name.as_str(),
            )
        })
        .collect();
    assert_eq!(
        pairs,
        [
            ("e1", "s1", "One"),
            ("e2", "s1", "One"),
            ("s3", "s3", "Three")
        ]
    );
}

#[tokio::test]
async fn adding_to_a_playlist_chunks_and_keeps_the_order() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/Playlists/pl1/Items"))
        .and(query_param("userId", USER))
        .respond_with(ResponseTemplate::new(204))
        .expect(3)
        .mount(&server)
        .await;
    let ids: Vec<String> = (0..150).map(|n| format!("s{n}")).collect();
    let client = signed_in(&server);
    client.add_to_playlist("pl1", &ids, Some(4)).await.unwrap();
    client
        .add_to_playlist("pl1", &["last"], None)
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    let sent: Vec<(usize, Option<String>)> = requests
        .iter()
        .map(|r| {
            (
                query_value(r, "ids").unwrap().split(',').count(),
                query_value(r, "position"),
            )
        })
        .collect();
    assert_eq!(
        sent,
        [(100, Some("4".into())), (50, Some("104".into())), (1, None)]
    );
    assert_eq!(
        query_value(&requests[1], "ids").unwrap().split(',').next(),
        Some("s100")
    );
}

#[tokio::test]
async fn removing_goes_by_entry_id_and_moving_by_index() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/Playlists/pl1/Items"))
        .and(query_param("entryIds", "e1,e2"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/Playlists/pl1/Items/e3/Move/0"))
        .and(header("Content-Length", "0"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let client = signed_in(&server);
    client
        .remove_from_playlist("pl1", &["e1", "e2"])
        .await
        .unwrap();
    let none: [&str; 0] = [];
    client.remove_from_playlist("pl1", &none).await.unwrap();
    client.move_playlist_entry("pl1", "e3", 0).await.unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn a_cover_uploads_as_base64_under_its_image_type() {
    let server = MockServer::start().await;
    let image = [0xFF_u8, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
    Mock::given(method("POST"))
        .and(path("/Items/pl1/Images/Primary"))
        .and(header("Content-Type", "image/jpeg"))
        .and(exact_header("Authorization", signed_in_header()))
        .and(|request: &Request| request.body == b"/9j/4AAQ")
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/Items/pl1/Images/Primary"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/Items/pl2/Images/Primary"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;

    let client = signed_in(&server);
    client
        .set_playlist_cover("pl1", &image, "image/jpeg")
        .await
        .unwrap();
    client.delete_playlist_cover("pl1").await.unwrap();
    // Not an administrator.
    assert!(matches!(
        client.set_playlist_cover("pl2", &image, "image/jpeg").await,
        Err(Error::Http { status: 403, .. })
    ));
}

#[tokio::test]
async fn playlist_edits_need_credentials_and_send_nothing_without_them() {
    let server = MockServer::start().await;
    let client = client_at(&server.uri());
    assert_eq!(
        client.create_playlist("x", &["a"]).await,
        Err(Error::NotSignedIn)
    );
    assert_eq!(client.delete_playlist("pl1").await, Err(Error::NotSignedIn));
    assert_eq!(
        client.move_playlist_entry("pl1", "e1", 2).await,
        Err(Error::NotSignedIn)
    );
    assert_eq!(
        client.remove_from_playlist("pl1", &["e1"]).await,
        Err(Error::NotSignedIn)
    );
    assert_eq!(
        client.set_playlist_cover("pl1", b"x", "image/png").await,
        Err(Error::NotSignedIn)
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

// MARK: - Radio

#[tokio::test]
async fn each_mix_seed_goes_to_its_own_endpoint() {
    let server = MockServer::start().await;
    for route in [
        "/Songs/t1/InstantMix",
        "/Albums/al1/InstantMix",
        "/Artists/ar1/InstantMix",
        "/Playlists/pl1/InstantMix",
    ] {
        Mock::given(method("GET"))
            .and(path(route))
            .and(query_param("userId", USER))
            .and(query_param("limit", "40"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(page(vec![item(route, "M", "Audio")])),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/MusicGenres/InstantMix"))
        .and(query_param("id", "g1"))
        .and(query_param("userId", USER))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(vec![item(
            "genre-mix",
            "M",
            "Audio",
        )])))
        .expect(1)
        .mount(&server)
        .await;

    let client = signed_in(&server);
    let first = |seed: MixSeed| {
        let client = client.clone();
        async move {
            client.instant_mix_from(&seed, 40).await.unwrap()[0]
                .id
                .clone()
        }
    };
    assert_eq!(
        first(MixSeed::Track("t1".into())).await,
        "/Songs/t1/InstantMix"
    );
    assert_eq!(
        first(MixSeed::Album("al1".into())).await,
        "/Albums/al1/InstantMix"
    );
    assert_eq!(
        first(MixSeed::Artist("ar1".into())).await,
        "/Artists/ar1/InstantMix"
    );
    assert_eq!(
        first(MixSeed::Playlist("pl1".into())).await,
        "/Playlists/pl1/InstantMix"
    );
    assert_eq!(first(MixSeed::Genre("g1".into())).await, "genre-mix");
}

#[tokio::test]
async fn similar_items_albums_and_artists() {
    let server = MockServer::start().await;
    for (route, kind) in [
        ("/Items/x1/Similar", "Audio"),
        ("/Albums/al1/Similar", "MusicAlbum"),
        ("/Artists/ar1/Similar", "MusicArtist"),
    ] {
        Mock::given(method("GET"))
            .and(path(route))
            .and(query_param("userId", USER))
            .and(query_param("limit", "12"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(page(vec![item(route, "S", kind)])),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    let client = signed_in(&server);
    assert_eq!(
        client.similar_items("x1", 12).await.unwrap()[0].id,
        "/Items/x1/Similar"
    );
    let albums = client.similar_albums("al1", 12).await.unwrap();
    assert_eq!(albums[0].kind, ItemKind::MusicAlbum);
    let artists = client.similar_artists("ar1", 12).await.unwrap();
    assert_eq!(artists[0].kind, ItemKind::MusicArtist);
}

// MARK: - Favourites

#[tokio::test]
async fn favourite_songs_albums_and_artists_are_lists_with_is_favorite() {
    let server = MockServer::start().await;
    for kinds in ["Audio", "MusicAlbum"] {
        Mock::given(method("GET"))
            .and(path("/Items"))
            .and(query_param("includeItemTypes", kinds))
            .and(query_param("isFavorite", "true"))
            .and(query_param("userId", USER))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(page(vec![item(kinds, "F", kinds)])),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/Artists"))
        .and(query_param("isFavorite", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(vec![item(
            "ar1",
            "F",
            "MusicArtist",
        )])))
        .expect(1)
        .mount(&server)
        .await;
    // Without the flag nothing about favourites is sent.
    Mock::given(method("GET"))
        .and(path("/Items"))
        .and(|request: &Request| query_value(request, "isFavorite").is_none())
        .respond_with(ResponseTemplate::new(200).set_body_json(page(vec![])))
        .expect(1)
        .mount(&server)
        .await;

    let client = signed_in(&server);
    let favourites = ListQuery {
        favorites_only: true,
        ..ListQuery::default()
    };
    assert_eq!(
        client.songs(&favourites).await.unwrap().items[0].id,
        "Audio"
    );
    assert_eq!(
        client.albums(&favourites).await.unwrap().items[0].id,
        "MusicAlbum"
    );
    assert_eq!(
        client.artists(&favourites).await.unwrap().items[0].id,
        "ar1"
    );
    assert!(
        client
            .songs(&ListQuery::default())
            .await
            .unwrap()
            .items
            .is_empty()
    );
}

// MARK: - Library maintenance

#[tokio::test]
async fn a_library_rescan_starts_the_scan_task_by_its_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ScheduledTasks"))
        .and(exact_header("Authorization", signed_in_header()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "Name": "Clean Cache Directory", "Key": "DeleteCacheFiles", "Id": "t-cache", "State": "Idle" },
            // The name is translated on a French server; the key is not.
            { "Name": "Actualiser la médiathèque", "Key": "RefreshLibrary", "Id": "t-scan", "State": "Idle" },
        ])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/ScheduledTasks/Running/t-scan"))
        .and(header("Content-Length", "0"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    signed_in(&server).refresh_library().await.unwrap();
}

#[tokio::test]
async fn a_server_without_the_scan_task_says_so_and_starts_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ScheduledTasks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "Name": "Other", "Key": "Other", "Id": "t1" }
        ])))
        .mount(&server)
        .await;
    assert_eq!(
        signed_in(&server).refresh_library().await,
        Err(Error::NotFound {
            path: "ScheduledTasks/RefreshLibrary".into()
        })
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn an_item_refresh_always_names_its_modes() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/Items/al1/Refresh"))
        .respond_with(ResponseTemplate::new(204))
        .expect(3)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/Items/al2/Refresh"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;

    let client = signed_in(&server);
    for refresh in [
        ItemRefresh::ScanFiles,
        ItemRefresh::FillMissing,
        ItemRefresh::ReplaceAll { images: true },
    ] {
        client.refresh_item("al1", refresh).await.unwrap();
    }
    let sent: Vec<Vec<Option<String>>> = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| {
            [
                "metadataRefreshMode",
                "imageRefreshMode",
                "replaceAllMetadata",
                "replaceAllImages",
            ]
            .iter()
            .map(|name| query_value(r, name))
            .collect()
        })
        .collect();
    let some = |values: [&str; 4]| values.map(|v| Some(v.to_owned())).to_vec();
    assert_eq!(
        sent,
        [
            some(["Default", "Default", "false", "false"]),
            some(["FullRefresh", "FullRefresh", "false", "false"]),
            some(["FullRefresh", "FullRefresh", "true", "true"]),
        ]
    );
    // Not an administrator.
    assert!(matches!(
        client.refresh_item("al2", ItemRefresh::ScanFiles).await,
        Err(Error::Http { status: 403, .. })
    ));
}
