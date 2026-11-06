//! Choosing one's own profile picture against a local mock server: from a
//! file, from the server's gallery, or none. Nothing here leaves 127.0.0.1.

use aoide_melete::{Credentials, Device, Error, MeleteClient, PictureType, ServerAddress};
use serde_json::json;
use wiremock::matchers::{body_bytes, body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "mlt_fixture-token-not-a-secret";
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfixture";

fn signed_in(server: &MockServer) -> MeleteClient {
    MeleteClient::new(
        ServerAddress::parse(&server.uri()).unwrap(),
        Device::new("Test", "test-device"),
    )
    .unwrap()
    .with_credentials(Credentials {
        user_id: "1".into(),
        access_token: TOKEN.into(),
        username: "alice".into(),
    })
}

fn bearer() -> wiremock::matchers::HeaderExactMatcher {
    header("authorization", format!("Bearer {TOKEN}").as_str())
}

fn picture_answer(url: Option<&str>) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "avatar_url": url }))
}

#[tokio::test]
async fn me_says_who_and_whether_an_administrator() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": 7, "username": "alice", "is_admin": true, "sees_all_music": true,
            "avatar_url": null
        })))
        .mount(&server)
        .await;
    let me = signed_in(&server).me().await.unwrap();
    assert_eq!(
        (me.id, me.username.as_str(), me.is_admin),
        (7, "alice", true)
    );
}

#[tokio::test]
async fn a_file_is_sent_as_it_is_with_its_type() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/me/avatar"))
        .and(bearer())
        .and(header("content-type", "image/png"))
        .and(body_bytes(PNG))
        .respond_with(picture_answer(Some(
            "/api/v1/users/1/avatar?v=0123456789ab",
        )))
        .expect(1)
        .mount(&server)
        .await;
    let url = signed_in(&server)
        .set_avatar(PNG, PictureType::Png)
        .await
        .unwrap();
    assert_eq!(
        url.as_deref(),
        Some("/api/v1/users/1/avatar?v=0123456789ab")
    );
}

#[tokio::test]
async fn a_refused_file_says_why_by_its_status() {
    for status in [400, 413] {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/me/avatar"))
            .respond_with(ResponseTemplate::new(status).set_body_json(json!({"error": "not that"})))
            .mount(&server)
            .await;
        let refused = signed_in(&server).set_avatar(PNG, PictureType::Png).await;
        assert!(
            matches!(refused, Err(Error::Http { status: s, .. }) if s == status),
            "{refused:?}"
        );
    }
}

#[tokio::test]
async fn the_gallery_lists_its_categories_and_pages_of_pictures() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/avatars"))
        .and(bearer())
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"categories": [
                {"id": "official", "name": "Melete", "count": 12},
                {"id": "gallery/Birds", "name": "Birds", "count": 3},
            ]})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/avatars/pictures"))
        .and(query_param("category", "gallery/Birds"))
        .and(query_param("offset", "2"))
        .and(query_param("limit", "200"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "total": 3,
            "pictures": [{"id": "gallery/Birds/wren.png",
                          "url": "/api/v1/avatars/picture?id=gallery%2FBirds%2Fwren.png"}]
        })))
        .mount(&server)
        .await;
    let client = signed_in(&server);
    let categories = client.avatar_categories().await.unwrap();
    assert_eq!(categories[1].id, "gallery/Birds");
    assert_eq!(
        (categories[0].name.as_str(), categories[0].count),
        ("Melete", 12)
    );
    let page = client
        .gallery_pictures("gallery/Birds", 2, 500)
        .await
        .unwrap();
    assert_eq!(page.total, 3);
    assert_eq!(page.pictures[0].id, "gallery/Birds/wren.png");
}

#[tokio::test]
async fn a_gallery_picture_is_read_with_the_token_by_its_id() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/avatars/picture"))
        .and(query_param("id", "gallery/Birds/wren.png"))
        .and(bearer())
        .respond_with(ResponseTemplate::new(200).set_body_bytes(PNG))
        .mount(&server)
        .await;
    let client = signed_in(&server);
    assert_eq!(
        client
            .gallery_picture("gallery/Birds/wren.png")
            .await
            .unwrap(),
        PNG
    );
    assert!(matches!(
        client.gallery_picture("gallery/Birds/none.png").await,
        Err(Error::NotFound { .. })
    ));
}

#[tokio::test]
async fn a_gallery_picture_is_chosen_by_its_id() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/me/avatar/gallery"))
        .and(body_json(json!({"id": "official/lyre.png"})))
        .respond_with(picture_answer(Some(
            "/api/v1/users/1/avatar?v=aaaaaaaaaaaa",
        )))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/me/avatar/gallery"))
        .and(body_json(json!({"id": "official/broken.png"})))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({"error": "not a picture"})))
        .mount(&server)
        .await;
    let client = signed_in(&server);
    assert_eq!(
        client
            .set_gallery_avatar("official/lyre.png")
            .await
            .unwrap()
            .as_deref(),
        Some("/api/v1/users/1/avatar?v=aaaaaaaaaaaa")
    );
    assert!(matches!(
        client.set_gallery_avatar("official/broken.png").await,
        Err(Error::Http { status: 422, .. })
    ));
    assert!(matches!(
        client.set_gallery_avatar("official/gone.png").await,
        Err(Error::NotFound { .. })
    ));
}

#[tokio::test]
async fn removing_the_picture_leaves_none() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/me/avatar"))
        .and(bearer())
        .respond_with(picture_answer(None))
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(signed_in(&server).remove_avatar().await.unwrap(), None);
}
