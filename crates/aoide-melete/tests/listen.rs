//! Listening together against a local mock server. Nothing here leaves 127.0.0.1.

use aoide_melete::{
    Credentials, Device, Error, ListenState, MeleteClient, ServerAddress, is_stale,
};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, method, path};
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

/// The server's answer for a session at `version`.
fn answer(version: i64) -> Value {
    json!({
        "session": {
            "id": 7, "code": "ABC234", "host": "alice", "queue": [4, 5], "index": 1,
            "position_ms": 2500, "position_at": 1_000_000, "playing": true,
            "version": version,
            "members": [
                {"username": "alice", "device": "Desk", "you": true},
                {"username": "bob", "device": "Phone", "you": false},
            ],
        },
        "server_time": 1_000_400,
    })
}

fn state() -> ListenState {
    ListenState {
        queue: vec![4, 5],
        index: 1,
        position_ms: 2500,
        playing: true,
    }
}

#[tokio::test]
async fn starting_sends_the_queue_and_reads_the_session() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/music/sessions"))
        .and(body_json(
            json!({"queue": [4, 5], "index": 1, "position_ms": 2500, "playing": true}),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(answer(1)))
        .mount(&server)
        .await;
    let listening = signed_in(&server).start_listening(&state()).await.unwrap();
    assert_eq!(listening.session.code, "ABC234");
    assert_eq!(listening.server_time, 1_000_400);
    let devices: Vec<(&str, bool)> = listening
        .session
        .members
        .iter()
        .map(|m| (m.device.as_str(), m.you))
        .collect();
    assert_eq!(devices, [("Desk", true), ("Phone", false)]);
}

#[tokio::test]
async fn joining_sends_the_code_and_a_wrong_one_is_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/music/sessions/join"))
        .and(body_json(json!({"code": "abc-234"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(answer(2)))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/music/sessions/join"))
        .respond_with(ResponseTemplate::new(404).set_body_json(
            json!({"error": "no session has that code; check it with whoever's listening"}),
        ))
        .mount(&server)
        .await;
    let client = signed_in(&server);
    assert_eq!(
        client.join_listening("abc-234").await.unwrap().session.id,
        7
    );
    assert!(matches!(
        client.join_listening("ZZZ999").await,
        Err(Error::NotFound { .. })
    ));
}

#[tokio::test]
async fn a_change_names_its_version_and_a_stale_one_is_told_apart() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/music/sessions/7"))
        .and(body_json(json!({"version": 2, "queue": [4, 5], "index": 1,
                              "position_ms": 2500, "playing": true})))
        .respond_with(ResponseTemplate::new(200).set_body_json(answer(3)))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/music/sessions/7"))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({
            "error": "it changed since; get it again and make the change against its new version"
        })))
        .mount(&server)
        .await;
    let client = signed_in(&server);
    let changed = client.change_listening(7, 2, &state()).await.unwrap();
    assert_eq!(changed.session.version, 3);
    let stale = client.change_listening(7, 1, &state()).await.unwrap_err();
    assert!(is_stale(&stale), "{stale:?}");
    assert!(!is_stale(&Error::NotFound {
        path: String::new()
    }));
}

#[tokio::test]
async fn leaving_posts_and_ending_deletes() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/music/sessions/7/leave"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/api/v1/music/sessions/7"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/music/sessions/7"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"error": "no such session"})))
        .mount(&server)
        .await;
    let client = signed_in(&server);
    client.leave_listening(7).await.unwrap();
    client.end_listening(7).await.unwrap();
    assert!(matches!(
        client.listening(7).await,
        Err(Error::NotFound { .. })
    ));
}
