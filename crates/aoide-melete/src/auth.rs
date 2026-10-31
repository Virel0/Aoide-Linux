use std::fmt;

use reqwest::Method;
use serde::Deserialize;
use serde_json::json;

use crate::client::MeleteClient;
use crate::{Error, Result};

/// Identifies this install. `name` is what sign-in shows in the server's device
/// list; `id` names this install to sync and the event stream, so it must stay the
/// same across launches.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Device {
    /// A name a person recognises the machine by, such as its hostname.
    pub name: String,
    /// An id made once per install and kept.
    pub id: String,
}

impl Device {
    /// A device called `name`, with the install's `id`.
    pub fn new(name: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            id: id.into(),
        }
    }

    /// The name as sign-in sends it: Melete takes 1 to 100 characters, so a blank
    /// name becomes "Aoide" and a long one is cut at a character boundary.
    pub(crate) fn sign_in_name(&self) -> String {
        let name = self.name.trim();
        if name.is_empty() {
            return "Aoide".to_owned();
        }
        name.chars().take(100).collect()
    }
}

/// A signed-in user. The access token is the secret: keep it in the system keyring,
/// never in a plain settings file. `Debug` leaves it out.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Credentials {
    /// The user's id on the server, from the sign-in answer, as text.
    pub user_id: String,
    /// The session token (`mlt_…`) the server issued at sign-in. Sent with every
    /// request until [`sign_out`](MeleteClient::sign_out) revokes it.
    pub access_token: String,
    /// The user's name on the server.
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

/// `GET /api/v1/health`. Needs no sign-in, which is what makes it the probe for
/// "is this really a Melete server?".
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ServerInfo {
    /// `ok` on a server that's up.
    pub status: String,
    /// Melete's version, such as `0.1.0`.
    pub version: String,
}

/// What Quick Connect shows while it waits: the `code` for the person to approve
/// in Melete's web UI, and the `secret` only this device polls with.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct QuickConnectCode {
    /// Shown on screen, to be typed into the web UI.
    pub code: String,
    /// Sent with each poll; never shown. `Debug` leaves it out.
    pub secret: String,
    /// Seconds until the code runs out.
    pub expires_in: u64,
}

impl fmt::Debug for QuickConnectCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QuickConnectCode")
            .field("code", &self.code)
            .field("secret", &"<redacted>")
            .field("expires_in", &self.expires_in)
            .finish()
    }
}

/// Another user of the server, by id and name: what sharing a playlist names
/// them by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserName {
    /// Their id, as text.
    pub id: String,
    /// Their username.
    pub name: String,
}

#[derive(Deserialize)]
struct WireUserName {
    id: i64,
    username: String,
}

/// `LoginResponse`: the token, shown only here, and who it signs in.
#[derive(Deserialize)]
struct Session {
    token: String,
    user: SessionUser,
}

#[derive(Deserialize)]
struct SessionUser {
    id: i64,
    username: String,
}

impl From<Session> for Credentials {
    fn from(session: Session) -> Self {
        Self {
            user_id: session.user.id.to_string(),
            access_token: session.token,
            username: session.user.username,
        }
    }
}

impl MeleteClient {
    /// Confirms the address really is a Melete server, before asking for a password.
    ///
    /// Anything that answers but not like Melete (an HTML page, a 404 from a proxy,
    /// JSON without `status: ok` and a version) is [`Error::NotMelete`]. Server
    /// errors and transport failures come back as themselves.
    ///
    /// # Errors
    ///
    /// [`Error::NotMelete`], carrying the host, as above;
    /// [`Error::Unauthorized`], [`Error::Http`] and [`Error::Transport`] as the
    /// server or network gave them. No sign-in is needed, so never
    /// [`Error::NotSignedIn`].
    pub async fn server_info(&self) -> Result<ServerInfo> {
        let not_melete = || Error::NotMelete(self.address().host().to_owned());
        match self.get::<ServerInfo>(&["api", "v1", "health"], &[]).await {
            Ok(info) if info.status != "ok" || info.version.trim().is_empty() => Err(not_melete()),
            Ok(info) => Ok(info),
            Err(Error::Decoding { .. } | Error::NotFound { .. }) => Err(not_melete()),
            Err(e) => Err(e),
        }
    }

    /// Signs in with a username and password, as this client's [`Device`]. A
    /// wrong password and an unknown username are both [`Error::Unauthorized`].
    ///
    /// # Errors
    ///
    /// [`Error::Unauthorized`] for a wrong username or password;
    /// [`Error::SecondFactorNeeded`] for a right password from a user with
    /// two-factor sign-in on, to finish with
    /// [`finish_sign_in`](Self::finish_sign_in); HTTP 429 as
    /// [`Error::Http`] after five wrong tries (the pair is locked for 15
    /// minutes); otherwise the [request errors](MeleteClient#errors).
    pub async fn sign_in(&self, username: &str, password: &str) -> Result<Credentials> {
        let body = json!({
            "username": username,
            "password": password,
            "device_name": self.device().sign_in_name(),
        });
        let session: Session = self
            .send_decoded(
                Method::POST,
                &["api", "v1", "auth", "login"],
                &[],
                Some(&body),
            )
            .await?;
        Ok(session.into())
    }

    /// Finishes a two-factor sign-in with `challenge` (from
    /// [`Error::SecondFactorNeeded`]) and the six-digit code from the user's
    /// authenticator app.
    ///
    /// # Errors
    ///
    /// [`Error::Unauthorized`] for a wrong code, which leaves the challenge for
    /// another try; HTTP 410 as [`Error::Http`] once the challenge has run out or
    /// was used (sign in with the password again); HTTP 429 after five wrong
    /// codes (the user's codes are locked for 15 minutes); otherwise the
    /// [request errors](MeleteClient#errors).
    pub async fn finish_sign_in(&self, challenge: &str, code: &str) -> Result<Credentials> {
        let body = json!({ "challenge": challenge, "code": code.trim() });
        let session: Session = self
            .send_decoded(
                Method::POST,
                &["api", "v1", "auth", "login", "second-factor"],
                &[],
                Some(&body),
            )
            .await?;
        Ok(session.into())
    }

    /// Starts Quick Connect: show the code, then
    /// [`poll_quick_connect`](Self::poll_quick_connect) with the secret every few
    /// seconds until someone approves it in Melete's web UI.
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors).
    pub async fn start_quick_connect(&self) -> Result<QuickConnectCode> {
        let body = json!({ "device_name": self.device().sign_in_name() });
        self.send_decoded(
            Method::POST,
            &["api", "v1", "auth", "quick-connect"],
            &[],
            Some(&body),
        )
        .await
    }

    /// Asks once whether the code was approved: `Some` with this device's session
    /// once it was (and only that once), `None` while it waits.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] once the code has run out or was used: start again.
    /// Otherwise the [request errors](MeleteClient#errors).
    pub async fn poll_quick_connect(&self, secret: &str) -> Result<Option<Credentials>> {
        let body = json!({ "secret": secret });
        let path = ["api", "v1", "auth", "quick-connect", "poll"];
        let (status, bytes) = self
            .send_with_status(Method::POST, &path, &[], Some(&body))
            .await?;
        if status == 202 {
            return Ok(None);
        }
        let session: Session = serde_json::from_slice(&bytes).map_err(|e| Error::Decoding {
            path: path.join("/"),
            message: e.to_string(),
        })?;
        Ok(Some(session.into()))
    }

    /// Every user's name on the server, for choosing whom to share with. Any
    /// signed-in user may list them; nothing else about them is given.
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors).
    pub async fn user_directory(&self) -> Result<Vec<UserName>> {
        let users: Vec<WireUserName> = self.get(&["api", "v1", "users", "directory"], &[]).await?;
        Ok(users
            .into_iter()
            .map(|user| UserName {
                id: user.id.to_string(),
                name: user.username,
            })
            .collect())
    }

    /// Ends the session on the server, which revokes the token. Best-effort: forgetting
    /// the credentials locally is what actually signs the person out, so callers can
    /// ignore a failure here.
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors). A signed-out client sends no
    /// token, which the server answers with [`Error::Unauthorized`].
    pub async fn sign_out(&self) -> Result<()> {
        self.send(Method::POST, &["api", "v1", "auth", "logout"], &[], None)
            .await
            .map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_device_name_signs_in_as_aoide() {
        assert_eq!(Device::new("  ", "id").sign_in_name(), "Aoide");
    }

    #[test]
    fn a_long_device_name_is_cut_to_a_hundred_characters() {
        let name = "é".repeat(150);
        assert_eq!(Device::new(name, "id").sign_in_name(), "é".repeat(100));
    }

    #[test]
    fn debug_never_prints_the_token() {
        let credentials = Credentials {
            user_id: "1".into(),
            access_token: "do-not-print".into(),
            username: "alice".into(),
        };
        let printed = format!("{credentials:?}");
        assert!(!printed.contains("do-not-print"));
        assert!(printed.contains("alice"));
    }

    #[test]
    fn debug_never_prints_the_quick_connect_secret() {
        let code = QuickConnectCode {
            code: "123456".into(),
            secret: "do-not-print".into(),
            expires_in: 300,
        };
        let printed = format!("{code:?}");
        assert!(!printed.contains("do-not-print"));
        assert!(printed.contains("123456"));
    }

    #[test]
    fn a_session_becomes_credentials() {
        let session: Session = serde_json::from_str(
            r#"{"token":"mlt_abc","user":{"id":7,"username":"alice","is_admin":false}}"#,
        )
        .unwrap();
        let credentials = Credentials::from(session);
        assert_eq!(credentials.user_id, "7");
        assert_eq!(credentials.access_token, "mlt_abc");
        assert_eq!(credentials.username, "alice");
    }
}
