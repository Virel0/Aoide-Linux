//! Lyrics from the internet, for tracks the Jellyfin server has none for.
//!
//! The lyrics come out as aoide-jellyfin's [`Lyrics`] (lines, optional line
//! times, optional word times), the same shape the server's own lyrics have, so
//! the lyrics panel needn't know where they came from.
//!
//! ```no_run
//! # async fn example(client: aoide_jellyfin::JellyfinClient, item: aoide_jellyfin::Item)
//! # -> Result<(), Box<dyn std::error::Error>> {
//! use aoide_lyrics::{LyricsFinder, TrackQuery};
//!
//! let finder = LyricsFinder::with_default_providers()?; // sends nothing
//! let lyrics = match client.lyrics(&item.id).await? {
//!     Some(server) => Some(server),
//!     // Only when the person has internet lyrics on: that switch is the app's.
//!     None => finder
//!         .find(&TrackQuery::from_item(&item))
//!         .await?
//!         .map(|found| found.lyrics),
//! };
//! # Ok(()) }
//! ```
//!
//! # Providers
//!
//! Each sits behind [`LyricsProvider`]; [`LyricsFinder`] asks them all at once,
//! each within a deadline, and ranks what comes back. Only documented public
//! APIs whose terms allow an app to use them are included, and no provider
//! scrapes web pages.
//!
//! - **LRCLIB** ([`Lrclib`]): a documented, keyless API whose terms permit use in
//!   applications, commercial ones included, for clients that identify
//!   themselves and honour `Retry-After`. The only provider.
//!
//! Left out, from the providers the Electron app had:
//!
//! - **NetEase Cloud Music** has no public API documentation; what clients use
//!   is reverse-engineered from its own apps.
//! - **Genius**' documented API (`api.genius.com`) needs a registered app's
//!   token and returns song metadata and annotations, not lyrics: the lyrics
//!   are only on its web pages, which would mean scraping.
//! - **SimpMusic Lyrics** has a generated OpenAPI description but no terms of
//!   use, labels itself educational and non-commercial, says part of its data
//!   is crawled from other services, and keys lyrics by YouTube video id, which
//!   a Jellyfin track doesn't have.
//!
//! # LRC
//!
//! [`parse_lrc`] and [`parse_plain`] read lyrics text into [`Lyrics`], for any
//! source of `.lrc` or plain text.

mod finder;
mod lrc;
mod lrclib;
mod provider;

pub use aoide_jellyfin::{LyricLine, LyricWord, Lyrics};
pub use finder::{AllFailed, DEFAULT_MAX_DURATION_GAP, DEFAULT_TIMEOUT, Found, LyricsFinder};
pub use lrc::{parse_lrc, parse_plain};
pub use lrclib::{LRCLIB_URL, Lrclib, USER_AGENT};
pub use provider::{BoxFuture, Candidate, LyricsProvider, ProviderError, TrackQuery};
