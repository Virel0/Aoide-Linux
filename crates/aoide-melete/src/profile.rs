//! Profile pictures. Melete gives each person's as `avatar_url`: a path on
//! the server (`/api/v1/users/{id}/avatar?v=…`) whose query carries the
//! picture's hash, so a new picture has a new address. Fetching it needs the
//! token, as a cover does. A server from before profile pictures sends no
//! `avatar_url`, which reads as none.

use serde::Deserialize;

use crate::{MeleteClient, Result, Url};

/// `GET /api/v1/me`: the signed-in user, as far as the app needs them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Me {
    /// Where their profile picture is fetched from; `None` without one, or
    /// from a server that has no profile pictures.
    #[serde(default)]
    pub avatar_url: Option<String>,
}

impl MeleteClient {
    /// The signed-in user, for their profile picture.
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors).
    pub async fn me(&self) -> Result<Me> {
        self.get(&["api", "v1", "me"], &[]).await
    }

    /// A profile picture's address, from the `avatar_url` the server gave,
    /// beneath the server's own address so a reverse proxy's prefix is kept.
    /// Fetching it needs [`authorization`](Self::authorization).
    ///
    /// `None` for anything but a plain absolute path on this server with a
    /// plain query (letters, digits, `=`, `&`, `-`, `_`, `.`).
    pub fn avatar_url(&self, path: &str) -> Option<Url> {
        let (path, query) = match path.split_once('?') {
            Some((path, query)) => (path, Some(query)),
            None => (path, None),
        };
        let plain = |query: &str| {
            query
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "=&-_.".contains(c))
        };
        if query.is_some_and(|q| q.is_empty() || !plain(q)) {
            return None;
        }
        let mut url = self.address().join_path(path)?;
        url.set_query(query);
        Some(url)
    }
}
