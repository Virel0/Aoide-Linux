//! Remote access: reaching Melete through its own WireGuard tunnel when its
//! usual address doesn't answer, as away from home, with no VPN app or system
//! change. The device joins the tunnel once, from Settings → Account, while it
//! can reach Melete; its private key never leaves it.

mod watch;

use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;

use aoide_melete::{MeleteClient, TunnelJoined};
use aoide_tunnel::{PrivateKey, Settings};

use crate::config::RemoteAccess;

pub use watch::subscription;

/// How the app reaches Melete right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Way {
    /// At its usual address.
    Direct,
    /// Through the tunnel: the usual address didn't answer.
    Tunnel,
    /// Neither answered; why the tunnel didn't help.
    Neither(String),
}

/// Remote access for the signed-in account.
#[derive(Debug, Default)]
pub struct State {
    /// The tunnel's settings with their keys, once read from the keyring.
    pub settings: Option<Arc<Settings>>,
    /// A signed-out client for the usual address alone, to ask whether it answers.
    pub probe: Option<MeleteClient>,
    /// As the watch last found it.
    pub way: Option<Way>,
    /// Joining or leaving is under way.
    pub busy: bool,
    /// Why joining, leaving or reading the keys failed.
    pub problem: Option<String>,
    /// Whether Melete has a tunnel to join, once asked.
    pub offered: Option<bool>,
}

/// The keyring's words for the two keys Melete's config doesn't keep.
pub fn secret(key: &PrivateKey, preshared: &str) -> String {
    format!("{} {}", key.to_base64(), preshared.trim())
}

/// The settings for the tunnel, from the config and the keyring's secret.
/// `None` for anything that doesn't read back as it was written.
pub fn settings(remote: &RemoteAccess, secret: &str) -> Option<Settings> {
    let (private, preshared) = secret.split_once(' ')?;
    Some(Settings {
        private_key: PrivateKey::from_base64(private).ok()?,
        address: remote.address.parse().ok()?,
        server_public_key: aoide_tunnel::decode(&remote.server_public_key).ok()?,
        preshared_key: aoide_tunnel::decode(preshared).ok()?,
        server: server_of(&remote.server)?,
        endpoint: remote.endpoint.clone(),
    })
}

/// What the config keeps of a device that has just joined.
pub fn remembered(account: &str, joined: &TunnelJoined) -> RemoteAccess {
    RemoteAccess {
        account: account.to_owned(),
        device_id: joined.id,
        address: joined.address.clone(),
        server_public_key: joined.server_public_key.clone(),
        server: joined.server.clone(),
        endpoint: joined.endpoint.clone(),
    }
}

/// Melete's address and port inside the tunnel, from its `http://10.73.0.1`.
fn server_of(server: &str) -> Option<SocketAddrV4> {
    let url = reqwest::Url::parse(server).ok()?;
    let ip: Ipv4Addr = url.host_str()?.parse().ok()?;
    Some(SocketAddrV4::new(ip, url.port_or_known_default()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined() -> TunnelJoined {
        serde_json::from_value(serde_json::json!({
            "id": 4, "address": "10.73.0.5", "server": "http://10.73.0.1",
            "server_public_key": PrivateKey::generate().unwrap().public_base64(),
            "endpoint": "melete.example:51820",
            "preshared_key": PrivateKey::generate().unwrap().to_base64(),
        }))
        .unwrap()
    }

    #[test]
    fn a_joined_device_reads_back_from_the_config_and_keyring() {
        let (joined, key) = (joined(), PrivateKey::generate().unwrap());
        let remote = remembered("1@https://music.example", &joined);
        let read = settings(&remote, &secret(&key, &joined.preshared_key)).unwrap();
        assert_eq!(
            read.server,
            SocketAddrV4::new(Ipv4Addr::new(10, 73, 0, 1), 80)
        );
        assert_eq!(read.address, Ipv4Addr::new(10, 73, 0, 5));
        assert_eq!(read.private_key.public_base64(), key.public_base64());
        assert_eq!(read.endpoint, "melete.example:51820");
    }

    #[test]
    fn a_damaged_secret_reads_as_none() {
        let remote = remembered("1@https://music.example", &joined());
        assert!(settings(&remote, "only-one-part").is_none());
        assert!(settings(&remote, "bad bad").is_none());
    }

    #[test]
    fn the_config_never_holds_a_key() {
        let joined = joined();
        let text = serde_json::to_string(&remembered("1@x", &joined)).unwrap();
        assert!(!text.contains(&joined.preshared_key));
    }
}
