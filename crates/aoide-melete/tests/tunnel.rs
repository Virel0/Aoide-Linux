//! Melete's own tunnel and the client's route, against local mock servers.
//! Nothing here leaves 127.0.0.1.

use aoide_melete::{Credentials, Device, Error, MeleteClient, ServerAddress};
use serde_json::json;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const DEVICES: &str = "/api/v1/tunnel/devices";

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

#[tokio::test]
async fn the_list_says_whether_the_server_has_a_tunnel() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(DEVICES))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "enabled": true, "limit": 10,
            "devices": [{"id": 3, "name": "Desk", "parts": ["music", "requests"],
                         "address": "10.73.0.4", "created_at": 1_790_000_000}],
        })))
        .mount(&server)
        .await;
    let devices = signed_in(&server).tunnel_devices().await.unwrap();
    assert!(devices.enabled);
    assert_eq!(devices.devices[0].address.as_deref(), Some("10.73.0.4"));
}

#[tokio::test]
async fn joining_sends_only_the_public_key_and_the_music_parts() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(DEVICES))
        .and(body_json(json!({
            "name": "Desk", "parts": ["music", "requests"], "public_key": "cHVibGlj",
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": 4, "name": "Desk", "parts": ["music", "requests"], "address": "10.73.0.5",
            "created_at": 1_790_000_000, "server_public_key": "c2VydmVy",
            "server_address": "10.73.0.1", "server": "http://10.73.0.1",
            "endpoint": "melete.example:51820", "preshared_key": "c2hhcmVk",
            "wireguard_config": null,
        })))
        .expect(1)
        .mount(&server)
        .await;
    let joined = signed_in(&server)
        .join_tunnel("Desk", "cHVibGlj")
        .await
        .unwrap();
    assert_eq!(joined.server, "http://10.73.0.1");
    assert_eq!(joined.endpoint, "melete.example:51820");
    assert!(
        !format!("{joined:?}").contains("c2hhcmVk"),
        "the preshared key stays out of logs"
    );
}

#[tokio::test]
async fn a_server_without_a_tunnel_answers_409() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(DEVICES))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({
            "error": "Melete's own tunnel isn't set up on this server (MELETE_TUNNEL_PORT)",
        })))
        .mount(&server)
        .await;
    let error = signed_in(&server)
        .join_tunnel("Desk", "cHVibGlj")
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::Http { status: 409, .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn leaving_deletes_the_device() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(format!("{DEVICES}/4")))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    signed_in(&server).leave_tunnel(4).await.unwrap();
}

#[tokio::test]
async fn every_clone_follows_the_route_and_back() {
    let (home, tunnel) = (MockServer::start().await, MockServer::start().await);
    for server in [&home, &tunnel] {
        Mock::given(method("GET"))
            .and(path(DEVICES))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "enabled": true, "limit": 10, "devices": [],
            })))
            .expect(1)
            .mount(server)
            .await;
    }
    let client = signed_in(&home);
    let clone = client.clone();
    client
        .route()
        .set(Some(&ServerAddress::parse(&tunnel.uri()).unwrap()));
    clone.tunnel_devices().await.unwrap();
    client.route().set(None);
    clone.tunnel_devices().await.unwrap();
    assert_eq!(
        clone.address().as_str(),
        home.uri(),
        "the address shown stays the server's own"
    );
}
