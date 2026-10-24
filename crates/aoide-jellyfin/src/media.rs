use reqwest::Url;

use crate::client::JellyfinClient;
use crate::{Item, Result};

/// How much bandwidth a stream may use.
///
/// Anything but `Lossless` sets a ceiling, and a ceiling makes the server
/// transcode even a file the player could have played as it is, which brings back
/// the gap between tracks. Gapless playback and a bitrate cap don't mix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum StreamQuality {
    /// 128 kbps.
    Low,
    /// 320 kbps.
    Medium,
    /// 1.5 Mbps: most lossy files play untouched.
    High,
    /// No ceiling: the file on disk, untouched.
    #[default]
    Lossless,
}

impl StreamQuality {
    /// The `maxStreamingBitrate` this asks for, in bits per second.
    pub fn bitrate(self) -> Option<u32> {
        match self {
            StreamQuality::Low => Some(128_000),
            StreamQuality::Medium => Some(320_000),
            StreamQuality::High => Some(1_500_000),
            StreamQuality::Lossless => None,
        }
    }
}

/// Containers the player can take as they are, so the server hands the file over
/// untouched. `alac` is deliberately absent: ALAC is a codec inside m4a, not a
/// Jellyfin container, so listing it would match nothing.
///
/// Every one is a format aoide-audio decodes and tests. Nothing that can carry
/// Opus is offered (no `opus`, `ogg`, `webm`, `mka`): the decoder has none, so the
/// server transcodes those to the target below, which it does decode. That
/// costs Ogg Vorbis files a transcode too.
const DIRECT_CONTAINERS: &str = "mp3,aac,m4a,m4b,flac,wav,aiff";

impl JellyfinClient {
    /// A playable audio URL, via `/Audio/{id}/universal`, which lets the server choose
    /// between sending the file as it is and transcoding, from what we say we decode.
    ///
    /// The token travels as the `ApiKey` query parameter, so the URL works without
    /// headers. Not `api_key`: that spelling is legacy authorisation, off by default
    /// from Jellyfin 10.12, and a server with it off answers every stream with 401.
    pub fn stream_url(&self, item_id: &str, quality: StreamQuality) -> Result<Url> {
        let credentials = self.require_credentials()?;
        let mut query = vec![
            ("userId", credentials.user_id.clone()),
            ("deviceId", self.device().id.clone()),
            ("ApiKey", credentials.access_token.clone()),
            ("container", DIRECT_CONTAINERS.to_owned()),
            ("audioCodec", "aac".to_owned()),
            // Progressive HTTP, not HLS: HLS segments cost a gap at every track
            // boundary. ADTS AAC for a transcode, because over progressive HTTP
            // Jellyfin can't promise an mp4's index lands at the front.
            ("transcodingContainer", "aac".to_owned()),
            ("transcodingProtocol", "http".to_owned()),
        ];
        if let Some(bitrate) = quality.bitrate() {
            query.push(("maxStreamingBitrate", bitrate.to_string()));
        }
        Ok(self.url(&["Audio", item_id, "universal"], &query))
    }

    /// The original file, for a full download. Token as `ApiKey`, as for streams.
    pub fn download_url(&self, item_id: &str) -> Result<Url> {
        let credentials = self.require_credentials()?;
        Ok(self.url(
            &["Items", item_id, "Download"],
            &[("ApiKey", credentials.access_token.clone())],
        ))
    }

    /// An item's primary image, scaled to at most `max_width` pixels wide.
    ///
    /// Pass the image tag where there is one: it changes when the image does, so a
    /// new cover gets a new URL and every cache refreshes on its own.
    pub fn artwork_url(&self, item_id: &str, tag: Option<&str>, max_width: u32) -> Url {
        let mut query = vec![
            ("maxWidth", max_width.to_string()),
            ("quality", "90".to_owned()),
        ];
        if let Some(tag) = tag {
            query.push(("tag", tag.to_owned()));
        }
        self.url(&["Items", item_id, "Images", "Primary"], &query)
    }

    /// The artwork for an item, borrowing its album's cover when it has none of its
    /// own. `None` when there's no cover at all.
    pub fn artwork_url_for(&self, item: &Item, max_width: u32) -> Option<Url> {
        let (item_id, tag) = item.artwork_source()?;
        Some(self.artwork_url(item_id, Some(tag), max_width))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Credentials, Device, Error, ServerAddress};

    fn client(address: &str) -> JellyfinClient {
        JellyfinClient::new(
            ServerAddress::parse(address).unwrap(),
            Device::new("Test", "test-device"),
        )
        .unwrap()
    }

    fn signed_in(address: &str) -> JellyfinClient {
        client(address).with_credentials(Credentials {
            user_id: "user1".into(),
            access_token: "tok".into(),
            username: "alice".into(),
        })
    }

    fn pairs(url: &Url) -> Vec<(String, String)> {
        url.query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }

    fn value<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
        pairs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn the_stream_url_carries_the_token_as_apikey_and_never_api_key() {
        let url = signed_in("http://host:8096")
            .stream_url("track1", StreamQuality::Lossless)
            .unwrap();
        let pairs = pairs(&url);
        assert_eq!(url.path(), "/Audio/track1/universal");
        assert_eq!(value(&pairs, "ApiKey"), Some("tok"));
        assert!(
            !pairs.iter().any(|(k, _)| k.eq_ignore_ascii_case("api_key")),
            "{url}"
        );
        assert_eq!(value(&pairs, "userId"), Some("user1"));
        assert_eq!(value(&pairs, "deviceId"), Some("test-device"));
        assert_eq!(value(&pairs, "container"), Some(DIRECT_CONTAINERS));
        assert_eq!(value(&pairs, "transcodingProtocol"), Some("http"));
        assert_eq!(value(&pairs, "transcodingContainer"), Some("aac"));
    }

    #[test]
    fn opus_is_never_offered_so_the_server_transcodes_it_to_adts_aac() {
        for quality in [StreamQuality::Lossless, StreamQuality::Low] {
            let url = signed_in("http://host").stream_url("t", quality).unwrap();
            let pairs = pairs(&url);
            // A Jellyfin entry may also name codecs, as `container|codec`.
            let offered: Vec<String> = value(&pairs, "container")
                .unwrap()
                .split([',', '|'])
                .map(str::to_ascii_lowercase)
                .collect();
            for opus_carrier in ["opus", "ogg", "oga", "webm", "webma", "mka", "matroska"] {
                assert!(
                    !offered.iter().any(|c| c == opus_carrier),
                    "{opus_carrier} is offered: {offered:?}"
                );
            }
            assert_eq!(value(&pairs, "audioCodec"), Some("aac"));
            assert_eq!(value(&pairs, "transcodingContainer"), Some("aac"));
        }
    }

    #[test]
    fn lossless_sends_no_ceiling_so_the_server_plays_the_file_as_is() {
        let url = signed_in("http://host")
            .stream_url("t", StreamQuality::Lossless)
            .unwrap();
        assert_eq!(value(&pairs(&url), "maxStreamingBitrate"), None);
    }

    #[test]
    fn a_quality_ceiling_sets_max_streaming_bitrate() {
        let url = signed_in("http://host")
            .stream_url("t", StreamQuality::Medium)
            .unwrap();
        assert_eq!(value(&pairs(&url), "maxStreamingBitrate"), Some("320000"));
    }

    #[test]
    fn media_urls_keep_a_reverse_proxy_prefix() {
        let client = signed_in("https://example.com/jellyfin/");
        assert_eq!(
            client
                .stream_url("t", StreamQuality::Lossless)
                .unwrap()
                .path(),
            "/jellyfin/Audio/t/universal"
        );
        assert_eq!(
            client.download_url("t").unwrap().path(),
            "/jellyfin/Items/t/Download"
        );
        assert_eq!(
            client.artwork_url("t", None, 300).path(),
            "/jellyfin/Items/t/Images/Primary"
        );
    }

    #[test]
    fn the_download_url_carries_apikey_only() {
        let url = signed_in("http://host").download_url("song9").unwrap();
        assert_eq!(url.as_str(), "http://host/Items/song9/Download?ApiKey=tok");
    }

    #[test]
    fn media_urls_need_a_signed_in_client() {
        let client = client("http://host");
        assert_eq!(
            client.stream_url("t", StreamQuality::Lossless),
            Err(Error::NotSignedIn)
        );
        assert_eq!(client.download_url("t"), Err(Error::NotSignedIn));
    }

    #[test]
    fn artwork_urls_carry_size_and_tag() {
        let url = client("http://host").artwork_url("album1", Some("tag9"), 600);
        assert_eq!(
            url.as_str(),
            "http://host/Items/album1/Images/Primary?maxWidth=600&quality=90&tag=tag9"
        );
    }

    #[test]
    fn a_track_borrows_its_albums_artwork() {
        let item: Item =
            serde_json::from_str(r#"{"Id":"t","AlbumId":"album1","AlbumPrimaryImageTag":"alb"}"#)
                .unwrap();
        let url = client("http://host").artwork_url_for(&item, 300).unwrap();
        assert_eq!(url.path(), "/Items/album1/Images/Primary");
        assert_eq!(value(&pairs(&url), "tag"), Some("alb"));

        let bare: Item = serde_json::from_str(r#"{"Id":"t"}"#).unwrap();
        assert_eq!(client("http://host").artwork_url_for(&bare, 300), None);
    }
}
