//! Lyrics text and Japanese readings, for the lyrics panel. The lyrics
//! themselves come from Melete, which reads a file beside the track, its tags,
//! or LRCLIB; nothing here goes on the internet.
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

#[cfg(feature = "furigana")]
mod furigana;
mod lrc;
mod romaji;

pub use aoide_melete::{LyricLine, LyricWord, Lyrics};
#[cfg(feature = "furigana")]
pub use furigana::{JapaneseReader, ReaderError, Ruby};
pub use lrc::{parse_lrc, parse_plain};
pub use romaji::{is_japanese, kana_to_romaji};
