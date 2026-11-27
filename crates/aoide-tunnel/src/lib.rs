//! WireGuard to Melete's own tunnel, inside the app.
//!
//! Melete 0.10 answers WireGuard itself and serves its API through it to the
//! household's devices. This crate is that device's side, in userspace: GotaTun
//! for WireGuard, smoltcp for TCP, and a port on 127.0.0.1 whose connections
//! come out at Melete. The app talks to that port instead of the server's usual
//! address, so nothing on the system changes: no TUN device, no root, no VPN.

mod forward;
mod keys;
mod link;
mod pump;
mod stack;

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;

use gotatun::device::{Device, Peer};
use gotatun::udp::socket::UdpSocketFactory;
use gotatun::x25519::PublicKey;
use ipnetwork::{IpNetwork, Ipv4Network};
use tokio::net::TcpListener;
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;

pub use keys::{BadKey, PrivateKey, decode};

/// How often an idle tunnel tells Melete it's still there, as Melete's own
/// configs do, so a home router's UDP mapping stays open.
const KEEPALIVE_SECONDS: u16 = 25;

/// What Melete gave this device when it joined the tunnel, with its own key.
#[derive(Clone)]
pub struct Settings {
    pub private_key: PrivateKey,
    /// This device's address inside the tunnel.
    pub address: Ipv4Addr,
    pub server_public_key: [u8; 32],
    pub preshared_key: [u8; 32],
    /// Melete inside the tunnel: its address and HTTP port.
    pub server: SocketAddrV4,
    /// Where the tunnel is reached from outside, `host:port`.
    pub endpoint: String,
}

impl std::fmt::Debug for Settings {
    // The keys stay out of logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Settings")
            .field("address", &self.address)
            .field("server", &self.server)
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

/// Why the tunnel couldn't start.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("couldn't find {0}")]
    Endpoint(String),
    #[error("couldn't start WireGuard: {0}")]
    WireGuard(String),
    #[error("couldn't open a local port: {0}")]
    Local(#[from] std::io::Error),
}

type Transports = (UdpSocketFactory, link::ToStack, link::FromStack);

/// A running tunnel. Dropping it closes it, and every connection through it.
pub struct Tunnel {
    local: SocketAddr,
    _device: Device<Transports>,
    tasks: [JoinHandle<()>; 2],
}

impl Tunnel {
    /// Starts WireGuard to `settings.endpoint` and the local port. WireGuard's
    /// handshake happens with the first connection, so a tunnel that starts may
    /// still find Melete unreachable.
    ///
    /// # Errors
    ///
    /// [`Error::Endpoint`] when the endpoint's name doesn't resolve,
    /// [`Error::WireGuard`] when its UDP socket can't open, [`Error::Local`] when
    /// no local port can.
    pub async fn start(settings: &Settings) -> Result<Tunnel, Error> {
        let endpoint = resolve(&settings.endpoint).await?;
        let wake = Arc::new(Notify::new());
        let (to_stack, from_tunnel) = mpsc::channel(stack::QUEUE);
        let (to_tunnel, from_stack) = mpsc::channel(stack::QUEUE);
        let mut peer = Peer::new(PublicKey::from(settings.server_public_key))
            .with_endpoint(endpoint)
            .with_allowed_ip(IpNetwork::V4(
                Ipv4Network::new(*settings.server.ip(), 32).expect("32 is a valid prefix"),
            ))
            .with_preshared_key(settings.preshared_key);
        peer.keepalive = Some(KEEPALIVE_SECONDS);
        let ip_out = link::ToStack {
            packets: to_stack,
            wake: wake.clone(),
        };
        let ip_in = link::FromStack {
            packets: from_stack,
            mtu: stack::MTU,
        };
        let device = gotatun::device::build()
            .with_default_udp()
            .with_ip_pair(ip_out, ip_in)
            .with_private_key(settings.private_key.secret())
            .with_peer(peer)
            .build()
            .await
            .map_err(|e| Error::WireGuard(e.to_string()))?;

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let local = listener.local_addr()?;
        let (opened_tx, opened) = mpsc::channel(forward::AT_ONCE);
        let joins = stack::Joins {
            from_tunnel,
            to_tunnel,
            opened,
            wake: wake.clone(),
        };
        let stack = stack::Stack::new(settings.address, settings.server, joins);
        let tasks = [
            tokio::spawn(stack.run()),
            tokio::spawn(forward::serve(listener, opened_tx, wake)),
        ];
        log::info!(
            "tunnel to {} up, local port {}",
            settings.endpoint,
            local.port()
        );
        Ok(Tunnel {
            local,
            _device: device,
            tasks,
        })
    }

    /// The local address whose connections reach Melete: the app's server
    /// address while the tunnel is in use is `http://` this.
    pub fn local_address(&self) -> SocketAddr {
        self.local
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

/// The endpoint's first IPv4 address: Melete's tunnel listens on IPv4.
async fn resolve(endpoint: &str) -> Result<SocketAddr, Error> {
    let found = tokio::net::lookup_host(endpoint)
        .await
        .map_err(|_| Error::Endpoint(endpoint.to_owned()))?;
    let mut found: Vec<SocketAddr> = found.collect();
    found.sort_by_key(|a| !a.is_ipv4());
    found
        .into_iter()
        .next()
        .ok_or_else(|| Error::Endpoint(endpoint.to_owned()))
}

#[cfg(test)]
mod tests;
