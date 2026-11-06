//! The server's gallery of profile pictures: Melete's own (`official`), then
//! one category per folder of the server's gallery. Nothing in it comes from
//! outside the server.

use serde::Deserialize;

use crate::profile::AVATAR_MAX_BYTES;
use crate::{Error, MeleteClient, Result};

/// One of the gallery's categories.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AvatarCategory {
    /// `official`, or `gallery/` and the folder's name: what
    /// [`gallery_pictures`](MeleteClient::gallery_pictures) asks by.
    pub id: String,
    /// As shown: "Melete" for Melete's own.
    pub name: String,
    /// How many pictures it has.
    pub count: u64,
}

/// One picture in the gallery.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GalleryPicture {
    /// What [`set_gallery_avatar`](MeleteClient::set_gallery_avatar) and
    /// [`gallery_picture`](MeleteClient::gallery_picture) take.
    pub id: String,
}

/// A page of a category's pictures.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GalleryPage {
    /// How many the category has in all.
    pub total: u64,
    /// This page of them.
    pub pictures: Vec<GalleryPicture>,
}

#[derive(Deserialize)]
struct Categories {
    categories: Vec<AvatarCategory>,
}

impl MeleteClient {
    /// The gallery's categories, Melete's own first; empty ones are left out.
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors).
    pub async fn avatar_categories(&self) -> Result<Vec<AvatarCategory>> {
        let found: Categories = self.get(&["api", "v1", "avatars"], &[]).await?;
        Ok(found.categories)
    }

    /// Up to `limit` (1–200) of `category`'s pictures, from `offset`.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] for a category the gallery doesn't have, and the
    /// [request errors](MeleteClient#errors).
    pub async fn gallery_pictures(
        &self,
        category: &str,
        offset: u64,
        limit: u64,
    ) -> Result<GalleryPage> {
        let query = [
            ("category", category.to_owned()),
            ("offset", offset.to_string()),
            ("limit", limit.clamp(1, 200).to_string()),
        ];
        self.get(&["api", "v1", "avatars", "pictures"], &query)
            .await
    }

    /// The bytes of the gallery's picture `id`, read with the token, to show.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] for a picture the gallery doesn't have;
    /// [`Error::Decoding`] for one larger than any profile picture; and the
    /// [request errors](MeleteClient#errors).
    pub async fn gallery_picture(&self, id: &str) -> Result<Vec<u8>> {
        let segments = ["api", "v1", "avatars", "picture"];
        let path = segments.join("/");
        let url = self.url(&segments, &[("id", id.to_owned())]);
        self.download(url, AVATAR_MAX_BYTES, &path)
            .await?
            .ok_or_else(|| Error::Decoding {
                path,
                message: "the picture is larger than any profile picture".to_owned(),
            })
    }
}
