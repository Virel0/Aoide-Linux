//! The Jellyfin HTTP client: sign-in, the library, streams, lyrics, and playback reporting.
//!
//! Everything goes through a [`JellyfinClient`]. Build one from a [`ServerAddress`]
//! (what the person typed, normalised) and a [`Device`], sign in with
//! [`JellyfinClient::authenticate_by_name`], then attach the [`Credentials`] with
//! [`JellyfinClient::with_credentials`].
//!
//! The client needs a tokio runtime: reqwest's connection pool runs on it.

mod address;
mod auth;
mod client;
mod error;
mod library;
mod lyrics;
mod media;
mod model;
mod playback;
pub mod ticks;

pub use address::ServerAddress;
pub use auth::{Credentials, Device, PublicSystemInfo};
pub use client::JellyfinClient;
pub use error::Error;
pub use library::{ListQuery, SearchResults, Sort, SortOrder};
pub use lyrics::{LyricLine, LyricWord, Lyrics};
pub use media::StreamQuality;
pub use model::{Item, ItemKind, NameId, Page, UserData};
/// The URL type every media URL is returned as.
pub use reqwest::Url;

/// Shorthand for this crate's results.
pub type Result<T, E = Error> = std::result::Result<T, E>;
