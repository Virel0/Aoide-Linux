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
//! - **Profile pictures:** [`MeleteClient::me`] for the signed-in user's, and
//!   [`MeleteClient::avatar_url`] for the address of anyone's. Their own is
//!   set with [`MeleteClient::set_avatar`] or from the server's gallery
//!   ([`MeleteClient::avatar_categories`], [`MeleteClient::gallery_pictures`],
//!   [`MeleteClient::set_gallery_avatar`]), and taken off with
//!   [`MeleteClient::remove_avatar`].
//! - **Account:** [`MeleteClient::change_password`]; two-factor sign-in with
//!   [`MeleteClient::two_factor`], [`MeleteClient::start_two_factor`],
//!   [`MeleteClient::confirm_two_factor`] and
//!   [`MeleteClient::turn_off_two_factor`]; for administrators,
//!   [`MeleteClient::users`] and [`MeleteClient::give_new_password`].
//! - **Playing:** [`MeleteClient::play`] for a stream link,
//!   [`MeleteClient::cover_url`] for covers at a [`CoverSize`], and
//!   [`MeleteClient::motion_cover`] for an album's looping video.
//! - **Maintenance** (administrators): [`MeleteClient::refresh_library`].
//! - **Lyrics:** [`MeleteClient::lyrics`], from a file beside the track, its
//!   tags, or LRCLIB through the server; [`MeleteClient::lyrics_with`] adds
//!   the server's translation and pronunciation of each line.
//! - **Music by description:** [`MeleteClient::music_filters`] and
//!   [`MeleteClient::music_mix`], through the server's AI;
//!   [`MeleteClient::ai_enabled`] says whether it's on.

#![warn(missing_docs)]
#![warn(clippy::missing_errors_doc)]

mod address;
mod ai;
mod auth;
mod client;
mod colours;
mod details;
mod error;
mod events;
mod gallery;
mod library;
mod listen;
mod lyrics;
mod maintenance;
mod media;
mod model;
mod motion;
mod password;
mod profile;
mod request_imports;
mod requests;
mod two_factor;

pub use address::ServerAddress;
pub use ai::{MusicFilters, MusicMix, MusicOrdering};
pub use auth::{Credentials, Device, QuickConnectCode, ServerInfo, UserName};
pub use client::MeleteClient;
pub use details::ItemDetails;
pub use error::Error;
pub use events::{Event, EventStream};
pub use gallery::{AvatarCategory, GalleryPage, GalleryPicture};
pub use library::{Home, Likes, ListQuery, SearchResults, Sort};
pub use listen::{ListenMember, ListenSession, ListenState, Listening, is_stale};
pub use lyrics::{LyricLine, LyricWord, Lyrics, LyricsAnswer, LyricsAsk};
pub use media::{CoverSize, Stream, StreamFormat};
pub use model::{COVER_TAG, Item, ItemKind, NameId, Page};
pub use motion::MOTION_COVER_MAX_BYTES;
pub use password::{NewPassword, PASSWORD_MAX_CHARS, PASSWORD_MIN_CHARS, User};
pub use profile::{AVATAR_MAX_BYTES, Me, PictureType};
pub use request_imports::{
    PlaylistImport, PlaylistImportSummary, PlaylistImportTrack, playlist_csv,
};
pub use requests::{
    Candidate, Copying, MusicRequest, Progress, RequestFeed, RequestKind, RequestState,
};
/// The URL type every media URL is returned as.
pub use reqwest::Url;
pub use two_factor::{
    ChangeKind, MadeFrom, TwoFactorChange, TwoFactorSetup, TwoFactorStatus, WRONG_CODE,
    WRONG_PASSWORD,
};

/// Shorthand for this crate's results.
pub type Result<T, E = Error> = std::result::Result<T, E>;
