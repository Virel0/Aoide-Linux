//! LAN discovery against a local UDP responder. Nothing here leaves 127.0.0.1.

use std::net::SocketAddr;
use std::time::Duration;

use aoide_jellyfin::{DISCOVERY_PROBE, discover_servers_at};
use serde_json::json;
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

/// A stand-in server: answers the first datagram with `replies`, then records
/// everything else that arrives until `listen` is up.
async fn responder(
    replies: Vec<Vec<u8>>,
    listen: Duration,
) -> (SocketAddr, JoinHandle<Vec<Vec<u8>>>) {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut received = Vec::new();
        let mut buffer = [0u8; 2048];
        let (length, from) = socket.recv_from(&mut buffer).await.unwrap();
        received.push(buffer[..length].to_vec());
        for reply in replies {
            socket.send_to(&reply, from).await.unwrap();
        }
        while let Ok(Ok((length, _))) =
            tokio::time::timeout(listen, socket.recv_from(&mut buffer)).await
        {
            received.push(buffer[..length].to_vec());
        }
        received
    });
    (address, task)
}

fn reply(id: &str, name: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "Address": "http://192.0.2.10:8096",
        "Id": id,
        "Name": name,
        "EndpointAddress": null
    }))
    .unwrap()
}

#[tokio::test]
async fn the_probe_is_sent_alone_and_each_server_is_listed_once() {
    let (address, task) = responder(
        vec![
            b"not json at all".to_vec(),
            reply("0f5d3a7c9b1e4d2f8a6c5b4e3d2c1b0a", "living-room"),
            // The same server again, as one with two interfaces answers.
            reply("0f5d3a7c9b1e4d2f8a6c5b4e3d2c1b0a", "living-room"),
            // JSON, but not a server's answer.
            br#"{"Name":"no id"}"#.to_vec(),
            reply("1a2b3c4d5e6f708192a3b4c5d6e7f809", "attic"),
        ],
        Duration::from_millis(400),
    )
    .await;

    let servers = discover_servers_at(address, Duration::from_millis(300))
        .await
        .unwrap();
    let found: Vec<_> = servers.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(found, ["living-room", "attic"]);
    assert_eq!(servers[0].address, "http://192.0.2.10:8096");
    assert_eq!(servers[0].endpoint_address, None);
    assert_eq!(
        servers[0].server_address().unwrap().as_str(),
        "http://192.0.2.10:8096"
    );

    // Exactly one datagram reached the responder, and it was the probe.
    let received = task.await.unwrap();
    assert_eq!(received, [DISCOVERY_PROBE.to_vec()]);
    assert_eq!(DISCOVERY_PROBE, b"Who is JellyfinServer?");
}

#[tokio::test]
async fn silence_is_an_empty_list_after_the_wait() {
    let (address, task) = responder(Vec::new(), Duration::from_millis(50)).await;
    let started = std::time::Instant::now();
    let servers = discover_servers_at(address, Duration::from_millis(200))
        .await
        .unwrap();
    assert!(servers.is_empty());
    assert!(started.elapsed() >= Duration::from_millis(200));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(task.await.unwrap(), [DISCOVERY_PROBE.to_vec()]);
}
