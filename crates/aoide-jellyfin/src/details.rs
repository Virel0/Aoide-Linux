//! Everything the server knows about one item's file, for a Get Info screen.

use std::time::SystemTime;

use serde::{Deserialize, Deserializer};

use crate::client::JellyfinClient;
use crate::model::{nullable, parse_date};
use crate::{Item, Result};

/// One item with its file's technical details. Any of them may be unknown: a
/// file the server hasn't probed yet has none.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemDetails {
    pub item: Item,
    /// Where the file is, on the server's disk.
    pub path: Option<String>,
    /// `flac`, `mp3`, `m4a`, …
    pub container: Option<String>,
    /// The audio codec: `flac`, `alac`, `aac`, `mp3`, …
    pub codec: Option<String>,
    /// Bits per second.
    pub bitrate: Option<u32>,
    /// Hertz.
    pub sample_rate: Option<u32>,
    /// Bits per sample. Lossy codecs have none.
    pub bit_depth: Option<u32>,
    pub channels: Option<u32>,
    /// `stereo`, `mono`, `5.1`, …
    pub channel_layout: Option<String>,
    /// Bytes.
    pub size: Option<u64>,
    /// When the server first saw the file.
    pub date_added: Option<SystemTime>,
    /// The signed-in user's plays, counted at each start.
    pub play_count: u32,
    pub last_played: Option<SystemTime>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WireDetails {
    #[serde(flatten)]
    item: Item,
    path: Option<String>,
    container: Option<String>,
    #[serde(default, deserialize_with = "date")]
    date_created: Option<SystemTime>,
    #[serde(default, deserialize_with = "nullable")]
    media_sources: Vec<WireSource>,
    #[serde(default, deserialize_with = "nullable")]
    media_streams: Vec<WireStream>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WireSource {
    path: Option<String>,
    container: Option<String>,
    size: Option<i64>,
    bitrate: Option<i64>,
    #[serde(default, deserialize_with = "nullable")]
    media_streams: Vec<WireStream>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WireStream {
    #[serde(rename = "Type")]
    kind: Option<String>,
    codec: Option<String>,
    bit_rate: Option<i64>,
    sample_rate: Option<i64>,
    bit_depth: Option<i64>,
    channels: Option<i64>,
    channel_layout: Option<String>,
}

fn date<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<SystemTime>, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.and_then(|raw| parse_date(&raw)))
}

/// A count or rate from the server, which sends them as signed numbers; a
/// negative or zero one means unknown.
fn positive<T: TryFrom<i64>>(value: Option<i64>) -> Option<T> {
    value.filter(|v| *v > 0).and_then(|v| T::try_from(v).ok())
}

impl ItemDetails {
    fn from_wire(wire: WireDetails) -> Self {
        // The first media source is the file; a track has exactly one.
        let source = wire.media_sources.into_iter().next();
        let (source_path, source_container, size, source_bitrate, source_streams) = match source {
            Some(s) => (s.path, s.container, s.size, s.bitrate, s.media_streams),
            None => (None, None, None, None, Vec::new()),
        };
        let streams = if source_streams.is_empty() {
            wire.media_streams
        } else {
            source_streams
        };
        let audio = streams
            .into_iter()
            .find(|stream| stream.kind.as_deref() == Some("Audio"));
        let user = wire.item.user_data.clone().unwrap_or_default();
        let (codec, stream_bitrate, sample_rate, bit_depth, channels, channel_layout) = match audio
        {
            Some(a) => (
                a.codec,
                a.bit_rate,
                a.sample_rate,
                a.bit_depth,
                a.channels,
                a.channel_layout,
            ),
            None => (None, None, None, None, None, None),
        };
        Self {
            path: source_path.or(wire.path),
            container: source_container.or(wire.container),
            codec,
            bitrate: positive(stream_bitrate).or(positive(source_bitrate)),
            sample_rate: positive(sample_rate),
            bit_depth: positive(bit_depth),
            channels: positive(channels),
            channel_layout,
            size: positive(size),
            date_added: wire.date_created,
            play_count: user.play_count,
            last_played: user.last_played,
            item: wire.item,
        }
    }
}

impl JellyfinClient {
    /// One item in full, with its file's details, from `GET /Items/{id}`.
    pub async fn item_details(&self, item_id: &str) -> Result<ItemDetails> {
        let query = [("userId", self.user_id()?.to_owned())];
        let wire: WireDetails = self.get(&["Items", item_id], &query).await?;
        Ok(ItemDetails::from_wire(wire))
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    fn decode(json: &str) -> ItemDetails {
        ItemDetails::from_wire(serde_json::from_str(json).unwrap())
    }

    #[test]
    fn a_flac_track_reads_from_its_media_source() {
        let details = decode(include_str!("../tests/fixtures/item_details.json"));
        assert_eq!(details.item.name, "Harbour Lights");
        assert_eq!(
            details.path.as_deref(),
            Some("/media/music/The Quiet Engines/Low Tide Radio/01 Harbour Lights.flac")
        );
        assert_eq!(details.container.as_deref(), Some("flac"));
        assert_eq!(details.codec.as_deref(), Some("flac"));
        assert_eq!(details.bitrate, Some(1_043_201));
        assert_eq!(details.sample_rate, Some(96_000));
        assert_eq!(details.bit_depth, Some(24));
        assert_eq!(details.channels, Some(2));
        assert_eq!(details.channel_layout.as_deref(), Some("stereo"));
        assert_eq!(details.size, Some(31_754_112));
        assert_eq!(details.play_count, 12);
        // 2025-11-02T09:30:00.0000000Z
        assert_eq!(
            details.date_added,
            Some(UNIX_EPOCH + Duration::from_secs(1_762_075_800))
        );
        assert!(details.last_played.is_some());
    }

    #[test]
    fn the_top_level_fields_stand_in_when_there_is_no_media_source() {
        let details = decode(
            r#"{"Id":"x","Path":"/m/a.mp3","Container":"mp3",
                "MediaStreams":[{"Type":"EmbeddedImage","Codec":"mjpeg"},
                                {"Type":"Audio","Codec":"mp3","BitRate":320000,"SampleRate":44100,"Channels":2}]}"#,
        );
        assert_eq!(details.path.as_deref(), Some("/m/a.mp3"));
        assert_eq!(details.container.as_deref(), Some("mp3"));
        // The audio stream, not the cover art stream before it.
        assert_eq!(details.codec.as_deref(), Some("mp3"));
        assert_eq!(details.bitrate, Some(320_000));
        assert_eq!(details.bit_depth, None);
    }

    #[test]
    fn an_unprobed_item_has_no_details_and_no_plays() {
        let details = decode(r#"{"Id":"x","MediaSources":null,"BitRate":0}"#);
        assert_eq!(details.codec, None);
        assert_eq!(details.size, None);
        assert_eq!(details.play_count, 0);
        assert_eq!(details.date_added, None);
    }

    #[test]
    fn zero_and_negative_numbers_are_unknown() {
        let details = decode(
            r#"{"Id":"x","MediaSources":[{"Size":0,"Bitrate":-1,
                "MediaStreams":[{"Type":"Audio","SampleRate":0,"BitDepth":-3}]}]}"#,
        );
        assert_eq!(details.size, None);
        assert_eq!(details.bitrate, None);
        assert_eq!(details.sample_rate, None);
        assert_eq!(details.bit_depth, None);
    }
}
