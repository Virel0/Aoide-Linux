use reqwest::{Method, Url};
use serde::Deserialize;
use serde_json::json;

use crate::client::MeleteClient;
use crate::{Error, Result};

/// The widths Melete makes album covers at: a JPEG made once from the cover
/// and kept, never larger than the cover itself. No other width is asked for,
/// as the server refuses one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CoverSize {
    /// 160 pixels wide.
    W160,
    /// 400 pixels wide.
    W400,
    /// 600 pixels wide.
    W600,
    /// 800 pixels wide.
    W800,
}

impl CoverSize {
    /// The width in pixels.
    pub fn pixels(self) -> u16 {
        match self {
            Self::W160 => 160,
            Self::W400 => 400,
            Self::W600 => 600,
            Self::W800 => 800,
        }
    }
}

/// What Melete sends a track as.
///
/// `Original` is the file untouched, which keeps gapless joins exact; a
/// conversion is made whole on the server before the link comes back (a second
/// or two), then kept two days. Opus isn't offered: the player has no decoder
/// for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum StreamFormat {
    /// The file as it is.
    #[default]
    Original,
    /// FLAC, lossless.
    Flac,
    /// AAC at this many kbps (64 to 320).
    Aac(u32),
    /// MP3 at this many kbps (64 to 320).
    Mp3(u32),
}

impl StreamFormat {
    /// The `/play` request body. Never `spatial`: the player folds nothing, so
    /// Atmos and multichannel tracks come folded into stereo.
    fn body(self) -> serde_json::Value {
        match self {
            StreamFormat::Original => json!({ "format": "original" }),
            StreamFormat::Flac => json!({ "format": "flac" }),
            StreamFormat::Aac(kbps) => {
                json!({ "format": "aac", "bitrate_kbps": kbps.clamp(64, 320) })
            }
            StreamFormat::Mp3(kbps) => {
                json!({ "format": "mp3", "bitrate_kbps": kbps.clamp(64, 320) })
            }
        }
    }
}

/// A link to play one track by. It carries its own token, works for 24 hours and
/// for this track only, serves byte ranges, and stops working when the session
/// that made it signs out. `Debug` leaves the URL out.
#[derive(Clone, PartialEq, Eq)]
pub struct Stream {
    /// Hand this to the player as it is.
    pub url: Url,
    /// When the link stops working, in Unix seconds.
    pub expires_at: i64,
    /// What's served: `audio/flac`, `audio/mp4`, `audio/mpeg`, …
    pub content_type: String,
}

impl std::fmt::Debug for Stream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stream")
            .field("url", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .field("content_type", &self.content_type)
            .finish()
    }
}

#[derive(Deserialize)]
struct WireStream {
    stream_url: String,
    expires_at: i64,
    content_type: String,
}

impl MeleteClient {
    /// A link to stream a track by, as `format` asks.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] for a track the server doesn't have; HTTP 503 as
    /// [`Error::Http`] when a conversion can't be made now; otherwise the
    /// [request errors](MeleteClient#errors). A link that isn't a URL under the
    /// server is [`Error::Decoding`].
    pub async fn play(&self, track_id: &str, format: StreamFormat) -> Result<Stream> {
        let path = ["api", "v1", "music", "tracks", track_id, "play"];
        let wire: WireStream = self
            .send_decoded(Method::POST, &path, &[], Some(&format.body()))
            .await?;
        // A path under the server's own address, as Melete gives it; joined
        // here so a reverse proxy's prefix stays.
        let url = self
            .address()
            .join_path(&wire.stream_url)
            .ok_or_else(|| Error::Decoding {
                path: path.join("/"),
                message: "the stream link isn't a path on this server".to_owned(),
            })?;
        Ok(Stream {
            url,
            expires_at: wire.expires_at,
            content_type: wire.content_type,
        })
    }

    /// An album's cover, at most `size` wide, or the whole cover for `None`.
    /// Fetching it needs the token as `Authorization: Bearer`, which
    /// [`authorization`](Self::authorization) gives. Clients may keep a cover
    /// a day. A server before sized covers ignores the size and sends the
    /// whole cover.
    pub fn cover_url(&self, album_id: &str, size: Option<CoverSize>) -> Url {
        let query: Vec<(&str, String)> = size
            .map(|size| ("size", size.pixels().to_string()))
            .into_iter()
            .collect();
        self.url(&["api", "v1", "music", "albums", album_id, "cover"], &query)
    }

    /// The `Authorization` header value for a request made outside this client,
    /// such as a cover download; `None` when signed out.
    pub fn authorization(&self) -> Option<String> {
        self.credentials()
            .map(|credentials| format!("Bearer {}", credentials.access_token))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_original_file_asks_for_no_bitrate() {
        assert_eq!(
            StreamFormat::Original.body(),
            json!({ "format": "original" })
        );
    }

    #[test]
    fn lossy_formats_carry_a_bitrate_held_to_meletes_range() {
        assert_eq!(
            StreamFormat::Aac(256).body(),
            json!({ "format": "aac", "bitrate_kbps": 256 })
        );
        assert_eq!(
            StreamFormat::Mp3(1000).body(),
            json!({ "format": "mp3", "bitrate_kbps": 320 })
        );
        assert_eq!(
            StreamFormat::Aac(8).body(),
            json!({ "format": "aac", "bitrate_kbps": 64 })
        );
    }

    #[test]
    fn a_cover_is_asked_for_at_its_size_or_whole() {
        let client = MeleteClient::new(
            crate::ServerAddress::parse("https://music.example/melete").unwrap(),
            crate::Device::new("Test", "test-device"),
        )
        .unwrap();
        assert_eq!(
            client.cover_url("7", Some(CoverSize::W160)).as_str(),
            "https://music.example/melete/api/v1/music/albums/7/cover?size=160"
        );
        assert_eq!(
            client.cover_url("7", Some(CoverSize::W800)).as_str(),
            "https://music.example/melete/api/v1/music/albums/7/cover?size=800"
        );
        assert_eq!(
            client.cover_url("7", None).as_str(),
            "https://music.example/melete/api/v1/music/albums/7/cover"
        );
    }

    #[test]
    fn debug_never_prints_the_stream_link() {
        let stream = Stream {
            url: Url::parse("https://host/api/v1/music/stream/secret").unwrap(),
            expires_at: 1,
            content_type: "audio/flac".into(),
        };
        assert!(!format!("{stream:?}").contains("secret"));
    }
}
