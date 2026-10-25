//! Finding Jellyfin servers on the local network.
//!
//! Jellyfin listens on UDP port 7359 for the text `Who is JellyfinServer?` and
//! answers the sender with a small JSON object naming itself. That probe is the
//! only thing sent here; nothing is sent until [`discover_servers`] is called.

use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use serde::Deserialize;
use tokio::net::UdpSocket;
use tokio::time::{Instant, timeout_at};

use crate::{Error, Result, ServerAddress};

/// The port Jellyfin's discovery listens on. Not configurable on the server.
pub const DISCOVERY_PORT: u16 = 7359;

/// The probe, byte for byte. The server matches it ignoring case.
pub const DISCOVERY_PROBE: &[u8] = b"Who is JellyfinServer?";

/// Big enough for any answer: a server's reply is a few hundred bytes.
const MAX_REPLY: usize = 4096;

/// A server that answered the probe.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DiscoveredServer {
    /// The server's id, the same as [`PublicSystemInfo::id`](crate::PublicSystemInfo::id).
    pub id: String,
    /// The name set in the server's dashboard.
    #[serde(default)]
    pub name: String,
    /// The address the server gives for itself, such as `http://192.0.2.10:8096`.
    pub address: String,
    /// The address the probe reached it on, when the server reports one.
    #[serde(default)]
    pub endpoint_address: Option<String>,
}

impl DiscoveredServer {
    /// The reported address, normalised, ready for
    /// [`JellyfinClient::new`](crate::JellyfinClient::new).
    pub fn server_address(&self) -> Result<ServerAddress> {
        ServerAddress::parse(&self.address)
    }
}

/// Broadcasts the probe on the local IPv4 network and gathers the servers that
/// answer within `wait`. One second is plenty on a home network.
///
/// Each server appears once, in the order they answered, however many answers
/// it sends (one per network interface, say). Datagrams that aren't a server's
/// answer are ignored. An empty list means nothing answered: no server, or one
/// with discovery turned off, or a firewall in between.
pub async fn discover_servers(wait: Duration) -> Result<Vec<DiscoveredServer>> {
    discover_servers_at(
        SocketAddr::from((Ipv4Addr::BROADCAST, DISCOVERY_PORT)),
        wait,
    )
    .await
}

/// [`discover_servers`] with the probe sent to `target` instead of the
/// broadcast address: a subnet's broadcast address, or one host.
pub async fn discover_servers_at(
    target: SocketAddr,
    wait: Duration,
) -> Result<Vec<DiscoveredServer>> {
    let deadline = Instant::now() + wait;
    let bind: SocketAddr = match target {
        SocketAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        SocketAddr::V6(_) => (std::net::Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let socket = UdpSocket::bind(bind).await.map_err(io)?;
    socket.set_broadcast(true).map_err(io)?;
    socket.send_to(DISCOVERY_PROBE, target).await.map_err(io)?;

    let mut seen = HashSet::new();
    let mut servers = Vec::new();
    let mut buffer = [0u8; MAX_REPLY];
    loop {
        let received = match timeout_at(deadline, socket.recv_from(&mut buffer)).await {
            Err(_elapsed) => break,
            Ok(received) => received,
        };
        let length = match received {
            Ok((length, _from)) => length,
            // A host that isn't listening can answer with an ICMP "port
            // unreachable", which some systems surface here. Keep listening for
            // the others.
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => continue,
            Err(error) => return Err(io(error)),
        };
        let Ok(server) = serde_json::from_slice::<DiscoveredServer>(&buffer[..length]) else {
            continue;
        };
        if server.id.trim().is_empty() || server.address.trim().is_empty() {
            continue;
        }
        if seen.insert(server.id.clone()) {
            servers.push(server);
        }
    }
    Ok(servers)
}

fn io(error: std::io::Error) -> Error {
    Error::Transport(format!("LAN discovery: {error}"))
}
