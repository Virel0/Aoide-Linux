//! Collections (Melete 0.8.0): a listener's saved song filters, made in
//! Melete's web, each a name for several genres, a span of years and "only
//! songs I like". They hold no music: their songs are what the songs list
//! gives for the filter now. Each user's are their own.

use reqwest::Method;
use serde::Deserialize;

use crate::model::{WirePage, WireTrack, nullable};
use crate::{Error, Item, MeleteClient, Page, Result};

/// The most songs Melete gives in one page.
pub const COLLECTION_PAGE_MAX: u32 = 500;

/// One saved filter, as `/api/v1/music/collections` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Collection {
    /// The server's id for it.
    pub id: i64,
    /// Its name, as the listener gave it.
    pub name: String,
    /// Songs of any of these genres (case aside); empty is any genre.
    #[serde(default, deserialize_with = "nullable")]
    pub genres: Vec<String>,
    /// Songs from this year on.
    #[serde(default)]
    pub year_from: Option<i32>,
    /// Songs up to this year.
    #[serde(default)]
    pub year_to: Option<i32>,
    /// Only the songs the listener likes.
    #[serde(default, deserialize_with = "nullable")]
    pub liked: bool,
    /// Unix seconds it was made.
    #[serde(default)]
    pub created_at: i64,
}

#[derive(Deserialize)]
struct WireCollections {
    items: Vec<Collection>,
}

impl Collection {
    /// The songs list's filters for it: `genres` joined by semicolons,
    /// `year_from`, `year_to` and `liked`, each left out when it doesn't
    /// narrow anything.
    pub fn filters(&self) -> Vec<(&'static str, String)> {
        let genres: Vec<&str> = self
            .genres
            .iter()
            .map(|g| g.trim())
            .filter(|g| !g.is_empty())
            .collect();
        let mut query = Vec::new();
        if !genres.is_empty() {
            query.push(("genres", genres.join(";")));
        }
        if let Some(year) = self.year_from {
            query.push(("year_from", year.to_string()));
        }
        if let Some(year) = self.year_to {
            query.push(("year_to", year.to_string()));
        }
        if self.liked {
            query.push(("liked", "true".to_owned()));
        }
        query
    }
}

impl MeleteClient {
    /// The signed-in user's collections, by name. A server before
    /// collections has none.
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors), but for a 404.
    pub async fn collections(&self) -> Result<Vec<Collection>> {
        let answer: Result<WireCollections> =
            self.get(&["api", "v1", "music", "collections"], &[]).await;
        match answer {
            Ok(list) => Ok(list.items),
            Err(Error::NotFound { .. }) => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }

    /// Deletes one of the signed-in user's collections.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] for one that is gone already (or never theirs);
    /// otherwise the [request errors](MeleteClient#errors).
    pub async fn delete_collection(&self, id: i64) -> Result<()> {
        let id = id.to_string();
        self.send_empty(
            Method::DELETE,
            &["api", "v1", "music", "collections", &id],
            &[],
        )
        .await
    }

    /// A page of `collection`'s songs in the songs list's own order (by
    /// artist, album, disc and number): `limit` of them (1 to
    /// [`COLLECTION_PAGE_MAX`]) from the `start`th.
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors).
    pub async fn collection_songs(
        &self,
        collection: &Collection,
        start: u32,
        limit: u32,
    ) -> Result<Page<Item>> {
        let mut query = collection.filters();
        query.push(("offset", start.to_string()));
        query.push(("limit", limit.clamp(1, COLLECTION_PAGE_MAX).to_string()));
        let page: WirePage<WireTrack> = self.get(&["api", "v1", "music", "tracks"], &query).await?;
        Ok(page.into_page(start))
    }

    /// Up to `limit` (1 to [`COLLECTION_PAGE_MAX`]) of `collection`'s songs
    /// in a random order over the whole collection (`sort=random`): what
    /// Shuffle plays. A new order each time; one page needs no seed.
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors).
    pub async fn collection_shuffled(
        &self,
        collection: &Collection,
        limit: u32,
    ) -> Result<Vec<Item>> {
        let mut query = collection.filters();
        query.push(("sort", "random".to_owned()));
        query.push(("offset", "0".to_owned()));
        query.push(("limit", limit.clamp(1, COLLECTION_PAGE_MAX).to_string()));
        let page: WirePage<WireTrack> = self.get(&["api", "v1", "music", "tracks"], &query).await?;
        Ok(page.into_page(0).items)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collection(json: &str) -> Collection {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn a_collection_reads_as_melete_sends_it() {
        let late_jazz = collection(
            r#"{"id":3,"name":"Late jazz","genres":["Jazz","Soul"],"year_from":1955,
                "year_to":1969,"liked":false,"created_at":1793000000}"#,
        );
        assert_eq!(late_jazz.id, 3);
        assert_eq!(late_jazz.genres, ["Jazz", "Soul"]);
        assert_eq!(
            (late_jazz.year_from, late_jazz.year_to),
            (Some(1955), Some(1969))
        );
        let liked = collection(
            r#"{"id":4,"name":"Mine","genres":null,"year_from":null,"year_to":null,
                "liked":true,"created_at":1}"#,
        );
        assert!(liked.genres.is_empty());
        assert!(liked.liked);
    }

    #[test]
    fn the_filters_leave_out_what_doesnt_narrow() {
        let mut filter = collection(
            r#"{"id":3,"name":"x","genres":["Jazz"," Soul ",""],"year_from":1955,
                "year_to":null,"liked":false,"created_at":0}"#,
        );
        assert_eq!(
            filter.filters(),
            [
                ("genres", "Jazz;Soul".to_owned()),
                ("year_from", "1955".to_owned())
            ]
        );
        filter.genres.clear();
        filter.year_from = None;
        filter.year_to = Some(1999);
        filter.liked = true;
        assert_eq!(
            filter.filters(),
            [("year_to", "1999".to_owned()), ("liked", "true".to_owned())]
        );
    }
}
