//! Artists in the library, and each one's page: their albums, and their
//! portrait (Melete 0.9.0) with its credit. A portrait is a path on the
//! server, read with the token at a [`CoverSize`](crate::CoverSize) through
//! [`MeleteClient::poster_url`]; a server before 0.9.0 sends none.

use serde::Deserialize;

use crate::client::MeleteClient;
use crate::model::WireAlbum;
use crate::{Item, ItemKind, PosterCredit, Result};

/// `MusicArtist`, trimmed.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WireArtist {
    pub id: i64,
    pub name: String,
    pub albums: i32,
    /// Servers before Melete 0.9.0 send neither.
    #[serde(default)]
    pub portrait: Option<String>,
    #[serde(default)]
    pub portrait_credit: Option<PosterCredit>,
}

impl From<WireArtist> for Item {
    fn from(artist: WireArtist) -> Self {
        Self {
            id: artist.id.to_string(),
            name: artist.name,
            kind: ItemKind::Artist,
            child_count: Some(artist.albums),
            portrait: artist.portrait,
            portrait_credit: artist.portrait_credit,
            ..Self::default()
        }
    }
}

/// `MusicArtistDetail`: the artist, their own albums and the ones they
/// appear on.
#[derive(Deserialize)]
struct WireArtistDetail {
    #[serde(flatten)]
    artist: WireArtist,
    album_list: Vec<WireAlbum>,
    appears_on_list: Vec<WireAlbum>,
}

impl MeleteClient {
    /// An artist, with their portrait when the server has one, and their own
    /// albums, then the ones they appear on.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`](crate::Error::NotFound) for an artist the server
    /// doesn't have; otherwise the [request errors](MeleteClient#errors).
    pub async fn artist(&self, artist_id: &str) -> Result<(Item, Vec<Item>)> {
        let artist: WireArtistDetail = self
            .get(&["api", "v1", "music", "artists", artist_id], &[])
            .await?;
        let albums = artist
            .album_list
            .into_iter()
            .chain(artist.appears_on_list)
            .map(Item::from)
            .collect();
        Ok((Item::from(artist.artist), albums))
    }

    /// An artist's own albums, then the ones they appear on.
    ///
    /// # Errors
    ///
    /// As [`artist`](Self::artist).
    pub async fn artist_albums(&self, artist_id: &str) -> Result<Vec<Item>> {
        Ok(self.artist(artist_id).await?.1)
    }
}
