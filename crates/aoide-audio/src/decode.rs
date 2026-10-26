//! Decoding with symphonia, from a local file or an HTTP stream.

use std::io::{self, Read, Seek, SeekFrom};
use std::path::PathBuf;

use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::{AudioCodecId, AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, MetadataRevision, StandardTag};
use symphonia::core::units::{TimeBase, Timestamp};

use crate::error::{Error, Result};
use crate::http::HttpSource;
use crate::loudness::{ReplayGainTags, parse_tag_number};

/// Where a track's bytes come from.
#[derive(Clone, Debug)]
pub enum MediaSource {
    /// A local file.
    File(PathBuf),
    /// Cloning shares one download, so reopening a stream to seek in it does
    /// not fetch it twice.
    Http(HttpSource),
}

impl MediaSource {
    /// A local file at `path`.
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
    /// The file's frame index of the first frame here.
    pub first_frame: u64,
    /// Samples per frame.
    pub channels: usize,
    /// Interleaved samples, nominally −1 to 1.
    pub samples: &'a [f32],
}

impl Decoded<'_> {
    /// Whole frames in the packet.
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
    replay_gain: ReplayGainTags,
    buf: Vec<f32>,
    /// The frame index the next decoded packet starts at, when it follows on.
    next_frame: u64,
    /// After a seek, the next packet's position is read from its timestamp.
    resync: bool,
}

impl Decoder {
    /// Opens and probes `source`, ready to decode its default audio track.
    ///
    /// # Errors
    ///
    /// [`Error::Open`] when the file or stream cannot be read, and
    /// [`Error::Unsupported`] when it is not a container or codec this build decodes, or
    /// it has no audio track or sample rate.
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
        let stream = tolerate_flac_block_sizes(stream).map_err(|e| Error::Open(e.to_string()))?;
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

    fn from_format(mut format: Box<dyn FormatReader>) -> Result<Self> {
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
        let replay_gain = read_replay_gain(&mut *format, track_id);
        Ok(Self {
            format,
            decoder,
            track_id,
            time_base,
            sample_rate,
            channels,
            total_frames,
            replay_gain,
            buf: Vec::new(),
            next_frame: 0,
            resync: false,
        })
    }

    /// The file's own sample rate, in hertz.
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

    /// The ReplayGain tags the file carries (Vorbis comments in FLAC, Ogg
    /// and Opus, ID3v2 `TXXX` frames, APE tags, iTunes `----` atoms in MP4),
    /// as read when it was opened.
    pub fn replay_gain(&self) -> ReplayGainTags {
        self.replay_gain
    }

    /// Seeks so the next packet holds `frame` (sample-accurate seek: the
    /// packet may start before it, and the caller discards up to it).
    ///
    /// # Errors
    ///
    /// [`Error::Seek`] when the container cannot seek there.
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
    ///
    /// # Errors
    ///
    /// [`Error::Decode`] when the container fails to read; a packet that fails to decode
    /// is skipped instead.
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

/// The ReplayGain tags in every metadata revision read so far: those before
/// the container (an ID3v2 tag), the container's own, and the track's. A
/// later revision overrides an earlier one, field by field.
fn read_replay_gain(format: &mut dyn FormatReader, track_id: u32) -> ReplayGainTags {
    let mut tags = ReplayGainTags::default();
    let mut metadata = format.metadata();
    let mut take = |revision: &MetadataRevision| {
        let track = revision
            .per_track
            .iter()
            .filter(|per_track| per_track.track_id == u64::from(track_id))
            .flat_map(|per_track| per_track.metadata.tags.iter());
        for tag in revision.media.tags.iter().chain(track) {
            let (slot, text) = match &tag.std {
                Some(StandardTag::ReplayGainTrackGain(text)) => (&mut tags.track_gain_db, text),
                Some(StandardTag::ReplayGainTrackPeak(text)) => (&mut tags.track_peak, text),
                Some(StandardTag::ReplayGainAlbumGain(text)) => (&mut tags.album_gain_db, text),
                Some(StandardTag::ReplayGainAlbumPeak(text)) => (&mut tags.album_peak, text),
                _ => continue,
            };
            if let Some(value) = parse_tag_number(text) {
                *slot = Some(value);
            }
        }
    };
    loop {
        if let Some(revision) = metadata.current() {
            take(revision);
        }
        if metadata.pop().is_none() {
            break;
        }
    }
    tags
}

/// The name of a codec a music library can hold that this build has no
/// decoder for. symphonia has no Opus decoder; Jellyfin transcodes Opus for
/// us (the stream URL never offers it), so one only reaches here from a
/// local file or a server that ignored the list.
fn missing_codec(codec: AudioCodecId) -> Option<&'static str> {
    (codec == CODEC_ID_OPUS).then_some("Opus")
}

/// symphonia 0.6.1 decides a FLAC stream's blocking strategy from STREAMINFO
/// (fixed when the minimum and maximum block sizes are equal) instead of from
/// the frames, whose headers say it in their sync code. A fixed-block-size
/// stream whose minimum is its short last block (what flacenc writes; the
/// spec says the minimum excludes the last block) then has every frame
/// header refused as out of sequence and fails with an unexpected end of
/// file; a last block under 16 frames fails STREAMINFO's own check. For
/// exactly that case, a fixed-block-size stream with unequal sizes, this
/// presents STREAMINFO's minimum as the maximum, which is what the spec has
/// it mean. Anything else passes through untouched.
fn tolerate_flac_block_sizes(
    mut stream: Box<dyn symphonia::core::io::MediaSource>,
) -> io::Result<Box<dyn symphonia::core::io::MediaSource>> {
    let max = fixed_block_size_to_restore(&mut stream).unwrap_or(None);
    stream.seek(SeekFrom::Start(0))?;
    Ok(match max {
        Some(max) => Box::new(PatchedFlacHead {
            inner: stream,
            pos: 0,
            min: max.to_be_bytes(),
        }),
        None => stream,
    })
}

/// Where STREAMINFO's minimum block size sits: after `fLaC` and the
/// metadata block header.
const FLAC_MIN_BLOCK_AT: u64 = 8;

/// The maximum block size, when `stream` is a fixed-block-size FLAC stream
/// whose STREAMINFO gives a different minimum.
fn fixed_block_size_to_restore(stream: &mut (impl Read + Seek)) -> io::Result<Option<u16>> {
    let mut head = [0u8; 12];
    stream.read_exact(&mut head)?;
    // STREAMINFO must be the first metadata block (type 0).
    if &head[..4] != b"fLaC" || head[4] & 0x7f != 0 {
        return Ok(None);
    }
    let (min, max) = (
        u16::from_be_bytes([head[8], head[9]]),
        u16::from_be_bytes([head[10], head[11]]),
    );
    if min == max || max < 16 {
        return Ok(None);
    }
    // Past the metadata blocks to the first frame.
    let mut at = 4u64;
    loop {
        stream.seek(SeekFrom::Start(at))?;
        let mut header = [0u8; 4];
        stream.read_exact(&mut header)?;
        at += 4 + u64::from(u32::from_be_bytes([0, header[1], header[2], header[3]]));
        if header[0] & 0x80 != 0 {
            break;
        }
    }
    stream.seek(SeekFrom::Start(at))?;
    // The frame sync code: 0xFFF8 is a fixed block size, 0xFFF9 variable.
    let mut sync = [0u8; 2];
    stream.read_exact(&mut sync)?;
    Ok((sync == [0xff, 0xf8]).then_some(max))
}

/// A FLAC stream with STREAMINFO's minimum block size replaced.
struct PatchedFlacHead {
    inner: Box<dyn symphonia::core::io::MediaSource>,
    pos: u64,
    min: [u8; 2],
}

impl Read for PatchedFlacHead {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        for (index, &byte) in self.min.iter().enumerate() {
            let at = FLAC_MIN_BLOCK_AT + index as u64;
            if (self.pos..self.pos + read as u64).contains(&at) {
                buf[(at - self.pos) as usize] = byte;
            }
        }
        self.pos += read as u64;
        Ok(read)
    }
}

impl Seek for PatchedFlacHead {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.pos = self.inner.seek(to)?;
        Ok(self.pos)
    }
}

impl symphonia::core::io::MediaSource for PatchedFlacHead {
    fn is_seekable(&self) -> bool {
        self.inner.is_seekable()
    }

    fn byte_len(&self) -> Option<u64> {
        self.inner.byte_len()
    }
}
