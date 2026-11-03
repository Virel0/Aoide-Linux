//! Profile pictures against a local mock server. Nothing here leaves 127.0.0.1.

use aoide_melete::{Credentials, Device, MeleteClient, ServerAddress};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "mlt_fixture-token-not-a-secret";

fn signed_in(base: &str) -> MeleteClient {
    MeleteClient::new(
        ServerAddress::parse(base).unwrap(),
        Device::new("Test", "test-device"),
    )
    .unwrap()
    .with_credentials(Credentials {
        user_id: "1".into(),
        access_token: TOKEN.into(),
        username: "alice".into(),
    })
}

async fn answering(route: &str, body: serde_json::Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(route))
        .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn me_gives_the_picture_and_none_from_an_older_server() {
    let me = json!({"id": 1, "username": "alice", "is_admin": false, "sees_all_music": true,
        "avatar_url": "/api/v1/users/1/avatar?v=0123456789ab"});
    let server = answering("/api/v1/me", me).await;
    let found = signed_in(&server.uri()).me().await.unwrap();
    assert_eq!(
        found.avatar_url.as_deref(),
        Some("/api/v1/users/1/avatar?v=0123456789ab")
    );

    let older = json!({"id": 1, "username": "alice", "is_admin": false, "sees_all_music": true});
    let server = answering("/api/v1/me", older).await;
    assert_eq!(
        signed_in(&server.uri()).me().await.unwrap().avatar_url,
        None
    );
}

#[tokio::test]
async fn the_directory_gives_each_picture_where_there_is_one() {
    let users = json!([
        {"id": 2, "username": "robin", "avatar_url": "/api/v1/users/2/avatar?v=aaaaaaaaaaaa"},
        {"id": 3, "username": "sam", "avatar_url": null},
        {"id": 4, "username": "kit"},
    ]);
    let server = answering("/api/v1/users/directory", users).await;
    let found = signed_in(&server.uri()).user_directory().await.unwrap();
    let pictures: Vec<(&str, Option<&str>)> = found
        .iter()
        .map(|u| (u.name.as_str(), u.avatar_url.as_deref()))
        .collect();
    assert_eq!(
        pictures,
        [
            ("robin", Some("/api/v1/users/2/avatar?v=aaaaaaaaaaaa")),
            ("sam", None),
            ("kit", None),
        ]
    );
}

#[tokio::test]
async fn listen_together_members_carry_a_picture_when_the_server_sends_one() {
    let session = json!([{
        "id": 7, "code": "ABC234", "host": "alice", "queue": [4], "index": 0,
        "position_ms": 0, "position_at": 1, "playing": false, "version": 1,
        "members": [
            {"username": "alice", "device": "Desk", "you": true,
             "avatar_url": "/api/v1/users/1/avatar?v=bbbbbbbbbbbb"},
            {"username": "bob", "device": "Phone", "you": false},
        ],
    }]);
    let server = answering("/api/v1/music/sessions", session).await;
    let sessions = signed_in(&server.uri()).listening_sessions().await.unwrap();
    let pictures: Vec<Option<&str>> = sessions[0]
        .members
        .iter()
        .map(|m| m.avatar_url.as_deref())
        .collect();
    assert_eq!(
        pictures,
        [Some("/api/v1/users/1/avatar?v=bbbbbbbbbbbb"), None]
    );
}

#[test]
fn a_picture_is_fetched_beneath_a_proxy_prefix_with_its_hash() {
    let client = signed_in("https://example.com/melete");
    assert_eq!(
        client
            .avatar_url("/api/v1/users/2/avatar?v=0123456789ab")
            .map(|u| u.to_string())
            .as_deref(),
        Some("https://example.com/melete/api/v1/users/2/avatar?v=0123456789ab")
    );
    assert_eq!(
        client
            .avatar_url("/api/v1/users/2/avatar")
            .map(|u| u.to_string())
            .as_deref(),
        Some("https://example.com/melete/api/v1/users/2/avatar")
    );
}

#[test]
fn a_picture_address_cant_leave_the_server() {
    let client = signed_in("https://example.com");
    for path in [
        "https://elsewhere.example/a.png",
        "//elsewhere.example/a.png",
        "/api/../admin?v=1",
        "/api/v1/users/2/avatar?",
        "/api/v1/users/2/avatar?v=1#x",
        "/api/v1/users/2/avatar?v=a b",
        "/api/v1/users/2/avatar?v=1?w=2",
    ] {
        assert_eq!(client.avatar_url(path), None, "{path}");
    }
}
