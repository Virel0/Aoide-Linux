//! The Jellyfin HTTP client: sign-in, the library, streams, lyrics, and playback reporting.
//!
//! Everything goes through a [`JellyfinClient`]. Build one from a [`ServerAddress`]
//! (what the person typed, normalised) and a [`Device`], sign in with
//! [`JellyfinClient::authenticate_by_name`], then attach the [`Credentials`] with
//! [`JellyfinClient::with_credentials`].
//!
//! The client needs a tokio runtime: reqwest's connection pool runs on it.
//!
//! What there is, by screen:
//!
//! - **Sign-in:** [`JellyfinClient::public_system_info`] to check an address,
//!   [`discover_servers`] to find servers on the LAN without typing one.
//! - **Lists:** [`ListQuery`] drives songs, albums, artists, genres and playlists:
//!   paging, sorting, a library, and filters (`genre_ids`, `years`, `tags`,
//!   `favorites_only` for the Favourites screen). [`JellyfinClient::query_filters`]
//!   lists the values a filter screen offers.
//! - **Folders:** [`JellyfinClient::folder_children`], reading only.
//! - **Get Info:** [`JellyfinClient::item_details`].
//! - **Radio:** [`JellyfinClient::instant_mix_from`] with a [`MixSeed`], and
//!   `similar_items`, `similar_albums`, `similar_artists`.
//! - **Jellyfin playlists:** `create_playlist`, `edit_playlist`, `delete_playlist`,
//!   [`JellyfinClient::playlist_entries`], `add_to_playlist`,
//!   `remove_from_playlist`, `move_playlist_entry`, `set_playlist_cover`,
//!   `delete_playlist_cover`.
//! - **Maintenance** (administrators): [`JellyfinClient::refresh_library`],
//!   [`JellyfinClient::refresh_item`].
//! - **Lyrics:** [`JellyfinClient::lyrics`], the server's own. The aoide-lyrics
//!   crate looks on the internet when this has none.

#![warn(missing_docs)]
#![warn(clippy::missing_errors_doc)]

mod address;
mod auth;
mod client;
mod details;
mod discovery;
mod error;
mod filters;
mod folders;
mod library;
mod lyrics;
mod maintenance;
mod media;
mod model;
mod playback;
mod playlists;
mod radio;
pub mod ticks;

pub use address::ServerAddress;
pub use auth::{Credentials, Device, PublicSystemInfo};
pub use client::JellyfinClient;
pub use details::ItemDetails;
pub use discovery::{
    DISCOVERY_PORT, DISCOVERY_PROBE, DiscoveredServer, discover_servers, discover_servers_at,
};
pub use error::Error;
pub use filters::QueryFilters;
pub use library::{ListQuery, SearchResults, Sort, SortOrder};
pub use lyrics::{LyricLine, LyricWord, Lyrics};
pub use maintenance::ItemRefresh;
pub use media::StreamQuality;
pub use model::{Item, ItemKind, NameId, Page, UserData};
pub use playlists::{PlaylistEdit, PlaylistEntry};
pub use radio::MixSeed;
/// The URL type every media URL is returned as.
pub use reqwest::Url;

/// Shorthand for this crate's results.
pub type Result<T, E = Error> = std::result::Result<T, E>;
