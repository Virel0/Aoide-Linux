//! "Find Servers on This Network": aoide-jellyfin's LAN discovery for the
//! sign-in screen. Nothing is sent until the button is pressed; then one
//! UDP probe goes to the local network's broadcast address, and the servers
//! that answer within a second are listed.

use std::net::SocketAddr;
use std::time::Duration;

use aoide_jellyfin::DiscoveredServer;

/// How long answers are gathered for. One second is plenty on a home
/// network.
pub const WAIT: Duration = Duration::from_secs(1);

/// A development build can probe one address instead of broadcasting
/// (`AOIDE_DEV_DISCOVERY_TARGET=127.0.0.1:17359`), so a test never reaches
/// a real server on the network.
const DEV_TARGET: &str = "AOIDE_DEV_DISCOVERY_TARGET";

fn dev_target() -> Option<SocketAddr> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var(DEV_TARGET).ok()?.parse().ok()
}

/// The servers that answered, in the order they did.
pub async fn find() -> Result<Vec<DiscoveredServer>, String> {
    let found = match dev_target() {
        Some(target) => aoide_jellyfin::discover_servers_at(target, WAIT).await,
        None => aoide_jellyfin::discover_servers(WAIT).await,
    };
    found.map_err(|e| crate::session::describe(&e))
}

/// A server as the list shows it: its name, or its address when it gave
/// none.
pub fn label(server: &DiscoveredServer) -> &str {
    match server.name.trim() {
        "" => server.address.as_str(),
        name => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(name: &str) -> DiscoveredServer {
        DiscoveredServer {
            id: "s1".into(),
            name: name.into(),
            address: "http://192.0.2.10:8096".into(),
            endpoint_address: None,
        }
    }

    #[test]
    fn a_server_is_listed_by_name_or_address() {
        assert_eq!(label(&server("Living Room")), "Living Room");
        assert_eq!(label(&server("  ")), "http://192.0.2.10:8096");
    }

    #[tokio::test]
    async fn the_servers_that_answer_are_found() {
        // A stand-in server on this machine, answering the probe.
        let responder = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let target = responder.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buffer = [0u8; 64];
            let (n, from) = responder.recv_from(&mut buffer).await.unwrap();
            assert_eq!(&buffer[..n], aoide_jellyfin::DISCOVERY_PROBE);
            let answer = br#"{"Id":"s1","Name":"Stand-in","Address":"http://192.0.2.10:8096"}"#;
            responder.send_to(answer, from).await.unwrap();
        });
        let found = aoide_jellyfin::discover_servers_at(target, Duration::from_millis(500))
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(label(&found[0]), "Stand-in");
    }
}
