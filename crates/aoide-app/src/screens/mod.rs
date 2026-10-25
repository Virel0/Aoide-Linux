//! The screens. Each has its own state, messages, update and view; this
//! module routes between them.
//!
//! A screen's view draws app messages directly (so its rows can play, open
//! things and open the action strip); its own messages come back to it as
//! `Message::Page(key, PageMsg::…)`. The key is the page's place in the back
//! stack, so an answer that arrives after the page was left goes nowhere.

pub mod album;
pub mod albums;
pub mod artist;
pub mod artists;
pub mod collections;
pub mod discover;
pub mod equalizer;
pub mod favourites;
pub mod genre;
pub mod genres;
pub mod home;
pub mod import;
pub mod jellyfin_playlist;
pub mod jellyfin_playlists;
pub mod login;
pub mod mixes;
pub mod playlist;
pub mod playlists;
pub mod replay;
pub mod search;
pub mod settings;
pub mod share;
pub mod songs;
pub mod sync_status;

use std::collections::HashMap;
use std::future::Future;

use aoide_jellyfin::JellyfinClient;
use iced::{Element, Task};

use crate::actions::Menu;
use crate::app::Message;
use crate::artwork::{ArtKey, Artwork};
use crate::session;
use crate::store_worker::StoreHandle;

/// A sidebar entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Section {
    Home,
    Albums,
    Artists,
    Songs,
    Genres,
    Playlists,
    Search,
    Settings,
    /// One Aoide playlist, by id.
    Playlist(String),
    Mixes,
    Favourites,
    Replay,
    JellyfinPlaylists,
    /// A new saved filter.
    Collections,
    /// One saved filter, by id.
    Collection(String),
}

impl Section {
    pub const LIBRARY: [Section; 10] = [
        Section::Home,
        Section::Albums,
        Section::Artists,
        Section::Songs,
        Section::Genres,
        Section::Favourites,
        Section::Playlists,
        Section::Mixes,
        Section::Replay,
        Section::Search,
    ];

    pub fn title(&self) -> &'static str {
        match self {
            Section::Home => "Home",
            Section::Albums => "Albums",
            Section::Artists => "Artists",
            Section::Songs => "Songs",
            Section::Genres => "Genres",
            Section::Playlists => "Playlists",
            Section::Search => "Search",
            Section::Settings => "Settings",
            Section::Playlist(_) => "Playlist",
            Section::Mixes => "Mixes",
            Section::Favourites => "Favourites",
            Section::Replay => "Replay",
            Section::JellyfinPlaylists => "Jellyfin playlists",
            Section::Collections => "New collection",
            Section::Collection(_) => "Collection",
        }
    }
}

/// Somewhere a page can be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Album { id: String, title: String },
    Artist { id: String, name: String },
    Genre { id: String, name: String },
    Playlist { id: String },
    Equalizer,
    Discover,
    SyncStatus,
    JellyfinPlaylist { id: String, name: String },
    Share { id: String, name: String },
    Import,
}

/// An Aoide playlist as the sidebar and the pickers list it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistSummary {
    pub id: String,
    pub name: String,
    pub count: i64,
    /// Its songs are what its rules match.
    pub smart: bool,
    /// The folder it is filed in; `None` is the top level.
    pub folder_id: Option<String>,
}

/// What a screen's update and open can reach.
pub struct Ctx<'a> {
    pub client: &'a JellyfinClient,
    /// The music library to stay inside, when the server has one.
    pub library: Option<&'a str>,
    pub store: &'a StoreHandle,
    pub key: u64,
}

/// What a screen's view can read.
pub struct View<'a> {
    pub key: u64,
    pub artwork: &'a Artwork,
    /// The Jellyfin id of what is playing, to mark its row.
    pub playing: Option<&'a str>,
    pub menu: Option<&'a Menu>,
    pub playlists: &'a [PlaylistSummary],
    pub favourites: &'a HashMap<String, bool>,
    pub can_go_back: bool,
    /// An OpenRouter key is set: Search offers search by description.
    pub smart_search: bool,
}

/// Something being fetched.
#[derive(Debug, Clone, PartialEq)]
pub enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Load<T> {
    pub fn from(result: Result<T, String>) -> Self {
        match result {
            Ok(value) => Load::Ready(value),
            Err(error) => Load::Failed(error),
        }
    }

    pub fn ready(&self) -> Option<&T> {
        match self {
            Load::Ready(value) => Some(value),
            _ => None,
        }
    }
}

/// Runs a request for the page with `key`. A rejected token signs out rather
/// than showing an error on whichever screen happened to ask.
pub fn fetch<T: Send + 'static>(
    key: u64,
    request: impl Future<Output = Result<T, aoide_jellyfin::Error>> + Send + 'static,
    wrap: impl Fn(Result<T, String>) -> PageMsg + Send + 'static,
) -> Task<Message> {
    Task::perform(request, move |result| match result {
        Err(aoide_jellyfin::Error::Unauthorized) => Message::TokenRejected,
        result => Message::Page(key, wrap(result.map_err(|e| session::describe(&e)))),
    })
}

/// One open page.
#[derive(Debug)]
pub enum Page {
    Home(home::State),
    Albums(albums::State),
    Album(album::State),
    Artists(artists::State),
    Artist(artist::State),
    Songs(songs::State),
    Genres(genres::State),
    Genre(genre::State),
    Search(search::State),
    Playlists(playlists::State),
    Playlist(playlist::State),
    Settings(settings::State),
    Mixes(mixes::State),
    Equalizer(equalizer::State),
    Discover(discover::State),
    Favourites(favourites::State),
    Replay(replay::State),
    SyncStatus(sync_status::State),
    JellyfinPlaylists(jellyfin_playlists::State),
    JellyfinPlaylist(jellyfin_playlist::State),
    Share(share::State),
    Import(import::State),
    Collections(collections::State),
}

#[derive(Debug, Clone)]
pub enum PageMsg {
    Home(home::Message),
    Albums(albums::Message),
    Album(album::Message),
    Artists(artists::Message),
    Artist(artist::Message),
    Songs(songs::Message),
    Genres(genres::Message),
    Genre(genre::Message),
    Search(search::Message),
    Playlists(playlists::Message),
    Playlist(playlist::Message),
    Settings(settings::Message),
    Mixes(mixes::Message),
    Equalizer(equalizer::Message),
    Discover(discover::Message),
    Favourites(favourites::Message),
    Replay(replay::Message),
    SyncStatus(sync_status::Message),
    JellyfinPlaylists(jellyfin_playlists::Message),
    JellyfinPlaylist(jellyfin_playlist::Message),
    Share(share::Message),
    Import(import::Message),
    Collections(collections::Message),
}

impl Page {
    /// The root page of a sidebar section.
    pub fn section(section: &Section, ctx: &Ctx<'_>) -> (Self, Task<Message>) {
        match section {
            Section::Home => wrap(home::open(ctx), Page::Home),
            Section::Albums => wrap(albums::open(ctx), Page::Albums),
            Section::Artists => wrap(artists::open(ctx), Page::Artists),
            Section::Songs => wrap(songs::open(ctx), Page::Songs),
            Section::Genres => wrap(genres::open(ctx), Page::Genres),
            Section::Playlists => wrap(playlists::open(ctx), Page::Playlists),
            Section::Search => wrap(search::open(), Page::Search),
            Section::Settings => wrap(settings::open(ctx), Page::Settings),
            Section::Playlist(id) => wrap(playlist::open(ctx, id), Page::Playlist),
            Section::Mixes => wrap(mixes::open(), Page::Mixes),
            Section::Favourites => wrap(favourites::open(ctx), Page::Favourites),
            Section::Replay => wrap(replay::open(ctx), Page::Replay),
            Section::JellyfinPlaylists => {
                wrap(jellyfin_playlists::open(ctx), Page::JellyfinPlaylists)
            }
            Section::Collections => wrap(collections::open(ctx, None), Page::Collections),
            Section::Collection(id) => wrap(collections::open(ctx, Some(id)), Page::Collections),
        }
    }

    pub fn route(route: &Route, ctx: &Ctx<'_>) -> (Self, Task<Message>) {
        match route {
            Route::Album { id, title } => wrap(album::open(ctx, id, title), Page::Album),
            Route::Artist { id, name } => wrap(artist::open(ctx, id, name), Page::Artist),
            Route::Genre { id, name } => wrap(genre::open(ctx, id, name), Page::Genre),
            Route::Playlist { id } => wrap(playlist::open(ctx, id), Page::Playlist),
            Route::Equalizer => (Page::Equalizer(equalizer::State::default()), Task::none()),
            Route::Discover => wrap(discover::open(ctx), Page::Discover),
            Route::SyncStatus => wrap(sync_status::open(ctx), Page::SyncStatus),
            Route::Share { id, name } => wrap(share::open(ctx, id, name), Page::Share),
            Route::Import => wrap(import::open(ctx), Page::Import),
            Route::JellyfinPlaylist { id, name } => wrap(
                jellyfin_playlist::open(ctx, id, name),
                Page::JellyfinPlaylist,
            ),
        }
    }

    pub fn update(&mut self, message: PageMsg, ctx: &Ctx<'_>) -> Task<Message> {
        match (self, message) {
            (Page::Home(state), PageMsg::Home(m)) => state.update(m),
            (Page::Albums(state), PageMsg::Albums(m)) => state.update(m, ctx),
            (Page::Album(state), PageMsg::Album(m)) => state.update(m, ctx),
            (Page::Artists(state), PageMsg::Artists(m)) => state.update(m, ctx),
            (Page::Artist(state), PageMsg::Artist(m)) => state.update(m, ctx),
            (Page::Songs(state), PageMsg::Songs(m)) => state.update(m, ctx),
            (Page::Genres(state), PageMsg::Genres(m)) => state.update(m),
            (Page::Genre(state), PageMsg::Genre(m)) => state.update(m),
            (Page::Search(state), PageMsg::Search(m)) => state.update(m, ctx),
            (Page::Playlists(state), PageMsg::Playlists(m)) => state.update(m, ctx),
            (Page::Playlist(state), PageMsg::Playlist(m)) => state.update(m, ctx),
            (Page::Settings(state), PageMsg::Settings(m)) => state.update(m),
            (Page::Mixes(state), PageMsg::Mixes(m)) => state.update(m, ctx.key),
            (Page::Discover(state), PageMsg::Discover(m)) => state.update(m),
            (Page::Favourites(state), PageMsg::Favourites(m)) => state.update(m),
            (Page::Replay(state), PageMsg::Replay(m)) => state.update(m, ctx),
            (Page::SyncStatus(state), PageMsg::SyncStatus(m)) => state.update(m, ctx),
            (Page::JellyfinPlaylists(state), PageMsg::JellyfinPlaylists(m)) => state.update(m, ctx),
            (Page::JellyfinPlaylist(state), PageMsg::JellyfinPlaylist(m)) => state.update(m, ctx),
            (Page::Share(state), PageMsg::Share(m)) => state.update(m, ctx),
            (Page::Import(state), PageMsg::Import(m)) => state.update(m, ctx),
            (Page::Collections(state), PageMsg::Collections(m)) => state.update(m, ctx),
            _ => Task::none(),
        }
    }

    /// Every page but Settings and the equaliser, which the app draws: it
    /// holds the settings.
    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, Message> {
        match self {
            Page::Home(state) => state.view(view),
            Page::Albums(state) => state.view(view),
            Page::Album(state) => state.view(view),
            Page::Artists(state) => state.view(view),
            Page::Artist(state) => state.view(view),
            Page::Songs(state) => state.view(view),
            Page::Genres(state) => state.view(view),
            Page::Genre(state) => state.view(view),
            Page::Search(state) => state.view(view),
            Page::Playlists(state) => state.view(view),
            Page::Playlist(state) => state.view(view),
            Page::Settings(_) => iced::widget::space().into(),
            Page::Mixes(state) => state.view(view),
            Page::Equalizer(_) => iced::widget::space().into(),
            Page::Discover(state) => state.view(view),
            Page::Favourites(state) => state.view(view),
            Page::Replay(state) => state.view(view),
            Page::SyncStatus(state) => state.view(view),
            Page::JellyfinPlaylists(state) => state.view(view),
            Page::JellyfinPlaylist(state) => state.view(view),
            Page::Share(state) => state.view(view),
            Page::Import(state) => state.view(view),
            Page::Collections(state) => state.view(view),
        }
    }

    /// The covers this page shows, to fetch.
    pub fn artwork(&self) -> Vec<ArtKey> {
        fn tiles(tiles: &[crate::library::Tile]) -> Vec<ArtKey> {
            tiles.iter().filter_map(|t| t.art.clone()).collect()
        }
        match self {
            Page::Home(state) => state.artwork(),
            Page::Albums(state) => tiles(&state.tiles),
            Page::Album(state) => state.art.clone().into_iter().collect(),
            Page::Artists(state) => tiles(&state.tiles),
            Page::Artist(state) => state.artwork(),
            Page::Songs(state) => track_art(&state.tracks),
            Page::Genre(state) => state.albums.ready().map(|t| tiles(t)).unwrap_or_default(),
            Page::Search(state) => state.artwork(),
            Page::Playlist(state) => state.artwork(),
            Page::Genres(_) | Page::Settings(_) => Vec::new(),
            Page::Discover(state) => state.artwork(),
            Page::Favourites(state) => state.artwork(),
            Page::Replay(state) => state.artwork(),
            Page::SyncStatus(_) | Page::JellyfinPlaylists(_) | Page::Share(_) | Page::Import(_) => {
                Vec::new()
            }
            Page::Playlists(state) => state.artwork(),
            Page::Collections(state) => state.artwork(),
            Page::JellyfinPlaylist(state) => state.artwork(),
            Page::Mixes(state) => state.artwork(),
            Page::Equalizer(_) => Vec::new(),
        }
    }
}

pub fn track_art(tracks: &[crate::playback::tracks::TrackMeta]) -> Vec<ArtKey> {
    tracks
        .iter()
        .filter_map(|t| t.artwork.as_ref().map(|(i, g)| ArtKey::new(i, g)))
        .collect()
}

fn wrap<S>(
    (state, task): (S, Task<Message>),
    page: impl FnOnce(S) -> Page,
) -> (Page, Task<Message>) {
    (page(state), task)
}
