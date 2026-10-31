//! The Melete HTTP client: sign-in, the music library, streams, lyrics and likes.
//!
//! Everything goes through a [`MeleteClient`]. Build one from a [`ServerAddress`]
//! (what the person typed, normalised) and a [`Device`], sign in with
//! [`MeleteClient::sign_in`] or Quick Connect, then attach the [`Credentials`]
//! with [`MeleteClient::with_credentials`]. Aoide's own routes (`/aoide/*`: sync,
//! Next, Discover, the measurements) are the aoide-sidecar crate's, on the same
//! server and token.
//!
//! The client needs a tokio runtime: reqwest's connection pool runs on it.
//!
//! What there is, by screen:
//!
//! - **Sign-in:** [`MeleteClient::server_info`] to check an address,
//!   [`MeleteClient::sign_in`], or [`MeleteClient::start_quick_connect`] and
//!   [`MeleteClient::poll_quick_connect`].
//! - **Lists:** [`ListQuery`] drives songs, albums and artists: paging, sorting,
//!   words, a genre and artists. [`MeleteClient::genres`] lists the genres.
//! - **Home:** [`MeleteClient::home`].
//! - **Likes:** [`MeleteClient::likes`], [`MeleteClient::set_liked`] and
//!   [`MeleteClient::set_album_liked`].
//! - **Listening together:** [`MeleteClient::start_listening`],
//!   [`MeleteClient::join_listening`] and the rest of [`ListenSession`]'s calls.
//! - **Requests:** [`MeleteClient::request_search`], [`MeleteClient::ask_for`],
//!   [`MeleteClient::music_requests`] and [`MeleteClient::withdraw_request`].
//! - **Get Info:** [`MeleteClient::item_details`].
//! - **Sharing:** [`MeleteClient::user_directory`] names whom to share with.
//! - **Playing:** [`MeleteClient::play`] for a stream link,
//!   [`MeleteClient::cover_url`] for covers.
//! - **Maintenance** (administrators): [`MeleteClient::refresh_library`].
//! - **Lyrics:** [`MeleteClient::lyrics`], the server's own. The aoide-lyrics
//!   crate looks on the internet when this has none.

#![warn(missing_docs)]
#![warn(clippy::missing_errors_doc)]

mod address;
mod auth;
mod client;
mod details;
mod error;
mod library;
mod listen;
mod lyrics;
mod maintenance;
mod media;
mod model;
mod requests;

pub use address::ServerAddress;
pub use auth::{Credentials, Device, QuickConnectCode, ServerInfo, UserName};
pub use client::MeleteClient;
pub use details::ItemDetails;
pub use error::Error;
pub use library::{Home, Likes, ListQuery, SearchResults, Sort};
pub use listen::{ListenMember, ListenSession, ListenState, Listening, is_stale};
pub use lyrics::{LyricLine, LyricWord, Lyrics};
pub use media::{Stream, StreamFormat};
pub use model::{COVER_TAG, Item, ItemKind, NameId, Page};
pub use requests::{Candidate, MusicRequest, Progress, RequestKind, RequestState};
/// The URL type every media URL is returned as.
pub use reqwest::Url;

/// Shorthand for this crate's results.
pub type Result<T, E = Error> = std::result::Result<T, E>;
