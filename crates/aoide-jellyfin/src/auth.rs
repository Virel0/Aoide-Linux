use std::fmt;

use reqwest::Method;
use serde::Deserialize;
use serde_json::json;

use crate::client::JellyfinClient;
use crate::{Error, Result};

/// The client name Jellyfin shows in its devices dashboard.
pub(crate) const CLIENT_NAME: &str = "Aoide";
/// The version reported to the server.
pub(crate) const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Identifies this install to the server. Jellyfin lists `name` in its devices
/// dashboard and keys sessions and playback reports off `id`, so `id` must stay the
/// same across launches.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Device {
    pub name: String,
    pub id: String,
}

impl Device {
    pub fn new(name: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            id: id.into(),
        }
    }

    /// The `Authorization` header value:
    /// `MediaBrowser Client="Aoide", Device="…", DeviceId="…", Version="…"`, plus
    /// `Token="…"` when there is one.
    pub fn authorization_header(&self, token: Option<&str>) -> String {
        let mut header = format!(
            "MediaBrowser Client=\"{}\", Device=\"{}\", DeviceId=\"{}\", Version=\"{}\"",
            escape(CLIENT_NAME),
            escape(&self.name),
            escape(&self.id),
            escape(CLIENT_VERSION),
        );
        if let Some(token) = token {
            header.push_str(&format!(", Token=\"{}\"", escape(token)));
        }
        header
    }
}

/// The header is a list of quoted strings, so a `"` in a device name (people name
/// their machines anything) would corrupt every request. Control characters would
/// make the header unsendable at all, so they become spaces.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// A signed-in user. The access token is the secret: keep it in the system keyring,
/// never in a plain settings file. `Debug` leaves it out.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Credentials {
    pub user_id: String,
    pub access_token: String,
    pub username: String,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("user_id", &self.user_id)
            .field("access_token", &"<redacted>")
            .field("username", &self.username)
            .finish()
    }
}

/// `GET /System/Info/Public`. Needs no sign-in, which is what makes it the probe for
/// "is this really a Jellyfin server?".
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PublicSystemInfo {
    /// The server's id. Required: every Jellyfin server sends one.
    pub id: String,
    /// Required, for the same reason.
    pub version: String,
    #[serde(default)]
    pub server_name: Option<String>,
    #[serde(default)]
    pub product_name: Option<String>,
    #[serde(default)]
    pub startup_wizard_completed: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AuthenticationResult {
    user: AuthenticatedUser,
    access_token: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AuthenticatedUser {
    id: String,
    #[serde(default)]
    name: Option<String>,
}

impl JellyfinClient {
    /// Confirms the address really is a Jellyfin server, before asking for a password.
    ///
    /// Anything that answers but not like Jellyfin (an HTML page, a 404 from a proxy,
    /// JSON without an id and version) is [`Error::NotJellyfin`]. Server errors and
    /// transport failures come back as themselves.
    pub async fn public_system_info(&self) -> Result<PublicSystemInfo> {
        let not_jellyfin = || Error::NotJellyfin(self.address().host().to_owned());
        match self
            .get::<PublicSystemInfo>(&["System", "Info", "Public"], &[])
            .await
        {
            Ok(info) if info.id.trim().is_empty() || info.version.trim().is_empty() => {
                Err(not_jellyfin())
            }
            Ok(info) => Ok(info),
            Err(Error::Decoding { .. } | Error::NotFound { .. }) => Err(not_jellyfin()),
            Err(e) => Err(e),
        }
    }

    /// Signs in with a username and password. A wrong password is
    /// [`Error::Unauthorized`].
    pub async fn authenticate_by_name(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Credentials> {
        // PascalCase, and the password field really is called `Pw`.
        let body = json!({ "Username": username, "Pw": password });
        let result: AuthenticationResult = self
            .send_decoded(
                Method::POST,
                &["Users", "AuthenticateByName"],
                &[],
                Some(&body),
            )
            .await?;
        Ok(Credentials {
            username: result
                .user
                .name
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| username.to_owned()),
            user_id: result.user.id,
            access_token: result.access_token,
        })
    }

    /// Ends the session on the server, which revokes the token. Best-effort: forgetting
    /// the credentials locally is what actually signs the person out, so callers can
    /// ignore a failure here.
    pub async fn sign_out(&self) -> Result<()> {
        self.send(Method::POST, &["Sessions", "Logout"], &[], None)
            .await
            .map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> Device {
        Device::new("Test", "test-device")
    }

    #[test]
    fn the_header_has_the_mediabrowser_shape() {
        assert_eq!(
            device().authorization_header(None),
            format!(
                "MediaBrowser Client=\"Aoide\", Device=\"Test\", DeviceId=\"test-device\", Version=\"{CLIENT_VERSION}\""
            )
        );
    }

    #[test]
    fn the_token_is_included_when_signed_in() {
        let header = device().authorization_header(Some("secret"));
        assert!(header.starts_with("MediaBrowser "));
        assert!(header.ends_with(", Token=\"secret\""));
    }

    #[test]
    fn the_token_is_omitted_entirely_when_signed_out() {
        assert!(!device().authorization_header(None).contains("Token"));
    }

    #[test]
    fn quotes_in_a_device_name_are_escaped() {
        let header = Device::new(r#"Alex's "Best" Laptop"#, "abc").authorization_header(None);
        assert!(
            header.contains(r#"Device="Alex's \"Best\" Laptop""#),
            "{header}"
        );
    }

    #[test]
    fn backslashes_and_control_characters_cant_break_the_header() {
        let header = Device::new("a\\b\nc", "abc").authorization_header(None);
        assert!(header.contains(r#"Device="a\\b c""#), "{header}");
        assert!(!header.chars().any(char::is_control));
    }

    #[test]
    fn debug_never_prints_the_token() {
        let credentials = Credentials {
            user_id: "u".into(),
            access_token: "do-not-print".into(),
            username: "alice".into(),
        };
        let printed = format!("{credentials:?}");
        assert!(!printed.contains("do-not-print"));
        assert!(printed.contains("alice"));
    }

    #[test]
    fn public_info_decodes_from_a_real_shape() {
        let info: PublicSystemInfo =
            serde_json::from_str(include_str!("../tests/fixtures/system_info_public.json"))
                .unwrap();
        assert_eq!(info.id, "0f5d3a7c9b1e4d2f8a6c5b4e3d2c1b0a");
        assert_eq!(info.version, "10.11.2");
        assert_eq!(info.product_name.as_deref(), Some("Jellyfin Server"));
    }

    #[test]
    fn public_info_without_an_id_is_rejected() {
        assert!(serde_json::from_str::<PublicSystemInfo>(r#"{"Version":"1.0"}"#).is_err());
        assert!(serde_json::from_str::<PublicSystemInfo>("{}").is_err());
    }
}
