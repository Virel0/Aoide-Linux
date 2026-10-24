//! From a source to frames at the device's rate, with markers saying where each
//! track starts and ends.
//!
//! A pipeline plays one track and then, gaplessly, whatever is chained after
//! it. Consecutive tracks at the same rate share one resampler, so a join
//! through a rate conversion is as continuous as the music. The output is a
//! plain run of frames plus markers at absolute frame positions in that run;
//! the render core reads the frames up to each marker and switches tracks
//! exactly there.
//!
//! A [`Feed`] carries a pipeline's output to the render core. The threaded
//! feed decodes on its own thread into lock-free rings, which is what the
//! audio callback reads; the inline feed decodes on demand, which is what the
//! offline tests use so their output does not depend on timing.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::thread;
use std::time::Duration;

use crate::decode::{Decoder, MediaSource};
use crate::error::{Error, Result};
use crate::loudness::linear_gain;
use crate::mix::SoundBounds;
use crate::queue::EntryId;
use crate::resample::{StreamResampler, scale};

/// One track, as the engine needs it.
#[derive(Clone, Debug)]
pub struct TrackSpec {
    pub id: EntryId,
    pub source: MediaSource,
    /// Play only this span of the file.
    pub bounds: Option<SoundBounds>,
    /// Loudness levelling, in dB; only a negative gain has any effect.
    pub gain_db: Option<f64>,
}

impl TrackSpec {
    pub fn new(id: EntryId, source: MediaSource) -> Self {
        Self {
            id,
            source,
            bounds: None,
            gain_db: None,
        }
    }

    pub fn with_bounds(mut self, bounds: Option<SoundBounds>) -> Self {
        self.bounds = bounds;
        self
    }

    pub fn with_gain_db(mut self, gain_db: Option<f64>) -> Self {
        self.gain_db = gain_db;
        self
    }
}

/// A position in a pipeline's output where something happens.
#[derive(Debug)]
pub(crate) enum Marker {
    Start {
        id: EntryId,
        gain: f32,
        /// Milliseconds into the file of the first frame after this marker.
        origin_ms: f64,
        /// Output frames from here to the track's end, when known.
        length: Option<u64>,
    },
    End {
        id: EntryId,
    },
    Error {
        id: EntryId,
        message: String,
    },
}

/// Where a pipeline's output goes.
#[derive(Default)]
pub(crate) struct Staging {
    samples: VecDeque<f32>,
    markers: VecDeque<(u64, Marker)>,
    /// Frames ever staged.
    staged: u64,
}

impl Staging {
    fn audio(&mut self, samples: &[f32], channels: usize) {
        self.samples.extend(samples.iter().copied());
        self.staged += (samples.len() / channels) as u64;
    }

    fn marker(&mut self, at: u64, marker: Marker) {
        self.markers.push_back((at, marker));
    }

    fn is_empty(&self) -> bool {
        self.samples.is_empty() && self.markers.is_empty()
    }

    fn frames(&self, channels: usize) -> usize {
        self.samples.len() / channels
    }
}

struct Active {
    spec: TrackSpec,
    decoder: Decoder,
    rate: u32,
    /// First and one-past-last source frame to play.
    start: u64,
    end: Option<u64>,
}

enum Next {
    None,
    Opening(TrackSpec, Receiver<Result<Decoder>>),
}

pub(crate) struct Pipeline {
    rate: u32,
    channels: usize,
    current: Option<Active>,
    next: Next,
    resampler: Option<StreamResampler>,
    /// Output frames emitted before the current resampler was made.
    resampler_base: u64,
    /// Output frames emitted so far, resampler output included.
    emitted: u64,
    mapped: Vec<f32>,
}

impl Pipeline {
    /// A pipeline starting `spec` at `start_ms` into the file (clamped to its
    /// sound bounds). Open errors arrive as an error marker, not here.
    pub(crate) fn new(
        rate: u32,
        channels: usize,
        spec: TrackSpec,
        start_ms: u64,
        out: &mut Staging,
    ) -> Self {
        let mut pipeline = Self {
            rate,
            channels,
            current: None,
            next: Next::None,
            resampler: None,
            resampler_base: 0,
            emitted: 0,
            mapped: Vec::new(),
        };
        match Decoder::open(&spec.source) {
            Ok(decoder) => pipeline.begin(spec, decoder, start_ms, out),
            Err(error) => pipeline.report(spec.id, error, out),
        }
        pipeline
    }

    /// Chains `spec` after the current track, replacing anything chained
    /// before. Opening starts at once, on its own thread, so a stream's first
    /// bytes are on their way long before they are needed.
    pub(crate) fn chain(&mut self, spec: Option<TrackSpec>) {
        self.next = match spec {
            None => Next::None,
            Some(spec) => {
                let (tx, rx) = mpsc::sync_channel(1);
                let source = spec.source.clone();
                let opened = thread::Builder::new()
                    .name("aoide-open".into())
                    .spawn(move || {
                        let _ = tx.send(Decoder::open(&source));
                    });
                if opened.is_err() {
                    // No thread to spare: the open happens when it is needed.
                    let (tx, rx) = mpsc::sync_channel(1);
                    let _ = tx.send(Decoder::open(&spec.source));
                    Next::Opening(spec, rx)
                } else {
                    Next::Opening(spec, rx)
                }
            }
        };
    }

    /// Nothing playing and nothing chained.
    pub(crate) fn is_idle(&self) -> bool {
        self.current.is_none() && matches!(self.next, Next::None)
    }

    /// Decodes one packet, or moves on to the chained track.
    pub(crate) fn step(&mut self, out: &mut Staging) {
        let Some(active) = self.current.as_mut() else {
            self.advance(out);
            return;
        };
        let decoded = match active.decoder.next_packet() {
            Ok(Some(decoded)) => decoded,
            Ok(None) => return self.finish_current(out),
            Err(error) => {
                let id = active.spec.id;
                self.report(id, error, out);
                return self.finish_current(out);
            }
        };

        // Keep only the frames inside the sound bounds.
        let frames = decoded.frames() as u64;
        let from = active.start.max(decoded.first_frame);
        let to = match active.end {
            Some(end) => end.min(decoded.first_frame + frames),
            None => decoded.first_frame + frames,
        };
        let channels_in = decoded.channels;
        let past_end = active
            .end
            .is_some_and(|end| decoded.first_frame + frames >= end);
        if to > from {
            let a = ((from - decoded.first_frame) as usize) * channels_in;
            let b = ((to - decoded.first_frame) as usize) * channels_in;
            map_channels(
                &decoded.samples[a..b],
                channels_in,
                self.channels,
                &mut self.mapped,
            );
            let rate = active.rate;
            self.emit_mapped(rate, out);
        }
        if past_end {
            self.finish_current(out);
        }
    }

    fn begin(&mut self, spec: TrackSpec, mut decoder: Decoder, start_ms: u64, out: &mut Staging) {
        let rate = decoder.sample_rate();
        let ms_to_frames = |ms: u64| scale(ms, 1000, rate);
        let bound_start = spec.bounds.map_or(0, |b| ms_to_frames(b.start_ms));
        let mut end = spec.bounds.map(|b| ms_to_frames(b.end_ms));
        if let Some(total) = decoder.total_frames() {
            end = Some(end.map_or(total, |end| end.min(total)));
        }
        let start = bound_start.max(ms_to_frames(start_ms));

        if start > 0 && decoder.seek(start).is_err() {
            // Not seekable: decode from the top and drop what comes before.
            match Decoder::open(&spec.source) {
                Ok(fresh) => decoder = fresh,
                Err(error) => return self.report(spec.id, error, out),
            }
        }

        // A change of rate ends the old resampler here, flushed, so the
        // previous track's last frames are not left inside it.
        if self
            .resampler
            .as_ref()
            .is_some_and(|r| r.input_rate() != rate)
            || rate == self.rate
        {
            self.flush_resampler(out);
        }
        if rate != self.rate && self.resampler.is_none() {
            match StreamResampler::new(rate, self.rate, self.channels) {
                Ok(resampler) => {
                    self.resampler_base = self.emitted;
                    self.resampler = Some(resampler);
                }
                Err(error) => return self.report(spec.id, error, out),
            }
        }

        let at = self.output_position();
        let length = end.map(|end| scale(end.saturating_sub(start), rate, self.rate));
        out.marker(
            at,
            Marker::Start {
                id: spec.id,
                gain: linear_gain(spec.gain_db),
                origin_ms: start as f64 * 1000.0 / f64::from(rate),
                length,
            },
        );
        self.current = Some(Active {
            spec,
            decoder,
            rate,
            start,
            end,
        });
    }

    /// Where the next input frame will come out, counting the resampler's lag.
    fn output_position(&self) -> u64 {
        match &self.resampler {
            Some(resampler) => {
                self.resampler_base + resampler.output_index(resampler.input_frames())
            }
            None => self.emitted,
        }
    }

    fn finish_current(&mut self, out: &mut Staging) {
        let Some(active) = self.current.take() else {
            return;
        };
        let chained = matches!(self.next, Next::Opening(..));
        if !chained {
            self.flush_resampler(out);
        }
        out.marker(self.output_position(), Marker::End { id: active.spec.id });
        if chained {
            self.advance(out);
        }
    }

    fn advance(&mut self, out: &mut Staging) {
        match std::mem::replace(&mut self.next, Next::None) {
            Next::None => self.flush_resampler(out),
            Next::Opening(spec, opening) => {
                let opened = opening
                    .recv()
                    .unwrap_or_else(|_| Err(Error::Open("the open was abandoned".into())));
                match opened {
                    Ok(decoder) => self.begin(spec, decoder, 0, out),
                    Err(error) => {
                        let id = spec.id;
                        self.report(id, error, out);
                        out.marker(self.output_position(), Marker::End { id });
                        self.flush_resampler(out);
                    }
                }
            }
        }
    }

    fn report(&mut self, id: EntryId, error: Error, out: &mut Staging) {
        out.marker(
            self.output_position(),
            Marker::Error {
                id,
                message: error.to_string(),
            },
        );
    }

    fn emit_mapped(&mut self, rate: u32, out: &mut Staging) {
        let channels = self.channels;
        if rate == self.rate {
            out.audio(&self.mapped, channels);
            self.emitted += (self.mapped.len() / channels) as u64;
            return;
        }
        let Some(resampler) = self.resampler.as_mut() else {
            return;
        };
        let mut produced = 0u64;
        let result = resampler.push(&self.mapped, &mut |samples| {
            out.audio(samples, channels);
            produced += (samples.len() / channels) as u64;
        });
        self.emitted += produced;
        if let (Err(error), Some(active)) = (result, self.current.as_ref()) {
            let id = active.spec.id;
            self.report(id, error, out);
        }
    }

    fn flush_resampler(&mut self, out: &mut Staging) {
        let Some(mut resampler) = self.resampler.take() else {
            return;
        };
        let channels = self.channels;
        let mut produced = 0u64;
        let _ = resampler.flush(&mut |samples| {
            out.audio(samples, channels);
            produced += (samples.len() / channels) as u64;
        });
        self.emitted += produced;
    }
}

/// Maps interleaved `input` with `from` channels to `to` channels: mono is
/// copied to every output channel, more channels than the output keep the
/// first ones (front left and right), and a mono output averages them all.
pub(crate) fn map_channels(input: &[f32], from: usize, to: usize, out: &mut Vec<f32>) {
    out.clear();
    if from == to {
        out.extend_from_slice(input);
        return;
    }
    let frames = input.len() / from;
    out.reserve(frames * to);
    for frame in input.chunks_exact(from) {
        if to == 1 {
            out.push(frame.iter().sum::<f32>() / from as f32);
        } else if from == 1 {
            out.extend(std::iter::repeat_n(frame[0], to));
        } else {
            out.extend(frame.iter().copied().chain(std::iter::repeat(0.0)).take(to));
        }
    }
}

/// What a feed has for the render core next.
#[derive(Debug)]
pub(crate) enum FeedRead {
    /// This many frames were written, all before the next marker.
    Audio(usize),
    Marker(Marker),
    /// Nothing ready yet; the stream is still coming.
    Starved,
    /// Nothing more will come.
    Finished,
}

/// The consumer side of a pipeline.
pub(crate) trait Feed: Send {
    /// Writes up to `out.len() / channels` frames, stopping at a marker.
    fn read(&mut self, out: &mut [f32]) -> FeedRead;
    /// Chains a track after the current one, or clears what was chained.
    fn chain(&mut self, spec: Option<TrackSpec>);
}

/// Decodes on demand, on the caller's thread. Deterministic, for offline
/// rendering and tests; not for the audio callback, where decoding a network
/// stream would block it.
pub(crate) struct InlineFeed {
    pipeline: Pipeline,
    staging: Staging,
    channels: usize,
    read: u64,
}

impl InlineFeed {
    pub(crate) fn new(rate: u32, channels: usize, spec: TrackSpec, start_ms: u64) -> Self {
        let mut staging = Staging::default();
        let pipeline = Pipeline::new(rate, channels, spec, start_ms, &mut staging);
        Self {
            pipeline,
            staging,
            channels,
            read: 0,
        }
    }
}

impl Feed for InlineFeed {
    fn read(&mut self, out: &mut [f32]) -> FeedRead {
        loop {
            if let Some(&(at, _)) = self.staging.markers.front()
                && at <= self.read
            {
                let (_, marker) = self.staging.markers.pop_front().expect("checked above");
                return FeedRead::Marker(marker);
            }
            let available = self.staging.frames(self.channels);
            if available > 0 {
                let until_marker = self
                    .staging
                    .markers
                    .front()
                    .map_or(u64::MAX, |&(at, _)| at - self.read);
                let frames = (out.len() / self.channels)
                    .min(available)
                    .min(until_marker as usize);
                if frames > 0 {
                    for (slot, sample) in out[..frames * self.channels]
                        .iter_mut()
                        .zip(self.staging.samples.drain(..frames * self.channels))
                    {
                        *slot = sample;
                    }
                    self.read += frames as u64;
                    return FeedRead::Audio(frames);
                }
            }
            if self.pipeline.is_idle() && self.staging.is_empty() {
                return FeedRead::Finished;
            }
            if self.pipeline.is_idle() && available == 0 {
                // Markers past the end of the audio: deliver them anyway.
                if let Some((_, marker)) = self.staging.markers.pop_front() {
                    return FeedRead::Marker(marker);
                }
            }
            self.pipeline.step(&mut self.staging);
        }
    }

    fn chain(&mut self, spec: Option<TrackSpec>) {
        self.pipeline.chain(spec);
    }
}

/// How much decoded audio a threaded feed keeps ahead of the speaker.
const AHEAD_SECONDS: usize = 4;

/// Decodes on its own thread into lock-free rings the audio callback reads
/// without blocking or allocating.
pub(crate) struct ThreadFeed {
    audio: rtrb::Consumer<f32>,
    markers: rtrb::Consumer<(u64, Marker)>,
    control: SyncSender<Option<TrackSpec>>,
    idle: Arc<AtomicBool>,
    channels: usize,
    read: u64,
}

impl ThreadFeed {
    pub(crate) fn spawn(
        rate: u32,
        channels: usize,
        spec: TrackSpec,
        start_ms: u64,
    ) -> Result<Self> {
        let (audio_tx, audio) = rtrb::RingBuffer::new(rate as usize * channels * AHEAD_SECONDS);
        let (marker_tx, markers) = rtrb::RingBuffer::new(64);
        let (control, control_rx) = mpsc::sync_channel(4);
        let idle = Arc::new(AtomicBool::new(false));
        let thread_idle = Arc::clone(&idle);
        thread::Builder::new()
            .name("aoide-decode".into())
            .spawn(move || {
                produce(
                    rate,
                    channels,
                    spec,
                    start_ms,
                    audio_tx,
                    marker_tx,
                    control_rx,
                    thread_idle,
                )
            })
            .map_err(|e| Error::Output(e.to_string()))?;
        Ok(Self {
            audio,
            markers,
            control,
            idle,
            channels,
            read: 0,
        })
    }
}

impl Feed for ThreadFeed {
    fn read(&mut self, out: &mut [f32]) -> FeedRead {
        // Read the flag before the rings: everything pushed before it was set
        // is visible after it.
        let idle = self.idle.load(Ordering::Acquire);
        if let Ok(&(at, _)) = self.markers.peek()
            && at <= self.read
        {
            let (_, marker) = self.markers.pop().expect("peeked above");
            return FeedRead::Marker(marker);
        }
        let until_marker = self
            .markers
            .peek()
            .map_or(u64::MAX, |&(at, _)| at - self.read);
        let available = self.audio.slots() / self.channels;
        let frames = (out.len() / self.channels)
            .min(available)
            .min(until_marker as usize);
        if frames > 0 {
            let samples = frames * self.channels;
            if let Ok(chunk) = self.audio.read_chunk(samples) {
                let (a, b) = chunk.as_slices();
                out[..a.len()].copy_from_slice(a);
                out[a.len()..samples].copy_from_slice(b);
                chunk.commit_all();
                self.read += frames as u64;
                return FeedRead::Audio(frames);
            }
        }
        if idle {
            if let Ok((_, marker)) = self.markers.pop() {
                return FeedRead::Marker(marker);
            }
            if self.audio.is_empty() {
                return FeedRead::Finished;
            }
        }
        FeedRead::Starved
    }

    fn chain(&mut self, spec: Option<TrackSpec>) {
        // Never blocks: a full control queue means the decoder thread is
        // already behind, and the next chain replaces this one anyway.
        let _ = self.control.try_send(spec);
    }
}

#[allow(clippy::too_many_arguments)]
fn produce(
    rate: u32,
    channels: usize,
    spec: TrackSpec,
    start_ms: u64,
    mut audio: rtrb::Producer<f32>,
    mut markers: rtrb::Producer<(u64, Marker)>,
    control: Receiver<Option<TrackSpec>>,
    idle: Arc<AtomicBool>,
) {
    let mut staging = Staging::default();
    let mut pipeline = Pipeline::new(rate, channels, spec, start_ms, &mut staging);
    let mut moved: u64 = 0;
    loop {
        if audio.is_abandoned() {
            return;
        }
        loop {
            match control.try_recv() {
                Ok(spec) => {
                    idle.store(false, Ordering::Release);
                    pipeline.chain(spec);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }

        // Markers go across once the audio before them has.
        while let Some(&(at, _)) = staging.markers.front() {
            if at > moved && !(pipeline.is_idle() && staging.samples.is_empty()) {
                break;
            }
            if markers.is_full() {
                break;
            }
            let entry = staging.markers.pop_front().expect("checked above");
            let _ = markers.push(entry);
        }
        let until_marker = staging
            .markers
            .front()
            .map_or(u64::MAX, |&(at, _)| at.saturating_sub(moved));
        let frames = (audio.slots() / channels)
            .min(staging.frames(channels))
            .min(until_marker as usize);
        if frames > 0
            && let Ok(mut chunk) = audio.write_chunk(frames * channels)
        {
            let (a, b) = chunk.as_mut_slices();
            for (slot, sample) in a
                .iter_mut()
                .chain(b.iter_mut())
                .zip(staging.samples.drain(..frames * channels))
            {
                *slot = sample;
            }
            chunk.commit_all();
            moved += frames as u64;
            continue;
        }

        let room = audio.slots() / channels;
        let staged = staging.frames(channels);
        if !pipeline.is_idle() && staged < room.max(1) && staged < rate as usize / 4 {
            pipeline.step(&mut staging);
            continue;
        }
        if pipeline.is_idle() && staging.is_empty() {
            idle.store(true, Ordering::Release);
            match control.recv_timeout(Duration::from_millis(50)) {
                Ok(spec) => {
                    idle.store(false, Ordering::Release);
                    pipeline.chain(spec);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
            continue;
        }
        thread::sleep(Duration::from_millis(5));
    }
}
