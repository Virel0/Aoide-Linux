//! The threaded feed: a decoder thread ahead of the speaker, through
//! lock-free rings.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::thread;
use std::time::Duration;

use super::gapless::Pipeline;
use super::{Feed, FeedRead, Marker, Staging, TrackSpec};
use crate::error::{Error, Result};

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

/// The decoder thread: runs the pipeline into the rings, keeping markers
/// behind the audio before them, until the feed is dropped.
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
        if audio.is_abandoned() || !take_chains(&control, &idle, &mut pipeline) {
            return;
        }
        move_markers(&mut staging, &mut markers, moved, pipeline.is_idle());
        let frames = move_audio(&mut staging, &mut audio, channels, moved);
        if frames > 0 {
            moved += frames;
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

/// Chains every track the feed has sent since the last look. False when the
/// feed has gone.
fn take_chains(
    control: &Receiver<Option<TrackSpec>>,
    idle: &AtomicBool,
    pipeline: &mut Pipeline,
) -> bool {
    loop {
        match control.try_recv() {
            Ok(spec) => {
                idle.store(false, Ordering::Release);
                pipeline.chain(spec);
            }
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => return false,
        }
    }
}

/// Markers go across once the audio before them has (all of them once the
/// pipeline is idle and its audio has gone), while the ring has room.
fn move_markers(
    staging: &mut Staging,
    markers: &mut rtrb::Producer<(u64, Marker)>,
    moved: u64,
    pipeline_idle: bool,
) {
    while let Some(&(at, _)) = staging.markers.front() {
        if at > moved && !(pipeline_idle && staging.samples.is_empty()) {
            break;
        }
        if markers.is_full() {
            break;
        }
        let entry = staging.markers.pop_front().expect("checked above");
        let _ = markers.push(entry);
    }
}

/// Moves staged audio into the ring, as much as fits and no further than
/// the next marker still staged. Returns the frames moved.
fn move_audio(
    staging: &mut Staging,
    audio: &mut rtrb::Producer<f32>,
    channels: usize,
    moved: u64,
) -> u64 {
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
        return frames as u64;
    }
    0
}
