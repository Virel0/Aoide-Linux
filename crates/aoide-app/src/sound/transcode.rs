//! Transcoding: the server converting each stream to a smaller one, at a
//! bitrate and in a format the decoder plays.

use serde::{Deserialize, Serialize};

/// Transcoding: off by default, since a capped stream loses the gapless
/// joins (the server transcodes even files that would have played as they
/// are).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Transcode {
    pub enabled: bool,
    pub bitrate_kbps: u32,
    pub format: TranscodeFormat,
}

impl Default for Transcode {
    fn default() -> Self {
        Self {
            enabled: false,
            bitrate_kbps: 256,
            format: TranscodeFormat::Aac,
        }
    }
}

/// What the server transcodes to: formats the decoder plays, over
/// progressive HTTP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TranscodeFormat {
    /// ADTS AAC, as the stream URL already asks for a transcode.
    #[default]
    Aac,
    Mp3,
}

impl TranscodeFormat {
    pub const ALL: [TranscodeFormat; 2] = [TranscodeFormat::Aac, TranscodeFormat::Mp3];

    /// Jellyfin's name for it, as codec and as container.
    pub fn jellyfin(self) -> &'static str {
        match self {
            TranscodeFormat::Aac => "aac",
            TranscodeFormat::Mp3 => "mp3",
        }
    }
}

impl std::fmt::Display for TranscodeFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            TranscodeFormat::Aac => "AAC",
            TranscodeFormat::Mp3 => "MP3",
        })
    }
}

impl Transcode {
    /// The bitrates offered, in kbps.
    pub const BITRATES: [u32; 4] = [128, 192, 256, 320];

    /// A bitrate the menu offers: the nearest one.
    pub fn sanitised(mut self) -> Self {
        self.bitrate_kbps = *Self::BITRATES
            .iter()
            .min_by_key(|b| b.abs_diff(self.bitrate_kbps))
            .unwrap_or(&256);
        self
    }
}

/// `url` (from `JellyfinClient::stream_url`, which the player's tracks ask
/// for as `StreamQuality::Lossless`) with the transcoding settings:
/// untouched when off; else the exact ceiling and bitrate, and the format
/// as the codec and container the server transcodes to. Every other
/// parameter (the token, the containers played as they are) stays.
pub fn transcoded_url(url: &aoide_jellyfin::Url, transcode: &Transcode) -> aoide_jellyfin::Url {
    if !transcode.enabled {
        return url.clone();
    }
    let bits = (transcode.bitrate_kbps * 1_000).to_string();
    let format = transcode.format.jellyfin();
    let replaced = |key: &str| -> Option<&str> {
        match key {
            "maxStreamingBitrate" | "audioBitRate" => Some(bits.as_str()),
            "audioCodec" | "transcodingContainer" => Some(format),
            _ => None,
        }
    };
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| {
            let v = replaced(&k).map_or_else(|| v.into_owned(), str::to_owned);
            (k.into_owned(), v)
        })
        .collect();
    for key in ["maxStreamingBitrate", "audioBitRate"] {
        if !pairs.iter().any(|(k, _)| k == key) {
            pairs.push((key.to_owned(), bits.clone()));
        }
    }
    let mut out = url.clone();
    out.query_pairs_mut().clear().extend_pairs(pairs);
    out
}

/// A built track streamed as the settings say.
pub fn transcoded(
    mut built: crate::playback::tracks::BuiltTrack,
    transcode: &Transcode,
) -> crate::playback::tracks::BuiltTrack {
    if transcode.enabled {
        built.url = transcoded_url(&built.url, transcode);
        built.track.source = aoide_audio::MediaSource::http(built.url.as_str());
    }
    built
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream_url() -> aoide_jellyfin::Url {
        let client = aoide_jellyfin::JellyfinClient::new(
            aoide_jellyfin::ServerAddress::parse("https://music.example.net").unwrap(),
            aoide_jellyfin::Device::new("Test", "device-1"),
        )
        .unwrap()
        .with_credentials(aoide_jellyfin::Credentials {
            user_id: "u1".into(),
            access_token: "tok".into(),
            username: "alice".into(),
        });
        client
            .stream_url("t1", aoide_jellyfin::StreamQuality::Lossless)
            .unwrap()
    }

    fn query(url: &aoide_jellyfin::Url) -> Vec<(String, String)> {
        url.query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }

    #[test]
    fn transcoding_off_leaves_the_stream_untouched() {
        let url = stream_url();
        assert_eq!(transcoded_url(&url, &Transcode::default()), url);
        assert!(!query(&url).iter().any(|(k, _)| k == "maxStreamingBitrate"));
    }

    #[test]
    fn transcoding_sets_the_bitrate_and_format_and_keeps_the_rest() {
        let url = stream_url();
        let transcode = Transcode {
            enabled: true,
            bitrate_kbps: 192,
            format: TranscodeFormat::Mp3,
        };
        let out = transcoded_url(&url, &transcode);
        let pairs = query(&out);
        let get = |k: &str| {
            pairs
                .iter()
                .filter(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(get("maxStreamingBitrate"), ["192000"]);
        assert_eq!(get("audioBitRate"), ["192000"]);
        assert_eq!(get("audioCodec"), ["mp3"]);
        assert_eq!(get("transcodingContainer"), ["mp3"]);
        assert_eq!(get("ApiKey"), ["tok"]);
        assert_eq!(get("transcodingProtocol"), ["http"]);
        assert_eq!(
            get("container"),
            query(&url)
                .iter()
                .filter(|(k, _)| k == "container")
                .map(|(_, v)| v.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(out.path(), "/Audio/t1/universal");
    }

    #[test]
    fn an_odd_bitrate_in_the_file_becomes_the_nearest_offered() {
        let t = Transcode {
            enabled: true,
            bitrate_kbps: 300,
            format: TranscodeFormat::Aac,
        };
        assert_eq!(t.sanitised().bitrate_kbps, 320);
        let t = Transcode {
            bitrate_kbps: 0,
            ..t
        };
        assert_eq!(t.sanitised().bitrate_kbps, 128);
    }
}
