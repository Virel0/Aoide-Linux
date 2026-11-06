//! Profile pictures. Melete gives each person's as `avatar_url`: a path on
//! the server (`/api/v1/users/{id}/avatar?v=…`) whose query carries the
//! picture's hash, so a new picture has a new address. Fetching it needs the
//! token, as a cover does. A server from before profile pictures sends no
//! `avatar_url`, which reads as none.
//!
//! The signed-in user sets their own from a file ([`MeleteClient::set_avatar`]),
//! from the server's gallery ([`MeleteClient::set_gallery_avatar`]), or takes
//! it off ([`MeleteClient::remove_avatar`]).

use reqwest::Method;
use serde::Deserialize;

use crate::{MeleteClient, Result, Url};

/// The largest picture Melete takes as a profile picture: 5 MB.
pub const AVATAR_MAX_BYTES: usize = 5 * 1024 * 1024;

/// `GET /api/v1/me`: the signed-in user, as far as the app needs them.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Me {
    /// Their user id; 0 from a server that doesn't say.
    #[serde(default)]
    pub id: i64,
    /// Their username; empty from a server that doesn't say.
    #[serde(default)]
    pub username: String,
    /// Whether they administer the server.
    #[serde(default)]
    pub is_admin: bool,
    /// Where their profile picture is fetched from; `None` without one, or
    /// from a server that has no profile pictures.
    #[serde(default)]
    pub avatar_url: Option<String>,
}

/// The kinds of picture Melete takes as a profile picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PictureType {
    /// `image/jpeg`.
    Jpeg,
    /// `image/png`.
    Png,
    /// `image/webp`.
    Webp,
}

impl PictureType {
    /// The type `bytes` are, by how they start, as Melete tells them; `None`
    /// for anything else.
    pub fn of(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(Self::Jpeg)
        } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
            Some(Self::Webp)
        } else {
            None
        }
    }

    /// Its media type.
    pub fn media_type(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Webp => "image/webp",
        }
    }
}

/// Melete's answer to setting or removing a picture.
#[derive(Deserialize)]
struct AvatarSet {
    avatar_url: Option<String>,
}

impl MeleteClient {
    /// The signed-in user: who they are, whether they administer the server,
    /// and their profile picture.
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors).
    pub async fn me(&self) -> Result<Me> {
        self.get(&["api", "v1", "me"], &[]).await
    }

    /// Makes `bytes`, a picture of type `kind`, the signed-in user's profile
    /// picture. Answers its address now (see [`avatar_url`](Self::avatar_url)).
    ///
    /// # Errors
    ///
    /// HTTP 400 as [`Error::Http`](crate::Error::Http) for bytes that are
    /// empty or not a JPEG, PNG or WebP picture of that type; HTTP 413 for one
    /// over [`AVATAR_MAX_BYTES`]; and the [request errors](MeleteClient#errors).
    pub async fn set_avatar(&self, bytes: &[u8], kind: PictureType) -> Result<Option<String>> {
        let answer: AvatarSet = self
            .send_file(
                Method::PUT,
                &["api", "v1", "me", "avatar"],
                bytes,
                kind.media_type(),
            )
            .await?;
        Ok(answer.avatar_url)
    }

    /// Makes the gallery's picture `id` (from
    /// [`gallery_pictures`](Self::gallery_pictures)) the signed-in user's
    /// profile picture. Answers its address now.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`](crate::Error::NotFound) for a picture the gallery
    /// doesn't have; HTTP 422 as [`Error::Http`](crate::Error::Http) for one
    /// the server can't use; and the [request errors](MeleteClient#errors).
    pub async fn set_gallery_avatar(&self, id: &str) -> Result<Option<String>> {
        let body = serde_json::json!({ "id": id });
        let answer: AvatarSet = self
            .send_decoded(
                Method::PUT,
                &["api", "v1", "me", "avatar", "gallery"],
                &[],
                Some(&body),
            )
            .await?;
        Ok(answer.avatar_url)
    }

    /// Takes the signed-in user's profile picture off. Answers its address
    /// now, which is `None` from Melete.
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors).
    pub async fn remove_avatar(&self) -> Result<Option<String>> {
        let answer: AvatarSet = self
            .send_decoded(Method::DELETE, &["api", "v1", "me", "avatar"], &[], None)
            .await?;
        Ok(answer.avatar_url)
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

#[cfg(test)]
mod tests {
    use super::PictureType;

    #[test]
    fn a_picture_is_told_by_how_it_starts() {
        assert_eq!(
            PictureType::of(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(PictureType::Jpeg)
        );
        assert_eq!(
            PictureType::of(b"\x89PNG\r\n\x1a\n...."),
            Some(PictureType::Png)
        );
        assert_eq!(
            PictureType::of(b"RIFF\x10\0\0\0WEBPVP8 "),
            Some(PictureType::Webp)
        );
        assert_eq!(PictureType::of(b"RIFF\x10\0\0\0WAVEfmt "), None);
        assert_eq!(PictureType::of(b"GIF89a"), None);
        assert_eq!(PictureType::of(b""), None);
    }
}
