use std::collections::HashMap;

use reqwest::Method;
use serde::Deserialize;

use crate::client::MeleteClient;
use crate::model::{WireAlbum, WireArtist, WireGenre, WirePage, WireTrack};
use crate::{Item, Page, Result};

/// How many ids go in one `?id=` request: Melete takes at most 100.
pub(crate) const MAX_IDS_PER_REQUEST: usize = 100;

/// The largest page Melete serves; a list asked for whole is read in pages this
/// big.
const MAX_PAGE: u32 = 500;

/// Sort orders, named for how the UI describes them. Each list takes the ones
/// Melete offers it and reads the rest as its default; Melete chooses the
/// direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Sort {
    /// A list's own order: tracks by artist, album, disc and number; albums by
    /// artist; artists by name. The default.
    #[default]
    Default,
    /// By title or name, a leading "The", "A" or "An" aside.
    Name,
    /// Albums by album artist.
    Artist,
    /// Albums by year, newest first.
    Year,
    /// Newest in the library first.
    Recent,
}

impl Sort {
    /// The `sort` value for tracks, if this order applies to them.
    fn for_tracks(self) -> Option<&'static str> {
        match self {
            Sort::Name => Some("title"),
            Sort::Recent => Some("recent"),
            Sort::Default | Sort::Artist | Sort::Year => None,
        }
    }

    /// The `sort` value for albums.
    fn for_albums(self) -> Option<&'static str> {
        match self {
            Sort::Name => Some("title"),
            Sort::Artist => Some("artist"),
            Sort::Year => Some("year"),
            Sort::Recent => Some("recent"),
            Sort::Default => None,
        }
    }

    /// The `sort` value for artists.
    fn for_artists(self) -> Option<&'static str> {
        match self {
            Sort::Recent => Some("recent"),
            Sort::Default | Sort::Name | Sort::Artist | Sort::Year => None,
        }
    }
}

/// Which page of a list, in what order, filtered by words, a genre or an artist.
/// The default is the first page of everything, in the list's own order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListQuery {
    /// Part of a title or name, case, accents and punctuation aside; blank is
    /// ignored.
    pub search_term: Option<String>,
    /// Only tracks (or albums with a track) of this genre, case aside.
    pub genre: Option<String>,
    /// Only these artists' tracks or albums.
    pub artist_ids: Vec<String>,
    /// What to sort by.
    pub sort: Sort,
    /// How many items to skip: the 0-based index of the page's first item.
    pub start_index: u32,
    /// `None` asks for everything.
    pub limit: Option<u32>,
}

/// What a search found, by kind.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SearchResults {
    /// Tracks whose title, artist or album match.
    pub songs: Vec<Item>,
    /// Albums whose title or artist match.
    pub albums: Vec<Item>,
    /// Artists whose names match.
    pub artists: Vec<Item>,
}

/// What Melete's Home view has, worked out on the server from this listener's
/// own history.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Home {
    /// Albums played lately, newest first.
    pub recently_played: Vec<Item>,
    /// The tracks played most in the last 30 days (skips not counted), with how
    /// many times.
    pub most_played: Vec<(Item, u32)>,
    /// Albums added lately, newest first.
    pub recently_added: Vec<Item>,
}

#[derive(Deserialize)]
struct WireAlbumDetail {
    #[serde(flatten)]
    album: WireAlbum,
    track_list: Vec<WireTrack>,
}

#[derive(Deserialize)]
struct WireArtistDetail {
    album_list: Vec<WireAlbum>,
    appears_on_list: Vec<WireAlbum>,
}

#[derive(Deserialize)]
struct WireHome {
    recently_played: Vec<WireAlbum>,
    most_played: Vec<WireMostPlayed>,
    recently_added: Vec<WireAlbum>,
}

#[derive(Deserialize)]
struct WireMostPlayed {
    plays: u32,
    track: WireTrack,
}

#[derive(Deserialize)]
struct WireLikes {
    tracks: Vec<WireLike>,
}

#[derive(Deserialize)]
struct WireLike {
    track: Option<WireTrack>,
}

type Query = Vec<(&'static str, String)>;

impl MeleteClient {
    /// One page of albums, as `query` asks.
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors).
    pub async fn albums(&self, query: &ListQuery) -> Result<Page<Item>> {
        let sort = query.sort.for_albums();
        self.list::<WireAlbum>("albums", query, sort).await
    }

    /// One page of tracks, as `query` asks.
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors).
    pub async fn songs(&self, query: &ListQuery) -> Result<Page<Item>> {
        let sort = query.sort.for_tracks();
        self.list::<WireTrack>("tracks", query, sort).await
    }

    /// Artists with an album in the library, or a track crediting them (a featured
    /// artist, one of a duet). The genre and artist filters don't apply.
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors).
    pub async fn artists(&self, query: &ListQuery) -> Result<Page<Item>> {
        let sort = query.sort.for_artists();
        let query = ListQuery {
            genre: None,
            artist_ids: Vec::new(),
            ..query.clone()
        };
        self.list::<WireArtist>("artists", &query, sort).await
    }

    /// Every genre in the tracks' tags, most tracks first. Each is an
    /// [`ItemKind::Genre`](crate::ItemKind::Genre) item whose id is its name, as
    /// [`ListQuery::genre`] takes it.
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors).
    pub async fn genres(&self) -> Result<Vec<Item>> {
        let genres: Vec<WireGenre> = self.get(&["api", "v1", "music", "genres"], &[]).await?;
        Ok(genres.into_iter().map(Item::from).collect())
    }

    /// Tracks by id, in the order asked for.
    ///
    /// Asked for 100 ids at a time. The server leaves out ids it no longer has (a
    /// file deleted since a playlist was saved); the result is in the requested
    /// order and a missing id is simply absent, which is how a caller tells an
    /// entry has dangled. An id asked for twice appears twice.
    ///
    /// # Errors
    ///
    /// No ids is `Ok(vec![])` without a request. Otherwise the [request
    /// errors](MeleteClient#errors) from any chunk; the chunks already fetched
    /// are dropped.
    pub async fn items_by_ids<S: AsRef<str>>(&self, ids: &[S]) -> Result<Vec<Item>> {
        let mut found: HashMap<String, Item> = HashMap::new();
        for chunk in ids.chunks(MAX_IDS_PER_REQUEST) {
            let joined = chunk
                .iter()
                .map(AsRef::as_ref)
                .collect::<Vec<_>>()
                .join(",");
            let query = vec![("id", joined), ("limit", MAX_IDS_PER_REQUEST.to_string())];
            let page: WirePage<WireTrack> =
                self.get(&["api", "v1", "music", "tracks"], &query).await?;
            for track in page.items {
                let item = Item::from(track);
                found.insert(item.id.clone(), item);
            }
        }
        Ok(ids
            .iter()
            .filter_map(|id| found.get(id.as_ref()).cloned())
            .collect())
    }

    /// An album, and its tracks in disc and track order.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`](crate::Error::NotFound) for an album the server
    /// doesn't have; otherwise the [request errors](MeleteClient#errors).
    pub async fn album(&self, album_id: &str) -> Result<(Item, Vec<Item>)> {
        let album: WireAlbumDetail = self
            .get(&["api", "v1", "music", "albums", album_id], &[])
            .await?;
        Ok((
            Item::from(album.album),
            album.track_list.into_iter().map(Item::from).collect(),
        ))
    }

    /// An album's tracks, in disc and track order.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`](crate::Error::NotFound) for an album the server
    /// doesn't have; otherwise the [request errors](MeleteClient#errors).
    pub async fn album_tracks(&self, album_id: &str) -> Result<Vec<Item>> {
        Ok(self.album(album_id).await?.1)
    }

    /// An artist's own albums, then the ones they appear on.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`](crate::Error::NotFound) for an artist the server
    /// doesn't have; otherwise the [request errors](MeleteClient#errors).
    pub async fn artist_albums(&self, artist_id: &str) -> Result<Vec<Item>> {
        let artist: WireArtistDetail = self
            .get(&["api", "v1", "music", "artists", artist_id], &[])
            .await?;
        Ok(artist
            .album_list
            .into_iter()
            .chain(artist.appears_on_list)
            .map(Item::from)
            .collect())
    }

    /// Tracks, albums and artists matching `term`, up to `limit` of each.
    ///
    /// # Errors
    ///
    /// A blank `term` is `Ok` with nothing, without a request. Otherwise the
    /// [request errors](MeleteClient#errors) from any of the three requests.
    pub async fn search(&self, term: &str, limit: u32) -> Result<SearchResults> {
        let term = term.trim();
        if term.is_empty() {
            return Ok(SearchResults::default());
        }
        let query = ListQuery {
            search_term: Some(term.to_owned()),
            limit: Some(limit),
            ..ListQuery::default()
        };
        let (songs, albums, artists) = tokio::try_join!(
            self.songs(&query),
            self.albums(&query),
            self.artists(&query),
        )?;
        Ok(SearchResults {
            songs: songs.items,
            albums: albums.items,
            artists: artists.items,
        })
    }

    /// The Home view: recently played and added albums, and the most played
    /// tracks.
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors).
    pub async fn home(&self) -> Result<Home> {
        let home: WireHome = self.get(&["api", "v1", "music", "home"], &[]).await?;
        Ok(Home {
            recently_played: home.recently_played.into_iter().map(Item::from).collect(),
            most_played: home
                .most_played
                .into_iter()
                .map(|most| (Item::from(most.track), most.plays))
                .collect(),
            recently_added: home.recently_added.into_iter().map(Item::from).collect(),
        })
    }

    /// The tracks the signed-in user likes, newest like first, as their sync log
    /// says; a liked track gone from the library is left out.
    ///
    /// # Errors
    ///
    /// HTTP 403 as [`Error::Http`](crate::Error::Http) for a user who may see only
    /// some music libraries; otherwise the [request errors](MeleteClient#errors).
    pub async fn liked_songs(&self) -> Result<Vec<Item>> {
        let likes: WireLikes = self.get(&["api", "v1", "music", "likes"], &[]).await?;
        Ok(likes
            .tracks
            .into_iter()
            .filter_map(|like| like.track.map(Item::from))
            .collect())
    }

    /// Likes or unlikes a track, written into the user's sync log as one of their
    /// devices would; the local store sees it on its next pull.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`](crate::Error::NotFound) for a track the server doesn't
    /// have; otherwise the [request errors](MeleteClient#errors).
    pub async fn set_liked(&self, track_id: &str, liked: bool) -> Result<()> {
        let method = if liked { Method::PUT } else { Method::DELETE };
        self.send_empty(
            method,
            &["api", "v1", "music", "tracks", track_id, "like"],
            &[],
        )
        .await
    }

    /// A list's page, or the whole list read 500 at a time when the query has no
    /// limit.
    async fn list<W>(
        &self,
        list: &'static str,
        query: &ListQuery,
        sort: Option<&'static str>,
    ) -> Result<Page<Item>>
    where
        W: for<'de> Deserialize<'de>,
        Item: From<W>,
    {
        let segments = ["api", "v1", "music", list];
        let mut filters = Query::new();
        if let Some(term) = query.search_term.as_deref().map(str::trim)
            && !term.is_empty()
        {
            filters.push(("q", term.to_owned()));
        }
        if let Some(genre) = query.genre.as_deref().filter(|g| !g.trim().is_empty()) {
            filters.push(("genre", genre.to_owned()));
        }
        if !query.artist_ids.is_empty() {
            filters.push(("artist_id", query.artist_ids.join(",")));
        }
        if let Some(sort) = sort {
            filters.push(("sort", sort.to_owned()));
        }

        let one_page = |offset: u32, limit: u32| {
            let mut page = filters.clone();
            page.push(("offset", offset.to_string()));
            page.push(("limit", limit.to_string()));
            page
        };
        if let Some(limit) = query.limit {
            let page: WirePage<W> = self
                .get(
                    &segments,
                    &one_page(query.start_index, limit.clamp(1, MAX_PAGE)),
                )
                .await?;
            return Ok(page.into_page(query.start_index));
        }
        let mut whole = Page {
            items: Vec::new(),
            total: 0,
            start_index: query.start_index,
        };
        loop {
            let offset = query.start_index + whole.items.len() as u32;
            let page: WirePage<W> = self.get(&segments, &one_page(offset, MAX_PAGE)).await?;
            let page = page.into_page(offset);
            whole.total = page.total;
            let got = page.items.len();
            whole.items.extend(page.items);
            if got == 0 || query.start_index + whole.items.len() as u32 >= whole.total {
                return Ok(whole);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_list_takes_the_orders_it_has() {
        assert_eq!(Sort::Name.for_tracks(), Some("title"));
        assert_eq!(Sort::Year.for_tracks(), None);
        assert_eq!(Sort::Year.for_albums(), Some("year"));
        assert_eq!(Sort::Artist.for_albums(), Some("artist"));
        assert_eq!(
            Sort::Name.for_artists(),
            None,
            "name is the artists' default"
        );
        assert_eq!(Sort::Recent.for_artists(), Some("recent"));
        assert_eq!(Sort::Default.for_albums(), None);
    }
}
