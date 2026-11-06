//! Passwords: the signed-in user changing their own, and an administrator
//! giving another user a new one. Neither password is kept here, and `Debug`
//! never prints one.

use std::fmt;

use reqwest::Method;
use serde::Deserialize;

use crate::{MeleteClient, Result};

/// The shortest password Melete takes.
pub const PASSWORD_MIN_CHARS: usize = 8;
/// The longest password Melete takes.
pub const PASSWORD_MAX_CHARS: usize = 1024;

/// One of the server's users, as an administrator sees them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct User {
    /// Their user id.
    pub id: i64,
    /// Their username.
    pub username: String,
    /// Whether they administer the server.
    #[serde(default)]
    pub is_admin: bool,
    /// Where their profile picture is fetched from; `None` without one.
    #[serde(default)]
    pub avatar_url: Option<String>,
}

/// A new password the server made for a user, shown only once.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct NewPassword {
    /// Whose it is.
    pub username: String,
    /// The password itself. `Debug` leaves it out.
    pub password: String,
}

impl fmt::Debug for NewPassword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewPassword")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

impl MeleteClient {
    /// Changes the signed-in user's password. The server signs out every
    /// other session and API key of theirs; this client's token stays valid.
    ///
    /// # Errors
    ///
    /// HTTP 400 as [`Error::Http`](crate::Error::Http) for a new password
    /// shorter than [`PASSWORD_MIN_CHARS`] or longer than
    /// [`PASSWORD_MAX_CHARS`]; HTTP 403 for a wrong current password; HTTP 429
    /// after too many wrong ones; and the [request errors](MeleteClient#errors).
    pub async fn change_password(&self, current: &str, new: &str) -> Result<()> {
        let body = serde_json::json!({ "current_password": current, "new_password": new });
        self.send(
            Method::PUT,
            &["api", "v1", "me", "password"],
            &[],
            Some(&body),
        )
        .await
        .map(drop)
    }

    /// Every user of the server. Administrators only.
    ///
    /// # Errors
    ///
    /// HTTP 403 as [`Error::Http`](crate::Error::Http) for a user who isn't
    /// an administrator, and the [request errors](MeleteClient#errors).
    pub async fn users(&self) -> Result<Vec<User>> {
        self.get(&["api", "v1", "users"], &[]).await
    }

    /// Gives user `id` a new password the server makes, and signs them out
    /// of every session and API key. Administrators only, and not for their
    /// own account. The password is in this answer only.
    ///
    /// # Errors
    ///
    /// HTTP 403 as [`Error::Http`](crate::Error::Http) for a user who isn't
    /// an administrator; [`Error::NotFound`](crate::Error::NotFound) for a user
    /// the server doesn't have; HTTP 409 for one's own account; and the
    /// [request errors](MeleteClient#errors).
    pub async fn give_new_password(&self, id: i64) -> Result<NewPassword> {
        let id = id.to_string();
        self.send_decoded(
            Method::POST,
            &["api", "v1", "users", &id, "password"],
            &[],
            None,
        )
        .await
    }
}
