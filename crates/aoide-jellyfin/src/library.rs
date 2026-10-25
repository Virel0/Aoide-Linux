use std::collections::HashMap;

use reqwest::Method;

use crate::client::JellyfinClient;
use crate::{Item, Page, Result, UserData};

/// Fields missing from Jellyfin's default item response that a music client needs.
/// Asked for per call rather than everything, to keep big library pages small.
pub(crate) const STANDARD_FIELDS: &str =
    "Genres,AlbumArtist,Artists,ParentId,ChildCount,PrimaryImageAspectRatio";

/// How many ids go in one `/Items?ids=` request. A few hundred 32-character ids
/// overruns what proxies in front of Jellyfin forward, and that fails as a 414.
pub(crate) const MAX_IDS_PER_REQUEST: usize = 100;

/// Sort orders, named for how the UI describes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Sort {
    #[default]
    Name,
    DateAdded,
    DatePlayed,
    PlayCount,
    Artist,
    ReleaseDate,
    /// Unseeded on the server: every page is a new shuffle, so only use it unpaged.
    Random,
    /// Disc, then track, then title: the order an album plays in.
    AlbumTrackOrder,
}

impl Sort {
    /// The server's `sortBy` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Sort::Name => "SortName",
            Sort::DateAdded => "DateCreated",
            Sort::DatePlayed => "DatePlayed",
            Sort::PlayCount => "PlayCount",
            Sort::Artist => "AlbumArtist,SortName",
            Sort::ReleaseDate => "PremiereDate,ProductionYear,SortName",
            Sort::Random => "Random",
            Sort::AlbumTrackOrder => "ParentIndexNumber,IndexNumber,SortName",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SortOrder {
    #[default]
    Ascending,
    Descending,
}

impl SortOrder {
    /// The server's `sortOrder` value.
    pub fn as_str(self) -> &'static str {
        match self {
            SortOrder::Ascending => "Ascending",
            SortOrder::Descending => "Descending",
        }
    }
}

/// Which page of a list, in what order, optionally within one library and
/// filtered by name. The default is the first page of everything, by name.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ListQuery {
    /// A library (or folder) to stay inside.
    pub parent_id: Option<String>,
    /// Matches names only; blank is ignored.
    pub search_term: Option<String>,
    /// Only items in any of these genres (Jellyfin's `genreIds`, by genre
    /// item id); empty is every genre.
    pub genre_ids: Vec<String>,
    /// Only the signed-in user's favourites (Jellyfin's `isFavorite=true`). Works
    /// for every list: [`songs`](JellyfinClient::songs),
    /// [`albums`](JellyfinClient::albums), [`artists`](JellyfinClient::artists),
    /// [`album_artists`](JellyfinClient::album_artists), and the rest.
    pub favorites_only: bool,
    pub sort: Sort,
    pub order: SortOrder,
    pub start_index: u32,
    /// `None` asks for everything.
    pub limit: Option<u32>,
}

/// What a search found, by kind.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SearchResults {
    pub songs: Vec<Item>,
    pub albums: Vec<Item>,
    pub artists: Vec<Item>,
}

type Query = Vec<(&'static str, String)>;

impl JellyfinClient {
    /// The signed-in user's music libraries. A server may have several, or none
    /// the user can see.
    pub async fn music_libraries(&self) -> Result<Vec<Item>> {
        let page: Page<Item> = self
            .get(&["UserViews"], &[("userId", self.user_id()?.to_owned())])
            .await?;
        Ok(page
            .items
            .into_iter()
            .filter(|view| view.collection_type.as_deref() == Some("music"))
            .collect())
    }

    pub async fn albums(&self, query: &ListQuery) -> Result<Page<Item>> {
        self.items("MusicAlbum", query, None).await
    }

    pub async fn songs(&self, query: &ListQuery) -> Result<Page<Item>> {
        self.items("Audio", query, None).await
    }

    /// Every artist, including ones only featured on a track.
    pub async fn artists(&self, query: &ListQuery) -> Result<Page<Item>> {
        self.by_name(&["Artists"], query).await
    }

    /// Artists credited with an album, which is what an artist page can show.
    pub async fn album_artists(&self, query: &ListQuery) -> Result<Page<Item>> {
        self.by_name(&["Artists", "AlbumArtists"], query).await
    }

    /// Playlists. Jellyfin keeps them in their own top-level folder, so a music
    /// library's id as `parent_id` finds none: leave it unset.
    pub async fn playlists(&self, query: &ListQuery) -> Result<Page<Item>> {
        self.items("Playlist", query, None).await
    }

    /// A playlist's entries, in playlist order.
    pub async fn playlist_items(&self, playlist_id: &str) -> Result<Vec<Item>> {
        let query = [
            ("userId", self.user_id()?.to_owned()),
            ("fields", STANDARD_FIELDS.to_owned()),
        ];
        let page: Page<Item> = self
            .get(&["Playlists", playlist_id, "Items"], &query)
            .await?;
        Ok(page.items)
    }

    pub async fn genres(&self, query: &ListQuery) -> Result<Page<Item>> {
        self.by_name(&["MusicGenres"], query).await
    }

    /// Items by id, in the order asked for.
    ///
    /// Asked for in chunks of [`MAX_IDS_PER_REQUEST`]. The server answers in its own
    /// order and leaves out ids it no longer has (a file deleted since a playlist was
    /// saved); the result is put back in the requested order and a missing id is
    /// simply absent, which is how a caller tells an entry has dangled. An id asked
    /// for twice appears twice.
    pub async fn items_by_ids<S: AsRef<str>>(&self, ids: &[S]) -> Result<Vec<Item>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let user_id = self.user_id()?.to_owned();

        let mut found: HashMap<String, Item> = HashMap::new();
        for chunk in ids.chunks(MAX_IDS_PER_REQUEST) {
            let joined = chunk
                .iter()
                .map(AsRef::as_ref)
                .collect::<Vec<_>>()
                .join(",");
            let query = [
                ("userId", user_id.clone()),
                ("ids", joined),
                ("fields", STANDARD_FIELDS.to_owned()),
            ];
            let page: Page<Item> = self.get(&["Items"], &query).await?;
            found.extend(page.items.into_iter().map(|item| (item.id.clone(), item)));
        }
        Ok(ids
            .iter()
            .filter_map(|id| found.get(id.as_ref()).cloned())
            .collect())
    }

    /// An album's tracks in disc, then track, order.
    pub async fn album_tracks(&self, album_id: &str) -> Result<Vec<Item>> {
        let query = ListQuery {
            parent_id: Some(album_id.to_owned()),
            sort: Sort::AlbumTrackOrder,
            ..ListQuery::default()
        };
        Ok(self.items("Audio", &query, None).await?.items)
    }

    /// Every album credited to an artist as album artist, newest first.
    pub async fn artist_albums(&self, artist_id: &str) -> Result<Vec<Item>> {
        let query = [
            ("userId", self.user_id()?.to_owned()),
            ("albumArtistIds", artist_id.to_owned()),
            ("includeItemTypes", "MusicAlbum".to_owned()),
            ("recursive", "true".to_owned()),
            ("sortBy", Sort::ReleaseDate.as_str().to_owned()),
            ("sortOrder", SortOrder::Descending.as_str().to_owned()),
            ("fields", STANDARD_FIELDS.to_owned()),
        ];
        let page: Page<Item> = self.get(&["Items"], &query).await?;
        Ok(page.items)
    }

    /// Songs, albums and artists whose names match `term`, up to `limit` of each.
    /// Scope it to a music library where possible: unscoped, songs also match
    /// audiobook chapters, which Jellyfin stores as audio too.
    ///
    /// Artists come from `/Artists/AlbumArtists`, not `/Items`: artists have no
    /// library parent, so `/Items` with a `parentId` never returns them, and album
    /// artists rather than every artist, so a name featured on one track doesn't
    /// lead to an empty artist page.
    pub async fn search(
        &self,
        term: &str,
        parent_id: Option<&str>,
        limit: u32,
    ) -> Result<SearchResults> {
        let term = term.trim();
        if term.is_empty() {
            return Ok(SearchResults::default());
        }
        let query = ListQuery {
            parent_id: parent_id.map(str::to_owned),
            search_term: Some(term.to_owned()),
            limit: Some(limit),
            ..ListQuery::default()
        };
        let (songs, albums, artists) = tokio::try_join!(
            self.items("Audio", &query, None),
            self.items("MusicAlbum", &query, None),
            self.by_name(&["Artists", "AlbumArtists"], &query),
        )?;
        Ok(SearchResults {
            songs: songs.items,
            albums: albums.items,
            artists: artists.items,
        })
    }

    /// Jellyfin's Instant Mix: a station seeded from a track, album, artist or genre.
    pub async fn instant_mix(&self, item_id: &str, limit: u32) -> Result<Vec<Item>> {
        let query = [
            ("userId", self.user_id()?.to_owned()),
            ("limit", limit.to_string()),
            ("fields", STANDARD_FIELDS.to_owned()),
        ];
        let page: Page<Item> = self.get(&["Items", item_id, "InstantMix"], &query).await?;
        Ok(page.items)
    }

    /// Marks or unmarks an item as a favourite; returns the user's updated state.
    pub async fn set_favorite(&self, item_id: &str, favorite: bool) -> Result<UserData> {
        let method = if favorite {
            Method::POST
        } else {
            Method::DELETE
        };
        let user_id = self.user_id()?;
        self.send_decoded(
            method,
            &["Users", user_id, "FavoriteItems", item_id],
            &[],
            None,
        )
        .await
    }

    /// Newly added music as albums, newest first.
    ///
    /// `/Items/Latest` ranks by the tracks, so an old album that just gained tracks
    /// resurfaces, which sorting albums by date added would miss. Pass the music
    /// library as `parent_id`: without one the server picks a library itself.
    pub async fn recently_added(&self, parent_id: Option<&str>, limit: u32) -> Result<Vec<Item>> {
        let mut query = vec![
            ("userId", self.user_id()?.to_owned()),
            ("includeItemTypes", "Audio".to_owned()),
            ("groupItems", "true".to_owned()),
            ("limit", limit.to_string()),
            ("fields", STANDARD_FIELDS.to_owned()),
        ];
        if let Some(parent_id) = parent_id {
            query.push(("parentId", parent_id.to_owned()));
        }
        // A bare array, not the paged envelope. A track with no album comes back as
        // itself; it would open as an album with no tracks, so it's dropped.
        let items: Vec<Item> = self.get(&["Items", "Latest"], &query).await?;
        Ok(items
            .into_iter()
            .filter(|item| item.kind == crate::ItemKind::MusicAlbum)
            .collect())
    }

    /// The tracks played most recently, newest first.
    ///
    /// Tracks, not albums: Jellyfin keeps no play statistics on albums at all.
    pub async fn recently_played(&self, parent_id: Option<&str>, limit: u32) -> Result<Vec<Item>> {
        self.played_tracks(parent_id, limit, Sort::DatePlayed).await
    }

    /// The tracks played most often, most first. Jellyfin counts a play when it
    /// starts, so skipping through an album counts every track on it.
    pub async fn most_played(&self, parent_id: Option<&str>, limit: u32) -> Result<Vec<Item>> {
        self.played_tracks(parent_id, limit, Sort::PlayCount).await
    }

    async fn played_tracks(
        &self,
        parent_id: Option<&str>,
        limit: u32,
        sort: Sort,
    ) -> Result<Vec<Item>> {
        let query = ListQuery {
            parent_id: parent_id.map(str::to_owned),
            sort,
            order: SortOrder::Descending,
            limit: Some(limit),
            ..ListQuery::default()
        };
        Ok(self.items("Audio", &query, Some("IsPlayed")).await?.items)
    }

    /// The paging, sorting and field set every list shares.
    fn list_query(&self, list: &ListQuery) -> Result<Query> {
        let mut query = vec![
            ("userId", self.user_id()?.to_owned()),
            ("sortBy", list.sort.as_str().to_owned()),
            ("sortOrder", list.order.as_str().to_owned()),
            ("startIndex", list.start_index.to_string()),
            ("fields", STANDARD_FIELDS.to_owned()),
        ];
        if let Some(parent_id) = &list.parent_id {
            query.push(("parentId", parent_id.clone()));
        }
        if let Some(term) = list.search_term.as_deref().map(str::trim)
            && !term.is_empty()
        {
            query.push(("searchTerm", term.to_owned()));
        }
        if !list.genre_ids.is_empty() {
            query.push(("genreIds", list.genre_ids.join(",")));
        }
        if list.favorites_only {
            query.push(("isFavorite", "true".to_owned()));
        }
        if let Some(limit) = list.limit {
            query.push(("limit", limit.to_string()));
        }
        Ok(query)
    }

    /// `/Items` for one or more item types.
    async fn items(
        &self,
        include_types: &str,
        list: &ListQuery,
        filters: Option<&str>,
    ) -> Result<Page<Item>> {
        let mut query = self.list_query(list)?;
        query.push(("includeItemTypes", include_types.to_owned()));
        query.push(("recursive", "true".to_owned()));
        if let Some(filters) = filters {
            query.push(("filters", filters.to_owned()));
        }
        self.get(&["Items"], &query).await
    }

    /// The by-name lists (artists, album artists, genres), which have their own
    /// endpoints rather than going through `/Items`.
    async fn by_name(&self, segments: &[&str], list: &ListQuery) -> Result<Page<Item>> {
        let query = self.list_query(list)?;
        self.get(segments, &query).await
    }
}
