//! Two-factor sign-in from the account: whether it is on and what changed it,
//! setting it up (a secret for an authenticator app, then a first code), and
//! turning it off. Signing in with a code is [`MeleteClient::finish_sign_in`].
//!
//! A wrong password or code is HTTP 403 as [`Error::Http`](crate::Error::Http)
//! whose `reason` says which: [`WRONG_PASSWORD`] or [`WRONG_CODE`].

use std::fmt;

use reqwest::Method;
use serde::Deserialize;

use crate::{MeleteClient, Result};

/// The `reason` of a 403 for a wrong password.
pub const WRONG_PASSWORD: &str = "wrong_password";
/// The `reason` of a 403 for a wrong code.
pub const WRONG_CODE: &str = "wrong_code";

/// `GET /api/v1/me/two-factor`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TwoFactorStatus {
    /// Signing in with the password asks for a code.
    pub on: bool,
    /// Since when, in Unix seconds; `None` while it's off.
    pub on_since: Option<i64>,
    /// The latest changes, newest first (20 at most).
    #[serde(default)]
    pub changes: Vec<TwoFactorChange>,
}

/// One change to two-factor sign-in.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TwoFactorChange {
    /// What happened.
    pub change: ChangeKind,
    /// Where it was done.
    pub made_from: MadeFrom,
    /// When, in Unix seconds; for wrong codes, the latest.
    pub at: i64,
    /// 1, except for wrong codes counted together.
    #[serde(default = "one")]
    pub count: u64,
}

fn one() -> u64 {
    1
}

/// What a [`TwoFactorChange`] was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// Turned on.
    TwoFactorOn,
    /// Turned off.
    TwoFactorOff,
    /// Wrong codes typed at sign-in, after a right password.
    WrongSignInCode,
    /// A change this version doesn't know.
    #[serde(other)]
    Other,
}

/// Where a [`TwoFactorChange`] was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MadeFrom {
    /// The user's own account.
    Account,
    /// The server's command line.
    Server,
    /// Signing in.
    SignIn,
    /// A place this version doesn't know.
    #[serde(other)]
    Other,
}

/// A new secret for an authenticator app, valid until a first code confirms
/// it. Never to be kept: `Debug` prints none of it.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct TwoFactorSetup {
    /// 160 bits in base32, to type into an authenticator app.
    pub secret: String,
    /// The `otpauth://` link, for an app on this computer.
    pub uri: String,
    /// The link as a QR code, an SVG document; `None` for a link too long to
    /// draw.
    pub qr: Option<String>,
}

impl fmt::Debug for TwoFactorSetup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TwoFactorSetup")
            .field("secret", &"<redacted>")
            .field("uri", &"<redacted>")
            .field("qr", &self.qr.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl MeleteClient {
    /// Whether two-factor sign-in is on for the signed-in user, with its
    /// latest changes.
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors).
    pub async fn two_factor(&self) -> Result<TwoFactorStatus> {
        self.get(&["api", "v1", "me", "two-factor"], &[]).await
    }

    /// Starts setting two-factor sign-in up: a new secret, replacing one
    /// not yet confirmed. Signing in doesn't change until
    /// [`confirm_two_factor`](Self::confirm_two_factor).
    ///
    /// # Errors
    ///
    /// HTTP 403 as [`Error::Http`](crate::Error::Http) with reason
    /// [`WRONG_PASSWORD`]; HTTP 409 when it's on already; HTTP 429 after too
    /// many wrong passwords or codes (locked for 15 minutes); and the
    /// [request errors](MeleteClient#errors).
    pub async fn start_two_factor(&self, password: &str) -> Result<TwoFactorSetup> {
        let body = serde_json::json!({ "password": password });
        self.send_decoded(
            Method::POST,
            &["api", "v1", "me", "two-factor", "setup"],
            &[],
            Some(&body),
        )
        .await
    }

    /// Turns two-factor sign-in on with the six-digit `code` the
    /// authenticator app shows for the new secret. Nothing is signed out.
    ///
    /// # Errors
    ///
    /// HTTP 403 as [`Error::Http`](crate::Error::Http) with reason
    /// [`WRONG_CODE`]; HTTP 409 with no setup waiting, or when it's on
    /// already; HTTP 429 after too many wrong tries; and the
    /// [request errors](MeleteClient#errors).
    pub async fn confirm_two_factor(&self, code: &str) -> Result<()> {
        let body = serde_json::json!({ "code": code });
        self.send(
            Method::POST,
            &["api", "v1", "me", "two-factor", "confirm"],
            &[],
            Some(&body),
        )
        .await
        .map(drop)
    }

    /// Turns two-factor sign-in off, with the password and a current code.
    /// Nothing is signed out.
    ///
    /// # Errors
    ///
    /// HTTP 403 as [`Error::Http`](crate::Error::Http) with reason
    /// [`WRONG_PASSWORD`] or [`WRONG_CODE`]; HTTP 409 when it's off already;
    /// HTTP 429 after too many wrong tries; and the
    /// [request errors](MeleteClient#errors).
    pub async fn turn_off_two_factor(&self, password: &str, code: &str) -> Result<()> {
        let body = serde_json::json!({ "password": password, "code": code });
        self.send(
            Method::POST,
            &["api", "v1", "me", "two-factor", "off"],
            &[],
            Some(&body),
        )
        .await
        .map(drop)
    }
}
