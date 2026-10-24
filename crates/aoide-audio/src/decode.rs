//! Decoding with symphonia, from a local file or an HTTP stream.

use std::path::PathBuf;

use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::{AudioCodecId, AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{TimeBase, Timestamp};

use crate::error::{Error, Result};
use crate::http::HttpSource;

/// Where a track's bytes come from.
#[derive(Clone, Debug)]
pub enum MediaSource {
    File(PathBuf),
    /// Cloning shares one download, so reopening a stream to seek in it does
    /// not fetch it twice.
    Http(HttpSource),
}

impl MediaSource {
    pub fn file(path: impl Into<PathBuf>) -> Self {
        MediaSource::File(path.into())
    }

    /// A stream whose URL already carries its authentication.
    pub fn http(url: impl Into<String>) -> Self {
        MediaSource::Http(HttpSource::new(url))
    }
}

/// One decoded packet: interleaved samples at the file's own rate and channel
/// count, and the index of its first frame in the file.
pub struct Decoded<'a> {
    pub first_frame: u64,
    pub channels: usize,
    pub samples: &'a [f32],
}

impl Decoded<'_> {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }
}

/// A demuxer and decoder for the default audio track of one source.
pub struct Decoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    sample_rate: u32,
    channels: usize,
    total_frames: Option<u64>,
    buf: Vec<f32>,
    /// The frame index the next decoded packet starts at, when it follows on.
    next_frame: u64,
    /// After a seek, the next packet's position is read from its timestamp.
    resync: bool,
}

impl Decoder {
    pub fn open(source: &MediaSource) -> Result<Self> {
        let mut hint = Hint::new();
        let stream: Box<dyn symphonia::core::io::MediaSource> = match source {
            MediaSource::File(path) => {
                if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
                    hint.with_extension(extension);
                }
                Box::new(std::fs::File::open(path).map_err(|e| Error::Open(e.to_string()))?)
            }
            MediaSource::Http(http) => {
                let reader = http.open().map_err(|e| Error::Open(e.to_string()))?;
                if let Some(mime) = http.content_type() {
                    hint.mime_type(mime.split(';').next().unwrap_or_default().trim());
                }
                Box::new(reader)
            }
        };
        let mss = MediaSourceStream::new(stream, Default::default());
        let format = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|e| Error::Unsupported(e.to_string()))?;
        Self::from_format(format)
    }

    fn from_format(format: Box<dyn FormatReader>) -> Result<Self> {
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| Error::Unsupported("no audio track".into()))?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|params| params.audio())
            .ok_or_else(|| Error::Unsupported("no audio codec parameters".into()))?;
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .map_err(|e| match missing_codec(params.codec) {
                Some(name) => Error::Unsupported(name.into()),
                None => Error::Unsupported(e.to_string()),
            })?;
        let sample_rate = params
            .sample_rate
            .ok_or_else(|| Error::Unsupported("unknown sample rate".into()))?;
        let channels = params
            .channels
            .as_ref()
            .map_or(0, |channels| channels.count());
        let track_id = track.id;
        let time_base = track.time_base;
        let total_frames = track.num_frames;
        Ok(Self {
            format,
            decoder,
            track_id,
            time_base,
            sample_rate,
            channels,
            total_frames,
            buf: Vec::new(),
            next_frame: 0,
            resync: false,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// The channel count the container announces; zero when it does not, in
    /// which case the decoded packets say.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Playable frames in the file, when the container says.
    pub fn total_frames(&self) -> Option<u64> {
        self.total_frames
    }

    /// Seeks so the next packet holds `frame` (sample-accurate seek: the
    /// packet may start before it, and the caller discards up to it).
    pub fn seek(&mut self, frame: u64) -> Result<()> {
        let ts = self.frames_to_ts(frame);
        self.format
            .seek(
                SeekMode::Accurate,
                SeekTo::Timestamp {
                    ts: Timestamp::new(ts),
                    track_id: self.track_id,
                },
            )
            .map_err(|e| Error::Seek(e.to_string()))?;
        self.decoder.reset();
        self.resync = true;
        Ok(())
    }

    /// The next decoded packet, or `None` at the end of the stream. Packets
    /// that fail to decode are skipped, as a player should.
    pub fn next_packet(&mut self) -> Result<Option<Decoded<'_>>> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => return Ok(None),
                Err(SymphoniaError::ResetRequired) => return Ok(None),
                Err(SymphoniaError::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    return Ok(None);
                }
                Err(e) => return Err(Error::Decode(e.to_string())),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let pts = packet.pts.get();
            let decoded = match self.decoder.decode(&packet) {
                Ok(decoded) => decoded,
                Err(SymphoniaError::DecodeError(_)) | Err(SymphoniaError::IoError(_)) => continue,
                Err(e) => return Err(Error::Decode(e.to_string())),
            };
            let channels = decoded.spec().channels().count();
            decoded.copy_to_vec_interleaved::<f32>(&mut self.buf);
            if channels == 0 || self.buf.is_empty() {
                continue;
            }
            if self.channels == 0 {
                self.channels = channels;
            }
            let frames = (self.buf.len() / channels) as u64;
            if self.resync {
                self.resync = false;
                self.next_frame = self.ts_to_frames(pts.max(0) as u64);
            }
            let first_frame = self.next_frame;
            self.next_frame += frames;
            return Ok(Some(Decoded {
                first_frame,
                channels,
                samples: &self.buf,
            }));
        }
    }

    fn frames_to_ts(&self, frame: u64) -> i64 {
        match self.time_base {
            Some(tb) => {
                let (numer, denom) = (u128::from(tb.numer.get()), u128::from(tb.denom.get()));
                (u128::from(frame) * denom / (numer * u128::from(self.sample_rate))) as i64
            }
            None => frame as i64,
        }
    }

    fn ts_to_frames(&self, ts: u64) -> u64 {
        match self.time_base {
            Some(tb) => {
                let (numer, denom) = (u128::from(tb.numer.get()), u128::from(tb.denom.get()));
                (u128::from(ts) * numer * u128::from(self.sample_rate) / denom) as u64
            }
            None => ts,
        }
    }
}

/// The name of a codec a music library can hold that this build has no
/// decoder for. symphonia has no Opus decoder; Jellyfin transcodes Opus for
/// us (the stream URL never offers it), so one only reaches here from a
/// local file or a server that ignored the list.
fn missing_codec(codec: AudioCodecId) -> Option<&'static str> {
    (codec == CODEC_ID_OPUS).then_some("Opus")
}
