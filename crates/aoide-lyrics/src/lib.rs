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
//!
//! # Japanese
//!
//! Always there: [`is_japanese`] (the text has kana) and [`kana_to_romaji`],
//! modified Hepburn for kana, with no dictionary, so kanji pass through.
//!
//! With the `furigana` feature, `JapaneseReader` adds the readings of kanji
//! (`furigana`, as `Ruby` runs) and whole-line romaji (`romaji`) that reads
//! kanji and particles, through the [lindera](https://github.com/lindera/lindera)
//! analyser (MIT) with the IPADIC dictionary built in. Measured on macOS
//! (arm64, release build of a probe binary):
//!
//! - binary: 49.0 MB against 0.4 MB without it, so about 48.6 MB of dictionary;
//! - build: lindera-ipadic's build script downloads the dictionary source,
//!   `mecab-ipadic-2.7.0-20250920.tar.gz` (12.9 MB, MD5-checked) from
//!   lindera.dev, and compiles it (58 MB in the build directory); a clean
//!   release build of the probe took 29 s. Offline builds need the archive
//!   put in `LINDERA_BUILD_DICTIONARY_CACHE_DIR` first;
//! - 55 more crates (build-time ones included), all MIT, Apache-2.0, BSD or
//!   similar; `webpki-roots` (CDLA-Permissive-2.0) only in the build script's
//!   downloader, not in the binary;
//! - run time: the dictionary loads in about 1 ms; a line takes 1–2 ms.
//!
//! IPADIC's licence is NAIST's and ICOT's: free use, change and redistribution,
//! provided its no-warranty section stays with it (lindera ships it as
//! `NOTICE.txt`; a package enabling the feature must carry it too). UniDic
//! (BSD-3-Clause, via `lindera-unidic`) would be the other choice; it is larger.

#![warn(missing_docs)]
#![warn(clippy::missing_errors_doc)]

mod finder;
#[cfg(feature = "furigana")]
mod furigana;
mod lrc;
mod lrclib;
mod provider;
mod romaji;

pub use aoide_jellyfin::{LyricLine, LyricWord, Lyrics};
pub use finder::{AllFailed, DEFAULT_MAX_DURATION_GAP, DEFAULT_TIMEOUT, Found, LyricsFinder};
#[cfg(feature = "furigana")]
pub use furigana::{JapaneseReader, ReaderError, Ruby};
pub use lrc::{parse_lrc, parse_plain};
pub use lrclib::{LRCLIB_URL, Lrclib, USER_AGENT};
pub use provider::{BoxFuture, Candidate, LyricsProvider, ProviderError, TrackQuery};
pub use romaji::{is_japanese, kana_to_romaji};
