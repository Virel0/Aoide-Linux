//! What the server knows about one track's file, for a Get Info screen.

use crate::client::MeleteClient;
use crate::model::{WirePage, WireTrack};
use crate::{Error, Item, Result};

/// One track with its file's technical details. Any of them may be unknown: a
/// file the server hasn't probed yet has none.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemDetails {
    /// The track, as the lists return it.
    pub item: Item,
    /// The audio codec: `flac`, `alac`, `aac`, `mp3`, …
    pub codec: Option<String>,
    /// Bits per second.
    pub bitrate: Option<u32>,
    /// Hertz.
    pub sample_rate: Option<u32>,
    /// Bits per sample. Lossy codecs have none.
    pub bit_depth: Option<u32>,
    /// How many audio channels: 2 for stereo.
    pub channels: Option<u32>,
}

/// A count or rate the server sent; zero means unknown.
fn positive(value: Option<u32>) -> Option<u32> {
    value.filter(|v| *v > 0)
}

impl ItemDetails {
    fn from_wire(track: WireTrack) -> Self {
        Self {
            codec: track.codec.clone(),
            bitrate: positive(track.bitrate_kbps).and_then(|kbps| kbps.checked_mul(1000)),
            sample_rate: positive(track.sample_rate),
            bit_depth: positive(track.bit_depth),
            channels: positive(track.channels),
            item: Item::from(track),
        }
    }
}

impl MeleteClient {
    /// One track in full, with its file's details.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] for a track the server doesn't have (or doesn't let
    /// this user see); otherwise the [request errors](MeleteClient#errors).
    pub async fn item_details(&self, track_id: &str) -> Result<ItemDetails> {
        let segments = ["api", "v1", "music", "tracks"];
        let page: WirePage<WireTrack> = self.get(&segments, &[("id", track_id.to_owned())]).await?;
        page.items
            .into_iter()
            .find(|track| track.id.to_string() == track_id)
            .map(ItemDetails::from_wire)
            .ok_or_else(|| Error::NotFound {
                path: format!("{}/{track_id}", segments.join("/")),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(json: &str) -> ItemDetails {
        ItemDetails::from_wire(serde_json::from_str(json).unwrap())
    }

    #[test]
    fn a_flac_track_has_its_files_details() {
        let details = decode(include_str!("../tests/fixtures/track.json"));
        assert_eq!(details.item.name, "Opening");
        assert_eq!(details.codec.as_deref(), Some("flac"));
        assert_eq!(details.bitrate, Some(123_000));
        assert_eq!(details.sample_rate, Some(44_100));
        assert_eq!(details.bit_depth, Some(16));
        assert_eq!(details.channels, Some(2));
    }

    #[test]
    fn an_unprobed_track_has_no_details() {
        let details = decode(
            r#"{"id":1,"title":"t","album_id":2,"album":"a","album_artist":"b",
                "track_number":null,"disc_number":null,"year":null,"duration_seconds":null,
                "genres":[],"codec":null,"bitrate_kbps":0,"sample_rate":null,
                "bit_depth":null,"channels":null,"isrc":null,"liked":false,"artists":[]}"#,
        );
        assert_eq!(details.codec, None);
        assert_eq!(details.bitrate, None);
        assert_eq!(details.sample_rate, None);
    }
}
