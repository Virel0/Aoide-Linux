//! Songs and albums by several genres, years and likes at once, for
//! Collections, Search by Description and Mixes. Melete narrows by words and
//! one genre per request; any of several genres is a request each, and years
//! and likes are applied here to what comes back.

use std::collections::HashSet;
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;

use aoide_melete::{Item, ListQuery, MeleteClient, Sort};

/// Songs per request when sampling the library.
const SAMPLE_PAGE: u32 = 25;

/// What a filter keeps.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SongFilter {
    /// Part of a title, artist or album; blank is ignored.
    pub term: Option<String>,
    /// Any of these genres, by name; empty is every genre.
    pub genres: Vec<String>,
    /// Any of these years; empty is every year.
    pub years: Vec<i32>,
    /// Only songs the listener likes (for albums: albums with one).
    pub liked_only: bool,
    pub sort: Sort,
}

/// What a filter screen offers to pick from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LibraryFilters {
    /// Every genre, most songs first.
    pub genres: Vec<String>,
    /// The years the library's albums are from, oldest first.
    pub years: Vec<i32>,
}

/// The genres and years the library has.
///
/// # Errors
///
/// The client's request errors.
pub async fn library_filters(client: &MeleteClient) -> aoide_melete::Result<LibraryFilters> {
    let every_album = ListQuery::default();
    let (genres, albums) = tokio::try_join!(client.genres(), client.albums(&every_album))?;
    let mut years: Vec<i32> = albums.items.iter().filter_map(|album| album.year).collect();
    years.sort_unstable();
    years.dedup();
    Ok(LibraryFilters {
        genres: genres.into_iter().map(|genre| genre.name).collect(),
        years,
    })
}

/// About `count` songs from all over the library: pages read at random
/// places, since Melete has no random order. Fewer when the library is small.
///
/// # Errors
///
/// The client's request errors.
pub async fn sample(client: &MeleteClient, count: usize) -> aoide_melete::Result<Vec<Item>> {
    let probe = ListQuery {
        limit: Some(1),
        ..ListQuery::default()
    };
    let total = client.songs(&probe).await?.total;
    let random = RandomState::new();
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    let pages = count.div_ceil(SAMPLE_PAGE as usize);
    for page in 0..pages {
        let room = total.saturating_sub(SAMPLE_PAGE);
        let start = if room == 0 {
            0
        } else {
            (random.hash_one(page) % u64::from(room + 1)) as u32
        };
        let query = ListQuery {
            start_index: start,
            limit: Some(SAMPLE_PAGE),
            ..ListQuery::default()
        };
        found.extend(
            client
                .songs(&query)
                .await?
                .items
                .into_iter()
                .filter(|song| seen.insert(song.id.clone())),
        );
        if room == 0 {
            break;
        }
    }
    Ok(found)
}

/// About `count` albums from anywhere in the library: a page at a random
/// place, since Melete has no random order.
///
/// # Errors
///
/// The client's request errors.
pub async fn sample_albums(client: &MeleteClient, count: usize) -> aoide_melete::Result<Vec<Item>> {
    let probe = ListQuery {
        limit: Some(1),
        ..ListQuery::default()
    };
    let total = client.albums(&probe).await?.total;
    let count = u32::try_from(count).unwrap_or(u32::MAX);
    let room = total.saturating_sub(count);
    let start = if room == 0 {
        0
    } else {
        (RandomState::new().hash_one(total) % u64::from(room + 1)) as u32
    };
    let query = ListQuery {
        start_index: start,
        limit: Some(count),
        ..ListQuery::default()
    };
    Ok(client.albums(&query).await?.items)
}

impl SongFilter {
    /// Whether anything is kept here rather than by the server, so whole
    /// lists must be read before `limit` is applied.
    fn narrows_here(&self) -> bool {
        !self.years.is_empty() || self.liked_only || self.genres.len() > 1
    }

    /// One list query per genre (one for every genre when none is named).
    fn queries(&self, limit: Option<usize>) -> Vec<ListQuery> {
        let limit = if self.narrows_here() {
            None
        } else {
            limit.map(|n| u32::try_from(n).unwrap_or(u32::MAX))
        };
        let base = ListQuery {
            search_term: self.term.clone(),
            sort: self.sort,
            limit,
            ..ListQuery::default()
        };
        if self.genres.is_empty() {
            return vec![base];
        }
        self.genres
            .iter()
            .map(|genre| ListQuery {
                genre: Some(genre.clone()),
                ..base.clone()
            })
            .collect()
    }

    /// Whether `item` is from one of the filter's years (any, when none).
    pub fn keeps_year(&self, item: &Item) -> bool {
        self.years.is_empty() || item.year.is_some_and(|year| self.years.contains(&year))
    }

    /// The songs the filter keeps, each once, in the server's order genre by
    /// genre, at most `limit`.
    ///
    /// # Errors
    ///
    /// The client's request errors.
    pub async fn songs(
        &self,
        client: &MeleteClient,
        limit: Option<usize>,
    ) -> aoide_melete::Result<Vec<Item>> {
        let mut found = Vec::new();
        for query in self.queries(limit) {
            found.extend(client.songs(&query).await?.items);
        }
        Ok(self.keep(found, |song| song.liked, limit))
    }

    /// The albums the filter keeps, each once, at most `limit`.
    ///
    /// # Errors
    ///
    /// The client's request errors.
    pub async fn albums(
        &self,
        client: &MeleteClient,
        limit: Option<usize>,
    ) -> aoide_melete::Result<Vec<Item>> {
        let liked_albums: HashSet<String> = if self.liked_only {
            client
                .liked_songs()
                .await?
                .into_iter()
                .filter_map(|song| song.album_id)
                .collect()
        } else {
            HashSet::new()
        };
        let mut found = Vec::new();
        for query in self.queries(limit) {
            found.extend(client.albums(&query).await?.items);
        }
        Ok(self.keep(found, |album| liked_albums.contains(&album.id), limit))
    }

    /// `found` without repeats, the years and likes applied, cut to `limit`.
    fn keep(
        &self,
        found: Vec<Item>,
        liked: impl Fn(&Item) -> bool,
        limit: Option<usize>,
    ) -> Vec<Item> {
        let mut seen = HashSet::new();
        found
            .into_iter()
            .filter(|item| seen.insert(item.id.clone()))
            .filter(|item| self.keeps_year(item))
            .filter(|item| !self.liked_only || liked(item))
            .take(limit.unwrap_or(usize::MAX))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, year: Option<i32>, liked: bool) -> Item {
        Item {
            id: id.into(),
            year,
            liked,
            ..Item::default()
        }
    }

    #[test]
    fn one_genre_or_none_is_asked_with_the_limit() {
        let filter = SongFilter {
            genres: vec!["Folk".into()],
            ..SongFilter::default()
        };
        let queries = filter.queries(Some(50));
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].genre.as_deref(), Some("Folk"));
        assert_eq!(queries[0].limit, Some(50));
        assert_eq!(SongFilter::default().queries(None)[0].genre, None);
    }

    #[test]
    fn several_genres_years_or_likes_read_whole_lists() {
        let genres = SongFilter {
            genres: vec!["Folk".into(), "Jazz".into()],
            ..SongFilter::default()
        };
        let queries = genres.queries(Some(50));
        assert_eq!(queries.len(), 2);
        assert!(queries.iter().all(|q| q.limit.is_none()));
        let years = SongFilter {
            years: vec![2020],
            ..SongFilter::default()
        };
        assert_eq!(years.queries(Some(50))[0].limit, None);
    }

    #[test]
    fn what_comes_back_is_kept_once_by_year_and_like() {
        let filter = SongFilter {
            years: vec![2019, 2022],
            liked_only: true,
            ..SongFilter::default()
        };
        let found = vec![
            item("1", Some(2019), true),
            item("2", Some(2020), true),
            item("3", Some(2022), false),
            item("1", Some(2019), true),
            item("4", None, true),
            item("5", Some(2022), true),
        ];
        let kept = filter.keep(found, |i| i.liked, Some(10));
        let ids: Vec<&str> = kept.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["1", "5"]);
    }

    #[test]
    fn the_limit_is_applied_last() {
        let found = (1..=5).map(|n| item(&n.to_string(), None, false)).collect();
        assert_eq!(
            SongFilter::default().keep(found, |_| false, Some(2)).len(),
            2
        );
    }
}
