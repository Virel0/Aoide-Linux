//! What a list can be filtered by, for a filter screen and saved filters.
//!
//! Jellyfin has no saved filters of its own (its "collections" are box sets, a
//! movie feature). A saved filter is the app's to keep: a [`ListQuery`] with
//! `genre_ids`, `years`, `tags` and `favorites_only` set, run through
//! [`songs`](JellyfinClient::songs) or [`albums`](JellyfinClient::albums).
//! [`JellyfinClient::query_filters`] says which values are there to pick from.
//!
//! [`ListQuery`]: crate::ListQuery

use serde::Deserialize;

use crate::client::JellyfinClient;
use crate::model::nullable;
use crate::{Error, ItemKind, NameId, Result};

/// The genres, years and tags present among the items of a library or folder.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QueryFilters {
    /// With ids, for [`ListQuery::genre_ids`](crate::ListQuery::genre_ids).
    pub genres: Vec<NameId>,
    /// Production years, for [`ListQuery::years`](crate::ListQuery::years).
    pub years: Vec<i32>,
    /// For [`ListQuery::tags`](crate::ListQuery::tags).
    pub tags: Vec<String>,
}

/// `/Items/Filters2`: genres with their ids, and tags.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Filters {
    #[serde(default, deserialize_with = "nullable")]
    genres: Vec<NameId>,
    #[serde(default, deserialize_with = "nullable")]
    tags: Vec<String>,
}

/// `/Items/Filters`, the older endpoint: the only one that lists years.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LegacyFilters {
    #[serde(default, deserialize_with = "nullable")]
    years: Vec<i32>,
}

impl JellyfinClient {
    /// The filter values present under `parent_id` (a music library or folder;
    /// `None` is everything the user can see), among items of `kinds` (empty is
    /// every kind; pass `[ItemKind::MusicAlbum]` for an album filter screen).
    ///
    /// Two requests, made together: `/Items/Filters2` for genres with ids and
    /// tags, and `/Items/Filters` for years, which only it reports. A server that
    /// has dropped the older one (a 404) still gives genres and tags, with no years.
    pub async fn query_filters(
        &self,
        parent_id: Option<&str>,
        kinds: &[ItemKind],
    ) -> Result<QueryFilters> {
        let mut query = vec![("userId", self.user_id()?.to_owned())];
        if let Some(parent_id) = parent_id {
            query.push(("parentId", parent_id.to_owned()));
        }
        let types: Vec<&str> = kinds.iter().filter_map(|kind| kind.type_name()).collect();
        if !types.is_empty() {
            query.push(("includeItemTypes", types.join(",")));
        }
        let mut current = query.clone();
        current.push(("recursive", "true".to_owned()));

        let (filters, legacy) = tokio::join!(
            self.get::<Filters>(&["Items", "Filters2"], &current),
            self.get::<LegacyFilters>(&["Items", "Filters"], &query),
        );
        let filters = filters?;
        let mut years = match legacy {
            Ok(legacy) => legacy.years,
            Err(Error::NotFound { .. }) => Vec::new(),
            Err(error) => return Err(error),
        };
        // Newest first, as a year picker reads.
        years.sort_unstable_by(|a, b| b.cmp(a));
        years.dedup();
        Ok(QueryFilters {
            genres: filters.genres,
            years,
            tags: filters.tags,
        })
    }
}
