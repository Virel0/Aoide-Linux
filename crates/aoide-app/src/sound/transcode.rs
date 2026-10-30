//! Transcoding: the server converting each stream to a smaller one, at a
//! bitrate and in a format the decoder plays.

use serde::{Deserialize, Serialize};

/// Transcoding: off by default, since a converted stream loses the gapless
/// joins (Melete converts even files that would have played as they are).
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

/// What Melete converts to: formats the decoder plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TranscodeFormat {
    #[default]
    Aac,
    Mp3,
}

impl TranscodeFormat {
    pub const ALL: [TranscodeFormat; 2] = [TranscodeFormat::Aac, TranscodeFormat::Mp3];
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

/// The format the player's streams are asked for: the file as it is when
/// transcoding is off, else the format and bitrate chosen.
pub fn stream_format(transcode: &Transcode) -> aoide_melete::StreamFormat {
    use aoide_melete::StreamFormat;
    if !transcode.enabled {
        return StreamFormat::Original;
    }
    match transcode.format {
        TranscodeFormat::Aac => StreamFormat::Aac(transcode.bitrate_kbps),
        TranscodeFormat::Mp3 => StreamFormat::Mp3(transcode.bitrate_kbps),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use aoide_melete::StreamFormat;

    #[test]
    fn transcoding_off_streams_the_file_as_it_is() {
        assert_eq!(stream_format(&Transcode::default()), StreamFormat::Original);
    }

    #[test]
    fn transcoding_asks_for_the_format_at_the_bitrate() {
        let transcode = Transcode {
            enabled: true,
            bitrate_kbps: 192,
            format: TranscodeFormat::Mp3,
        };
        assert_eq!(stream_format(&transcode), StreamFormat::Mp3(192));
        let aac = Transcode {
            format: TranscodeFormat::Aac,
            ..transcode
        };
        assert_eq!(stream_format(&aac), StreamFormat::Aac(192));
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
