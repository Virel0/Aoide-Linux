//! This device on Melete's own WireGuard tunnel (Melete 0.10): joining it,
//! seeing which of the user's devices are on it, and leaving it.

use reqwest::Method;
use serde::Deserialize;
use serde_json::json;

use crate::{MeleteClient, Result};

/// The parts of Melete a music app's device reaches through the tunnel.
const MUSIC_PARTS: [&str; 2] = ["music", "requests"];

/// The user's devices on the tunnel, and whether there is one.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TunnelDevices {
    /// Whether Melete's own tunnel is set up on this server
    /// (`MELETE_TUNNEL_PORT`), so whether a device can join.
    pub enabled: bool,
    /// The most devices one user has.
    pub limit: u32,
    /// The user's devices on it now.
    pub devices: Vec<TunnelDevice>,
}

/// One of the user's devices on the tunnel.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TunnelDevice {
    /// What [`leave_tunnel`](MeleteClient::leave_tunnel) takes.
    pub id: i64,
    /// As it joined, such as "Desk".
    pub name: String,
    /// Its address inside the tunnel, the same across restarts.
    pub address: Option<String>,
}

/// What Melete gives a device as it joins: all a WireGuard peer needs but
/// the device's own private key, which never left it.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct TunnelJoined {
    /// What [`leave_tunnel`](MeleteClient::leave_tunnel) takes.
    pub id: i64,
    /// The device's own address in the tunnel, such as `10.73.0.2`.
    pub address: String,
    /// Melete's WireGuard public key, base64.
    pub server_public_key: String,
    /// Melete as reached through the tunnel, such as `http://10.73.0.1`.
    pub server: String,
    /// Where the tunnel is reached from outside, `host:port`.
    pub endpoint: String,
    /// Shown this once; Melete keeps it sealed.
    pub preshared_key: String,
}

impl std::fmt::Debug for TunnelJoined {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunnelJoined")
            .field("id", &self.id)
            .field("address", &self.address)
            .field("server", &self.server)
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl MeleteClient {
    /// The user's devices on the tunnel, and whether the server has one.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`](crate::Error::NotFound) from a Melete older than
    /// 0.10, and the [request errors](MeleteClient#errors).
    pub async fn tunnel_devices(&self) -> Result<TunnelDevices> {
        self.get(&["api", "v1", "tunnel", "devices"], &[]).await
    }

    /// Joins the tunnel as `name`, reaching music and requests, with this
    /// device's own WireGuard `public_key` (base64).
    ///
    /// # Errors
    ///
    /// [`Error::Http`](crate::Error::Http) with 409 when the server has no
    /// tunnel (or already has this key), 429 when the user has as many devices
    /// on it as it takes, and the [request errors](MeleteClient#errors).
    pub async fn join_tunnel(&self, name: &str, public_key: &str) -> Result<TunnelJoined> {
        let body = json!({ "name": name, "parts": MUSIC_PARTS, "public_key": public_key });
        self.send_decoded(
            Method::POST,
            &["api", "v1", "tunnel", "devices"],
            &[],
            Some(&body),
        )
        .await
    }

    /// Takes device `id` off the tunnel, cutting it off at once.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`](crate::Error::NotFound) when it's gone already, and
    /// the [request errors](MeleteClient#errors).
    pub async fn leave_tunnel(&self, id: i64) -> Result<()> {
        let id = id.to_string();
        self.send_empty(
            Method::DELETE,
            &["api", "v1", "tunnel", "devices", &id],
            &[],
        )
        .await
    }
}
