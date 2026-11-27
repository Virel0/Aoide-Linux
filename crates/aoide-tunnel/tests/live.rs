//! Against a real Melete with its tunnel on: joins as a new device, reaches the
//! signed-in user's own account through the tunnel alone, and leaves. Ignored
//! unless asked for; it needs
//!
//! - `MELETE_LIVE_SERVER`: Melete's HTTP `host:port`, to join and leave by;
//! - `MELETE_LIVE_ENDPOINT`: its tunnel's `host:port` (UDP);
//! - `MELETE_LIVE_TOKEN_FILE`: a file holding a signed-in token.
//!
//! `cargo test -p aoide-tunnel --test live -- --ignored`

use std::net::SocketAddrV4;
use std::time::Duration;

use aoide_tunnel::{PrivateKey, Settings, Tunnel, decode};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} isn't set"))
}

/// One HTTP/1.1 request on a fresh connection: the status and the body.
async fn http(
    to: &str,
    method: &str,
    path: &str,
    token: &str,
    body: Option<&Value>,
) -> (u16, String) {
    let mut stream = TcpStream::connect(to).await.unwrap();
    let body = body.map(Value::to_string).unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {to}\r\nAuthorization: Bearer {token}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut answer = Vec::new();
    tokio::time::timeout(Duration::from_secs(30), stream.read_to_end(&mut answer))
        .await
        .expect("an answer within 30 seconds")
        .unwrap();
    let answer = String::from_utf8_lossy(&answer).into_owned();
    let status = answer[9..12].parse().unwrap();
    let body = answer
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_owned())
        .unwrap_or_default();
    (status, body)
}

/// A chunked body's JSON: Melete answers without a length.
fn json_of(body: &str) -> Value {
    let start = body.find('{').unwrap();
    let end = body.rfind('}').unwrap();
    serde_json::from_str(&body[start..=end]).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a real Melete with its tunnel on"]
async fn a_device_joins_and_reaches_its_account_through_the_tunnel() {
    let server = env("MELETE_LIVE_SERVER");
    let token = std::fs::read_to_string(env("MELETE_LIVE_TOKEN_FILE")).unwrap();
    let token = token.trim();
    let key = PrivateKey::generate().unwrap();

    let ask = json!({"name": "aoide-tunnel live test", "parts": ["music", "requests"],
                     "public_key": key.public_base64()});
    let (status, body) = http(&server, "POST", "/api/v1/tunnel/devices", token, Some(&ask)).await;
    assert_eq!(status, 201, "{body}");
    let joined = json_of(&body);
    let text = |field: &str| joined[field].as_str().unwrap().to_owned();
    let settings = Settings {
        private_key: key,
        address: text("address").parse().unwrap(),
        server_public_key: decode(&text("server_public_key")).unwrap(),
        preshared_key: decode(&text("preshared_key")).unwrap(),
        server: SocketAddrV4::new(text("server_address").parse().unwrap(), 80),
        endpoint: env("MELETE_LIVE_ENDPOINT"),
    };

    let tunnel = Tunnel::start(&settings).await.unwrap();
    let local = tunnel.local_address().to_string();
    let (status, body) = http(&local, "GET", "/api/v1/me", token, None).await;
    let (tracks, _) = http(&local, "GET", "/api/v1/music/tracks?limit=1", token, None).await;
    drop(tunnel);

    let id = joined["id"].as_i64().unwrap();
    let (left, _) = http(
        &server,
        "DELETE",
        &format!("/api/v1/tunnel/devices/{id}"),
        token,
        None,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("\"tester\""), "{body}");
    assert_eq!(tracks, 200);
    assert_eq!(left, 204);
}
