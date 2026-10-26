//! The engine and its render: decks into the block, then the equaliser, the
//! compressor, the fades and the volume.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64};
use std::sync::mpsc::{self, Receiver, SyncSender};

use super::blend::Blend;
use super::commands::Command;
use super::deck::{Deck, pull};
use super::fade::{Fade, FadeShape};
use super::handle::{EngineHandle, Shared};
use super::reaper::{Garbage, spawn_reaper};
use super::timeline::{Anchor, Timeline};
use super::{
    BLOCK, DECLICK_SECONDS, EngineConfig, EngineEvent, PAUSE_FADE_SECONDS, REMEMBERED, Transport,
    frames_of,
};
use crate::compressor::Compressor;
use crate::eq::Equalizer;
use crate::pipeline::TrackSpec;
use crate::visualizer::TapRing;

pub struct Engine {
    pub(super) config: EngineConfig,
    pub(super) commands: Receiver<Command>,
    pub(super) events: SyncSender<EngineEvent>,
    pub(super) reaper: Option<SyncSender<Garbage>>,
    pub(super) shared: Arc<Shared>,
    pub(super) transport: Transport,
    /// Pause and resume ramp.
    pub(super) fade: Fade,
    pub(super) declick_frames: u32,
    pub(super) pause_frames: u32,
    pub(super) volume: f32,
    pub(super) volume_target: f32,
    pub(super) primary: Option<Deck>,
    pub(super) blend: Option<Blend>,
    /// The blend after the one under way.
    pub(super) queued: Option<Blend>,
    /// Frames ever rendered, silence included.
    pub(super) clock: u64,
    pub(super) latency: u64,
    pub(super) timeline: Timeline,
    pub(super) scratch: Vec<f32>,
    pub(super) eq: Equalizer,
    pub(super) compressor: Compressor,
    pub(super) tap: Arc<TapRing>,
    /// The loaded track and those chained since, newest last.
    pub(super) remembered: VecDeque<TrackSpec>,
    /// What follows the current track, and the overlap when it blends in.
    pub(super) chained: Option<(TrackSpec, Option<u64>)>,
}

impl Engine {
    /// An engine, the handle that drives it, and the channel its events come
    /// out on.
    pub fn new(config: EngineConfig) -> (Engine, EngineHandle, Receiver<EngineEvent>) {
        assert!(
            config.sample_rate > 0 && config.channels > 0,
            "a device has a rate and channels"
        );
        let (commands_tx, commands) = mpsc::sync_channel(64);
        let (events, events_rx) = mpsc::sync_channel(1024);
        let shared = Arc::new(Shared {
            position_ms: AtomicU64::new(0),
            current: AtomicU64::new(0),
            transport: AtomicU8::new(0),
        });
        let tap = Arc::new(TapRing::new(config.sample_rate));
        let engine = Engine {
            config,
            commands,
            events,
            reaper: spawn_reaper(config.decode),
            shared: Arc::clone(&shared),
            transport: Transport::Stopped,
            fade: Fade::full(
                frames_of(DECLICK_SECONDS, config.sample_rate),
                FadeShape::Linear,
            ),
            declick_frames: frames_of(DECLICK_SECONDS, config.sample_rate),
            pause_frames: frames_of(PAUSE_FADE_SECONDS, config.sample_rate),
            volume: 1.0,
            volume_target: 1.0,
            primary: None,
            blend: None,
            queued: None,
            clock: 0,
            latency: 0,
            timeline: Timeline::new(),
            scratch: vec![0.0; BLOCK * config.channels],
            eq: Equalizer::new(config.sample_rate, config.channels),
            compressor: Compressor::new(config.sample_rate, config.channels),
            tap: Arc::clone(&tap),
            remembered: VecDeque::with_capacity(REMEMBERED),
            chained: None,
        };
        let handle = EngineHandle {
            config,
            commands: commands_tx,
            shared,
            tap,
        };
        (engine, handle, events_rx)
    }

    pub fn config(&self) -> EngineConfig {
        self.config
    }

    /// How many frames written now will take to be heard. The output sets
    /// this from the device's timestamps before each render.
    pub fn set_output_latency_frames(&mut self, frames: u64) {
        self.latency = frames;
    }

    pub fn transport(&self) -> Transport {
        self.transport
    }

    pub(crate) fn event_sender(&self) -> SyncSender<EngineEvent> {
        self.events.clone()
    }

    /// Fills `out` (interleaved, the configured channel count) with the next
    /// frames. Silence when nothing is playing.
    pub fn render(&mut self, out: &mut [f32]) {
        while let Ok(command) = self.commands.try_recv() {
            self.apply(command);
        }
        let channels = self.config.channels;
        for chunk in out.chunks_mut(BLOCK * channels) {
            self.render_block(chunk);
        }
        self.publish();
    }

    fn render_block(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let frames = out.len() / self.config.channels;

        // Decks run while playing, and while a pause is still fading out:
        // to the fade's last frame and no further, so a resume carries on
        // from the first frame that was not heard.
        let fading_out = self.transport == Transport::Paused && !self.fade.is_silent();
        let running =
            self.primary.is_some() && (self.transport == Transport::Playing || fading_out);
        let deck_frames = if fading_out {
            frames.min(self.fade.at as usize)
        } else {
            frames
        };
        if running {
            let mut done = 0;
            while done < deck_frames {
                match self.render_span(out, done, deck_frames - done) {
                    Some(advanced) => done += advanced,
                    None => break,
                }
            }
        }

        // The equaliser and the compressor on the mix, before the fades and
        // the volume, so neither changes what they do.
        self.eq.process(out);
        self.compressor.process(out);
        self.fade_and_level(out);
        self.clock += frames as u64;
        if fading_out && self.fade.is_silent() {
            // Paused: the position holds where the decks stopped.
            self.anchor_here(false);
        }
    }

    /// The pause and resume fade, then the master volume, per frame; the
    /// visualizer's copy is taken between the two.
    fn fade_and_level(&mut self, out: &mut [f32]) {
        let channels = self.config.channels;
        let frames = out.len() / channels;
        let fading_in = self.transport != Transport::Paused;
        let volume_step = (self.volume_target - self.volume) / frames.max(1) as f32;
        let mono = 1.0 / channels as f32;
        for (index, frame) in out.chunks_exact_mut(channels).enumerate() {
            let fade = self.fade.step(fading_in);
            // The visualizer's copy: faded, before the volume.
            let sum: f32 = frame.iter().sum();
            self.tap.write(self.clock + index as u64, sum * mono * fade);
            self.volume += volume_step;
            let gain = fade * self.volume;
            if gain != 1.0 {
                for sample in frame {
                    *sample *= gain;
                }
            }
        }
        self.volume = self.volume_target;
    }

    /// Renders up to `frames` frames at `offset` into `out`, stopping where a
    /// blend starts or ends so each lands on its exact frame. `None` means the
    /// rest of the block is silence.
    fn render_span(&mut self, out: &mut [f32], offset: usize, frames: usize) -> Option<usize> {
        if self
            .blend
            .as_ref()
            .is_some_and(|blend| blend.progress.is_some())
        {
            return Some(self.render_blend(out, offset, frames));
        }

        let channels = self.config.channels;
        let clock = self.clock + offset as u64;
        let primary = self.primary.as_mut()?;
        let mut limit = frames;
        if let Some(blend) = &self.blend
            && let Some(remaining) = primary.remaining()
        {
            limit = limit.min(remaining.saturating_sub(blend.overlap) as usize);
        }

        let mut written = 0;
        if limit > 0 && !primary.finished {
            let region = &mut out[offset * channels..(offset + limit) * channels];
            written = pull(
                primary,
                region,
                channels,
                &mut self.timeline,
                clock,
                true,
                self.config.sample_rate,
                &self.events,
            );
        }

        if primary.finished {
            return self.primary_finished(clock + written as u64, written);
        }
        if let Some(blend) = self.blend.as_mut()
            && let Some(remaining) = primary.remaining()
            && remaining <= blend.overlap
            && written == limit
        {
            // Never longer than what is left of the outgoing track.
            blend.overlap = remaining.max(1);
            blend.progress = Some(0);
            return Some(written);
        }
        if written < limit {
            return None; // starved
        }
        Some(written)
    }

    /// The primary deck has run out, `written` frames into the span, at
    /// `clock`. A blend waiting to start (the track's length was not known,
    /// or was shorter than said) simply follows; with none, playback ends.
    fn primary_finished(&mut self, clock: u64, written: usize) -> Option<usize> {
        match self.blend.take() {
            Some(blend) => {
                if let Some(old) = self.primary.replace(blend.deck) {
                    self.reap(old.feed);
                }
                Some(written)
            }
            None => {
                self.finish(clock);
                None
            }
        }
    }

    fn finish(&mut self, clock: u64) {
        if let Some(deck) = self.primary.take() {
            self.reap(deck.feed);
        }
        self.transport = Transport::Stopped;
        self.timeline.anchor(Anchor {
            clock,
            id: None,
            ms: 0.0,
            playing: false,
        });
        self.timeline
            .event(clock, EngineEvent::Finished, &self.events);
    }
}
