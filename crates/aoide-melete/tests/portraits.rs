//! Artists' portraits (Melete 0.9.0) against a local mock server. Nothing
//! here leaves 127.0.0.1.

use aoide_melete::{
    CoverSize, Credentials, Device, ListQuery, MeleteClient, PosterCredit, ServerAddress,
};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
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

/// A `MusicArtist` as Melete 0.9.0 sends it.
fn artist(id: i64, name: &str, portrait: Value, credit: Value) -> Value {
    json!({
        "id": id, "name": name, "mb_artist_id": null, "albums": 2, "appears_on": 0,
        "portrait": portrait, "portrait_credit": credit,
    })
}

fn commons_credit() -> Value {
    json!({
        "author": "A. Photographer", "licence": "CC BY-SA 4.0",
        "licence_url": "https://creativecommons.org/licenses/by-sa/4.0/",
        "source_url": "https://commons.wikimedia.org/wiki/File:Engines.jpg",
    })
}

#[tokio::test]
async fn an_artist_in_a_list_carries_their_portrait_and_its_credit() {
    let server = MockServer::start().await;
    let commons = artist(
        7,
        "The Quiet Engines",
        json!("/api/v1/music/artists/7/portrait"),
        commons_credit(),
    );
    let own = artist(
        8,
        "Mara Vey",
        json!("/api/v1/music/artists/8/portrait"),
        Value::Null,
    );
    let none = artist(9, "Low Tide", Value::Null, Value::Null);
    // A server before 0.9.0 sends neither field.
    let mut older = artist(10, "Paper Boats", Value::Null, Value::Null);
    older.as_object_mut().unwrap().remove("portrait");
    older.as_object_mut().unwrap().remove("portrait_credit");
    Mock::given(method("GET"))
        .and(path("/api/v1/music/artists"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "total": 4, "items": [commons, own, none, older],
        })))
        .mount(&server)
        .await;
    let page = signed_in(&server)
        .artists(&ListQuery::default())
        .await
        .unwrap();
    let found = &page.items;
    assert_eq!(
        found[0].portrait.as_deref(),
        Some("/api/v1/music/artists/7/portrait")
    );
    assert_eq!(
        found[0].portrait_credit,
        Some(PosterCredit {
            author: Some("A. Photographer".into()),
            licence: Some("CC BY-SA 4.0".into()),
            licence_url: Some("https://creativecommons.org/licenses/by-sa/4.0/".into()),
            source_url: Some("https://commons.wikimedia.org/wiki/File:Engines.jpg".into()),
        })
    );
    assert_eq!(
        (found[1].portrait.as_deref(), &found[1].portrait_credit),
        (Some("/api/v1/music/artists/8/portrait"), &None),
        "a picture of their own has no credit"
    );
    for artist in &found[2..] {
        assert_eq!((&artist.portrait, &artist.portrait_credit), (&None, &None));
    }
    assert_eq!(found[3].child_count, Some(2), "{:?}", found[3]);
}

#[tokio::test]
async fn an_artists_page_gives_them_with_their_portrait_and_albums() {
    let server = MockServer::start().await;
    let mut detail = artist(
        7,
        "The Quiet Engines",
        json!("/api/v1/music/artists/7/portrait"),
        json!({ "author": null, "licence": "CC0" }),
    );
    detail["album_list"] = json!([{
        "id": 2, "title": "First Album", "artist_id": 7, "artist": "The Quiet Engines",
        "year": 2020, "tracks": 3, "duration_seconds": 63.0, "cover": null,
        "compilation": false,
    }]);
    detail["appears_on_list"] = json!([]);
    Mock::given(method("GET"))
        .and(path("/api/v1/music/artists/7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(detail))
        .mount(&server)
        .await;
    let (found, albums) = signed_in(&server).artist("7").await.unwrap();
    assert_eq!(found.name, "The Quiet Engines");
    assert_eq!(found.kind, aoide_melete::ItemKind::Artist);
    assert_eq!(
        found.portrait.as_deref(),
        Some("/api/v1/music/artists/7/portrait")
    );
    assert_eq!(
        found.portrait_credit,
        Some(PosterCredit {
            licence: Some("CC0".into()),
            ..PosterCredit::default()
        })
    );
    let titles: Vec<&str> = albums.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(titles, ["First Album"]);
}

#[test]
fn a_portrait_is_asked_for_at_its_size_beneath_the_servers_prefix() {
    let client = MeleteClient::new(
        ServerAddress::parse("https://music.example/melete").unwrap(),
        Device::new("Test", "test-device"),
    )
    .unwrap();
    let url = client
        .poster_url("/api/v1/music/artists/7/portrait", CoverSize::W600)
        .unwrap();
    assert_eq!(
        url.as_str(),
        "https://music.example/melete/api/v1/music/artists/7/portrait?size=600"
    );
    for path in [
        "https://elsewhere.example/a.jpg",
        "//elsewhere.example/a.jpg",
        "api/v1/music/artists/7/portrait",
        "/api/v1/music/artists/7/../../../admin",
        "/api/v1/music/artists/7/portrait?size=800#x",
    ] {
        assert_eq!(client.poster_url(path, CoverSize::W600), None, "{path}");
    }
}
