//! Collections (Melete 0.8.0) against a local mock server. Nothing here leaves
//! 127.0.0.1.

use aoide_melete::{Credentials, Device, Error, MeleteClient, ServerAddress};
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
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

fn listed() -> serde_json::Value {
    json!({ "items": [
        { "id": 2, "name": "Late jazz", "genres": ["Jazz", "Soul"], "year_from": 1955,
          "year_to": 1969, "liked": false, "created_at": 1_793_000_000 },
        { "id": 5, "name": "Liked nineties", "genres": [], "year_from": 1990,
          "year_to": 1999, "liked": true, "created_at": 1_793_000_100 },
    ]})
}

#[tokio::test]
async fn the_list_is_read_with_the_token_and_an_older_server_has_none() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/music/collections"))
        .and(header(
            "authorization",
            "Bearer mlt_fixture-token-not-a-secret",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(listed()))
        .expect(1)
        .mount(&server)
        .await;
    let found = signed_in(&server).collections().await.unwrap();
    let names: Vec<&str> = found.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Late jazz", "Liked nineties"]);
    assert!(found[1].liked);

    let older = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/music/collections"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"error": "not found"})))
        .mount(&older)
        .await;
    assert!(signed_in(&older).collections().await.unwrap().is_empty());

    let failing = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/music/collections"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&failing)
        .await;
    assert!(signed_in(&failing).collections().await.is_err());
}

#[tokio::test]
async fn a_collections_songs_are_asked_for_by_its_filter() {
    let server = MockServer::start().await;
    let tracks: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/tracks.json")).unwrap();
    Mock::given(method("GET"))
        .and(path("/api/v1/music/tracks"))
        .and(query_param("genres", "Jazz;Soul"))
        .and(query_param("year_from", "1955"))
        .and(query_param("year_to", "1969"))
        .and(query_param("offset", "0"))
        .and(query_param("limit", "500"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tracks))
        .expect(1)
        .mount(&server)
        .await;
    let client = signed_in(&server);
    let jazz: aoide_melete::Collection =
        serde_json::from_value(listed()["items"][0].clone()).unwrap();
    let page = client.collection_songs(&jazz, 0, 10_000).await.unwrap();
    assert_eq!(page.total, 4);
    assert_eq!(page.items[0].name, "Café Song");
    let asked = &server.received_requests().await.unwrap();
    let query = asked
        .last()
        .unwrap()
        .url
        .query()
        .unwrap_or_default()
        .to_owned();
    assert!(!query.contains("liked"), "{query}");
}

#[tokio::test]
async fn shuffle_asks_for_one_page_in_a_random_order_over_the_whole_collection() {
    let server = MockServer::start().await;
    let mut tracks: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/tracks.json")).unwrap();
    tracks["seed"] = json!(12345);
    Mock::given(method("GET"))
        .and(path("/api/v1/music/tracks"))
        .and(query_param("sort", "random"))
        .and(query_param("year_from", "1990"))
        .and(query_param("year_to", "1999"))
        .and(query_param("liked", "true"))
        .and(query_param("offset", "0"))
        .and(query_param("limit", "500"))
        .respond_with(ResponseTemplate::new(200).set_body_json(tracks))
        .expect(1)
        .mount(&server)
        .await;
    let nineties: aoide_melete::Collection =
        serde_json::from_value(listed()["items"][1].clone()).unwrap();
    let songs = signed_in(&server)
        .collection_shuffled(&nineties, 500)
        .await
        .unwrap();
    assert_eq!(songs.len(), 4);
    let asked = &server.received_requests().await.unwrap();
    let query = asked
        .last()
        .unwrap()
        .url
        .query()
        .unwrap_or_default()
        .to_owned();
    assert!(!query.contains("seed"), "one page needs no seed: {query}");
    assert!(!query.contains("genres"), "{query}");
}

#[tokio::test]
async fn deleting_one_sends_its_id_and_a_gone_one_is_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/music/collections/2"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/music/collections/9"))
        .respond_with(
            ResponseTemplate::new(404).set_body_json(json!({"error": "no such collection"})),
        )
        .mount(&server)
        .await;
    let client = signed_in(&server);
    client.delete_collection(2).await.unwrap();
    assert!(matches!(
        client.delete_collection(9).await,
        Err(Error::NotFound { .. })
    ));
}
