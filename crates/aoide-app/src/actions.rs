//! What can be done to a song (or a set of them) from any list: the inline
//! action strip that opens under a row, the same actions for an album, an
//! artist or a playlist whose songs are fetched when asked, and the requests
//! they send.

use aoide_melete::{Item, ListQuery, MeleteClient};
use aoide_store::PlaySource;

use crate::playback::Enqueue;
use crate::playback::tracks::TrackMeta;
use crate::store_worker::StoreHandle;

/// Past this many playlists, Add to Playlist offers a search field.
pub const PLAYLIST_SEARCH_AFTER: usize = 8;

/// Play these, this way.
#[derive(Debug, Clone)]
pub struct PlayRequest {
    pub tracks: Vec<TrackMeta>,
    pub how: Enqueue,
    pub source: PlaySource,
}

impl PlayRequest {
    pub fn from(tracks: &[TrackMeta], start: usize, source: PlaySource) -> Self {
        Self {
            tracks: tracks.to_vec(),
            how: Enqueue::Replace {
                start,
                shuffle: false,
            },
            source,
        }
    }

    pub fn shuffled(tracks: &[TrackMeta], source: PlaySource) -> Self {
        Self {
            tracks: tracks.to_vec(),
            how: Enqueue::Replace {
                start: crate::playback::random_index(tracks.len()),
                shuffle: true,
            },
            source,
        }
    }
}

/// Songs that are not listed yet: what an album tile, an artist or a
/// playlist row stands for. Their actions fetch the songs first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Collection {
    Album(String),
    Artist(String),
    /// An Aoide playlist.
    Playlist(String),
}

impl Collection {
    pub fn source(&self) -> PlaySource {
        match self {
            Collection::Album(_) => PlaySource::Album,
            Collection::Artist(_) => PlaySource::Unknown,
            Collection::Playlist(_) => PlaySource::Playlist,
        }
    }
}

/// Why a collection's songs could not be had.
#[derive(Debug)]
pub enum Failure {
    Server(aoide_melete::Error),
    Store(String),
}

/// The songs of a collection, in the order they play: an album's in disc and
/// track order, an artist's album by album (their own albums' and the ones
/// crediting them elsewhere), a playlist's in its order without the entries
/// the server no longer has.
pub async fn collection_tracks(
    client: MeleteClient,
    store: StoreHandle,
    collection: Collection,
) -> Result<Vec<Item>, Failure> {
    match collection {
        Collection::Album(id) => client.album_tracks(&id).await.map_err(Failure::Server),
        Collection::Artist(id) => {
            let query = ListQuery {
                artist_ids: vec![id],
                ..ListQuery::default()
            };
            Ok(client.songs(&query).await.map_err(Failure::Server)?.items)
        }
        Collection::Playlist(id) => {
            let ids = store
                .call(move |w| {
                    w.store.items(&id).map(|items| {
                        items
                            .iter()
                            .filter(|item| !item.is_unresolved())
                            .map(|item| item.linked_jellyfin_id().to_owned())
                            .collect::<Vec<_>>()
                    })
                })
                .await
                .ok_or_else(|| Failure::Store("the library is closing".into()))?
                .map_err(|e| Failure::Store(e.to_string()))?;
            client.items_by_ids(&ids).await.map_err(Failure::Server)
        }
    }
}

/// The taste flags on one track.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flags {
    pub not_interested: bool,
    pub dont_count: bool,
}

/// The open action strip.
#[derive(Debug, Clone, PartialEq)]
pub struct Menu {
    /// Which row it belongs to: the same song can be listed twice.
    pub anchor: String,
    pub tracks: Vec<TrackMeta>,
    /// Showing the playlists to add to.
    pub adding: bool,
    pub filter: String,
    pub new_name: String,
    /// Read from the store when the strip opens, for a single track.
    pub flags: Option<Flags>,
    /// For an album, artist or playlist: what it is and its name. Its songs
    /// arrive after the menu opens.
    pub collection: Option<(Collection, String)>,
    pub loading: bool,
    pub error: Option<String>,
}

impl Menu {
    pub fn new(anchor: String, tracks: Vec<TrackMeta>) -> Self {
        Self {
            anchor,
            tracks,
            adding: false,
            filter: String::new(),
            new_name: String::new(),
            flags: None,
            collection: None,
            loading: false,
            error: None,
        }
    }

    /// The menu for a collection, its songs still to come.
    pub fn for_collection(anchor: String, collection: Collection, title: String) -> Self {
        Self {
            collection: Some((collection, title)),
            loading: true,
            ..Self::new(anchor, Vec::new())
        }
    }

    /// Where plays started from here come from.
    pub fn source(&self) -> PlaySource {
        self.collection
            .as_ref()
            .map_or(PlaySource::Unknown, |(c, _)| c.source())
    }

    /// The one track the flags and Go To apply to, if there is just one (and
    /// the menu is a song's, not a one-song album's).
    pub fn single(&self) -> Option<&TrackMeta> {
        if self.collection.is_some() {
            return None;
        }
        match self.tracks.as_slice() {
            [one] => Some(one),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum RowAction {
    Open {
        anchor: String,
        tracks: Vec<TrackMeta>,
    },
    /// The menu of an album tile, an artist or a playlist row.
    OpenCollection {
        anchor: String,
        collection: Collection,
        title: String,
    },
    /// A collection's songs arrived for the menu with this anchor.
    CollectionTracks(String, Result<Vec<TrackMeta>, String>),
    Close,
    Play,
    Shuffle,
    PlayNext,
    PlayLast,
    ShowPlaylists,
    Filter(String),
    AddTo(String),
    NewName(String),
    CreatePlaylist,
    NotInterested,
    DontCount,
    Favourite,
    FlagsLoaded(String, Flags),
    GoToAlbum,
    GoToArtist,
    /// A station from the menu's song, album, artist or playlist.
    StartRadio,
    /// The song's file, as the server knows it.
    GetInfo,
}

/// The playlists to offer, filtered by name once there are enough to need it.
pub fn matching<'a, T>(playlists: &'a [T], name: impl Fn(&T) -> &str, filter: &str) -> Vec<&'a T> {
    let filter = filter.trim().to_lowercase();
    playlists
        .iter()
        .filter(|p| filter.is_empty() || name(p).to_lowercase().contains(&filter))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_matches_names_ignoring_case() {
        let lists = ["Morning", "Late Night", "night drive"];
        let found = matching(&lists, |s| s, "NIGHT");
        assert_eq!(found, [&"Late Night", &"night drive"]);
        assert_eq!(matching(&lists, |s| s, "  ").len(), 3);
    }

    #[test]
    fn a_collections_menu_has_no_single_track_and_plays_from_its_kind() {
        let mut menu = Menu::for_collection(
            "tile:al1".into(),
            Collection::Album("al1".into()),
            "Low Tide Radio".into(),
        );
        assert!(menu.loading && menu.tracks.is_empty());
        menu.tracks = vec![crate::playback::tracks::TrackMeta::from_item(&Item {
            id: "t1".into(),
            name: "One".into(),
            ..Item::default()
        })];
        assert!(menu.single().is_none(), "a one-song album is not a song");
        assert_eq!(menu.source(), PlaySource::Album);
        let playlist =
            Menu::for_collection("p".into(), Collection::Playlist("p1".into()), "P".into());
        assert_eq!(playlist.source(), PlaySource::Playlist);
        assert_eq!(
            Menu::new("row".into(), Vec::new()).source(),
            PlaySource::Unknown
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_playlists_songs_come_from_the_store_in_its_order() {
        use aoide_melete::{Credentials, Device, ServerAddress};
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/music/tracks"))
            .and(query_param("id", "2,1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(crate::test_json::page(vec![
                    crate::test_json::track(1, "A"),
                    crate::test_json::track(2, "B"),
                ])),
            )
            .expect(1)
            .mount(&server)
            .await;
        let client = MeleteClient::new(
            ServerAddress::parse(&server.uri()).unwrap(),
            Device::new("Test", "device-1"),
        )
        .unwrap()
        .with_credentials(Credentials {
            user_id: "u1".into(),
            access_token: "tok".into(),
            username: "alice".into(),
        });
        let store = StoreHandle::in_memory("device-1").unwrap();
        let id = store
            .call(|w| {
                let list = w.store.create_playlist("Mix", None, None, 1).unwrap();
                let link = |id: &str| aoide_store::TrackLink::new(id.to_owned(), format!("k-{id}"));
                w.store
                    .add_tracks(&[link("2"), link("1")], &list.id, 2)
                    .unwrap();
                list.id
            })
            .await
            .unwrap();
        let tracks = collection_tracks(client, store, Collection::Playlist(id))
            .await
            .unwrap();
        let names: Vec<&str> = tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["B", "A"]);
    }

    #[test]
    fn a_shuffle_starts_somewhere_inside_the_list() {
        let request = PlayRequest::shuffled(&[], PlaySource::Shuffle);
        assert!(matches!(
            request.how,
            Enqueue::Replace {
                start: 0,
                shuffle: true
            }
        ));
    }
}
