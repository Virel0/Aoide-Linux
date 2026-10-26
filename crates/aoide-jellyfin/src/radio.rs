//! Radio: Instant Mix stations by seed, and similar items.

use crate::client::JellyfinClient;
use crate::library::STANDARD_FIELDS;
use crate::{Item, Page, Result};

/// What a station starts from. Each kind has its own Instant Mix endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MixSeed {
    /// A track: `/Songs/{id}/InstantMix`.
    Track(String),
    /// `/Albums/{id}/InstantMix`.
    Album(String),
    /// `/Artists/{id}/InstantMix`.
    Artist(String),
    /// A genre by its item id: `/MusicGenres/InstantMix?id=`.
    Genre(String),
    /// `/Playlists/{id}/InstantMix`.
    Playlist(String),
}

impl JellyfinClient {
    /// A station of up to `limit` tracks seeded by `seed`.
    ///
    /// [`instant_mix`](Self::instant_mix) is the kind-blind `/Items/{id}/InstantMix`;
    /// this asks the endpoint the API documents for the seed's kind.
    ///
    /// # Errors
    ///
    /// [`Error::NotSignedIn`](crate::Error::NotSignedIn) without credentials,
    /// before any request; otherwise the [request errors](JellyfinClient#errors).
    pub async fn instant_mix_from(&self, seed: &MixSeed, limit: u32) -> Result<Vec<Item>> {
        let mut query = vec![
            ("userId", self.user_id()?.to_owned()),
            ("limit", limit.to_string()),
            ("fields", STANDARD_FIELDS.to_owned()),
        ];
        let page: Page<Item> = match seed {
            MixSeed::Track(id) => self.get(&["Songs", id, "InstantMix"], &query).await?,
            MixSeed::Album(id) => self.get(&["Albums", id, "InstantMix"], &query).await?,
            MixSeed::Artist(id) => self.get(&["Artists", id, "InstantMix"], &query).await?,
            MixSeed::Playlist(id) => self.get(&["Playlists", id, "InstantMix"], &query).await?,
            MixSeed::Genre(id) => {
                query.push(("id", id.clone()));
                self.get(&["MusicGenres", "InstantMix"], &query).await?
            }
        };
        Ok(page.items)
    }

    /// Up to `limit` items like this one, of whatever kind it is.
    ///
    /// # Errors
    ///
    /// [`Error::NotSignedIn`](crate::Error::NotSignedIn) without credentials,
    /// before any request; otherwise the [request errors](JellyfinClient#errors).
    pub async fn similar_items(&self, item_id: &str, limit: u32) -> Result<Vec<Item>> {
        self.similar("Items", item_id, limit).await
    }

    /// Up to `limit` albums like this album.
    ///
    /// # Errors
    ///
    /// [`Error::NotSignedIn`](crate::Error::NotSignedIn) without credentials,
    /// before any request; otherwise the [request errors](JellyfinClient#errors).
    pub async fn similar_albums(&self, album_id: &str, limit: u32) -> Result<Vec<Item>> {
        self.similar("Albums", album_id, limit).await
    }

    /// Up to `limit` artists like this artist.
    ///
    /// # Errors
    ///
    /// [`Error::NotSignedIn`](crate::Error::NotSignedIn) without credentials,
    /// before any request; otherwise the [request errors](JellyfinClient#errors).
    pub async fn similar_artists(&self, artist_id: &str, limit: u32) -> Result<Vec<Item>> {
        self.similar("Artists", artist_id, limit).await
    }

    async fn similar(&self, kind: &str, id: &str, limit: u32) -> Result<Vec<Item>> {
        let query = [
            ("userId", self.user_id()?.to_owned()),
            ("limit", limit.to_string()),
            ("fields", STANDARD_FIELDS.to_owned()),
        ];
        let page: Page<Item> = self.get(&[kind, id, "Similar"], &query).await?;
        Ok(page.items)
    }
}
