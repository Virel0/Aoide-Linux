//! The render core: f32 frames into a caller's buffer.
//!
//! The output device's callback calls [`Engine::render`]; offline tests call it
//! directly. The engine never touches a device, so everything it does can be
//! measured without one. It is driven through an [`EngineHandle`] whose
//! commands are applied at the start of the next render, and it reports what
//! the listener *hears* (the rendered position minus the output latency) on an
//! event channel.
//!
//! Inside a render nothing blocks: commands arrive on a bounded channel read
//! with `try_recv`, events leave with `try_send`, decoded audio comes from
//! lock-free rings (in [`DecodeMode::Threaded`]), and feeds that are done with
//! are handed to a reaper thread rather than freed here.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread;

use crate::compressor::{Compressor, CompressorSettings};
use crate::eq::{EqCurve, EqParams, Equalizer};
use crate::error::{Error, Result};
use crate::mix::{MixTransition, curve};
use crate::pipeline::{Feed, FeedRead, InlineFeed, Marker, ThreadFeed, TrackSpec};
use crate::queue::EntryId;

/// Frames rendered per inner step; any caller buffer is split into these.
const BLOCK: usize = 4096;
/// A pause, a resume or a skip while playing is faded over this long, so it
/// does not click.
const DECLICK_SECONDS: f32 = 0.010;
/// How often a position event is sent, in heard time.
const POSITION_EVERY_MS: f64 = 250.0;
const ANCHORS: usize = 32;
const PENDING_EVENTS: usize = 64;
/// Tracks the engine remembers, to open one again after the output lost
/// what it had buffered: the loaded one and those chained since.
const REMEMBERED: usize = 4;

/// Where decoding happens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeMode {
    /// Each track decodes on its own thread ahead of the speaker. What a
    /// device callback needs.
    Threaded,
    /// Decoding happens inside `render`, on demand. Deterministic: what
    /// offline rendering and tests use.
    Inline,
}

#[derive(Clone, Copy, Debug)]
pub struct EngineConfig {
    pub sample_rate: u32,
    pub channels: usize,
    pub decode: DecodeMode,
}

/// What the engine says about what is heard.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    TrackStarted {
        id: EntryId,
    },
    TrackEnded {
        id: EntryId,
    },
    /// The heard position in the current track, in milliseconds into the file.
    Position {
        id: EntryId,
        ms: u64,
    },
    Error {
        id: EntryId,
        message: String,
    },
    /// Nothing more to play.
    Finished,
    /// The output device reported a problem (an underrun, a lost device).
    DeviceError {
        message: String,
    },
    /// The output stream failed and has been opened again; playback carries
    /// on from what was last heard.
    DeviceRestored,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Stopped,
    Playing,
    Paused,
}

enum Command {
    Load {
        feed: Box<dyn Feed>,
        spec: TrackSpec,
        play: bool,
    },
    SetNext(Option<NextPlan>),
    Play,
    Pause,
    Stop,
    Volume(f32),
    Equalizer(EqParams),
    Compressor(Option<CompressorSettings>),
}

struct NextPlan {
    spec: TrackSpec,
    blend: Option<(Box<dyn Feed>, u64)>,
}

/// State the handle can read without a round trip.
struct Shared {
    position_ms: AtomicU64,
    /// Zero for none.
    current: AtomicU64,
    transport: AtomicU8,
}

/// The control side. Cheap to clone; every method returns at once.
#[derive(Clone)]
pub struct EngineHandle {
    config: EngineConfig,
    commands: SyncSender<Command>,
    shared: Arc<Shared>,
}

impl EngineHandle {
    pub fn config(&self) -> EngineConfig {
        self.config
    }

    fn feed(&self, spec: TrackSpec, start_ms: u64) -> Result<Box<dyn Feed>> {
        make_feed(self.config, spec, start_ms)
    }

    fn send(&self, command: Command) -> Result<()> {
        self.commands
            .send(command)
            .map_err(|_| Error::Output("the engine has gone".into()))
    }

    /// Replaces whatever is playing with `spec`, from `start_ms` into the file
    /// (a seek is a load at the new position). Anything chained is dropped.
    pub fn load(&self, spec: TrackSpec, start_ms: u64, play: bool) -> Result<()> {
        let feed = self.feed(spec.clone(), start_ms)?;
        self.send(Command::Load { feed, spec, play })
    }

    /// What follows the current track, and how. `Gapless` and `Cut` join it to
    /// the current track's last frame; `Blend` overlaps the two with
    /// equal-power curves. `None` clears it.
    pub fn set_next(&self, next: Option<(TrackSpec, MixTransition)>) -> Result<()> {
        let plan = match next {
            None => None,
            Some((spec, MixTransition::Blend { seconds })) if seconds > 0.0 => {
                let feed = self.feed(spec.clone(), 0)?;
                let frames = (seconds * f64::from(self.config.sample_rate)).round() as u64;
                Some(NextPlan {
                    spec,
                    blend: Some((feed, frames)),
                })
            }
            Some((spec, _)) => Some(NextPlan { spec, blend: None }),
        };
        self.send(Command::SetNext(plan))
    }

    pub fn play(&self) -> Result<()> {
        self.send(Command::Play)
    }

    pub fn pause(&self) -> Result<()> {
        self.send(Command::Pause)
    }

    pub fn stop(&self) -> Result<()> {
        self.send(Command::Stop)
    }

    /// Master volume, 0 to 1, ramped over one block.
    pub fn set_volume(&self, volume: f32) -> Result<()> {
        self.send(Command::Volume(volume.clamp(0.0, 1.0)))
    }

    /// The equaliser's curve, or `None` to turn it off. The change glides in
    /// over about 15 ms, so moving a slider never clicks; off glides to flat
    /// and then the equaliser is not run at all. Refuses a curve of more than
    /// [`crate::eq::MAX_BANDS`] bands or one with a number that is not one;
    /// values out of range are held to it.
    pub fn set_equalizer(&self, curve: Option<&EqCurve>) -> Result<()> {
        let params = match curve {
            Some(curve) => EqParams::from_curve(curve)?,
            None => EqParams::flat(),
        };
        self.send(Command::Equalizer(params))
    }

    /// The compressor's settings, or `None` to turn it off. It runs after
    /// the equaliser. A change of threshold, ratio or knee moves the target
    /// reduction, which the attack and release smooth; makeup glides over
    /// about 15 ms. Off lets the reduction release and is then not run at
    /// all. Refuses a setting that is not a number; values out of range are
    /// held to it.
    pub fn set_compressor(&self, settings: Option<&CompressorSettings>) -> Result<()> {
        let settings = settings.map(CompressorSettings::validated).transpose()?;
        self.send(Command::Compressor(settings))
    }

    /// The heard position in the current track, in milliseconds.
    pub fn position_ms(&self) -> u64 {
        self.shared.position_ms.load(Ordering::Relaxed)
    }

    /// The track being heard.
    pub fn current(&self) -> Option<EntryId> {
        match self.shared.current.load(Ordering::Relaxed) {
            0 => None,
            id => Some(EntryId(id)),
        }
    }

    pub fn transport(&self) -> Transport {
        match self.shared.transport.load(Ordering::Relaxed) {
            1 => Transport::Playing,
            2 => Transport::Paused,
            _ => Transport::Stopped,
        }
    }
}

fn make_feed(config: EngineConfig, spec: TrackSpec, start_ms: u64) -> Result<Box<dyn Feed>> {
    let EngineConfig {
        sample_rate,
        channels,
        decode,
    } = config;
    Ok(match decode {
        DecodeMode::Inline => Box::new(InlineFeed::new(sample_rate, channels, spec, start_ms)),
        DecodeMode::Threaded => Box::new(ThreadFeed::spawn(sample_rate, channels, spec, start_ms)?),
    })
}

/// What the reaper thread frees, so the render never does.
enum Garbage {
    Feed(#[allow(dead_code)] Box<dyn Feed>),
    Spec(#[allow(dead_code)] TrackSpec),
}

struct DeckTrack {
    id: EntryId,
    gain: f32,
    origin_ms: f64,
    played: u64,
    length: Option<u64>,
}

struct Deck {
    feed: Box<dyn Feed>,
    track: Option<DeckTrack>,
    finished: bool,
    starved: bool,
}

impl Deck {
    fn new(feed: Box<dyn Feed>) -> Self {
        Self {
            feed,
            track: None,
            finished: false,
            starved: false,
        }
    }

    /// Output frames left in the current track, when its length is known.
    fn remaining(&self) -> Option<u64> {
        let track = self.track.as_ref()?;
        Some(track.length?.saturating_sub(track.played))
    }
}

struct Blend {
    deck: Deck,
    overlap: u64,
    /// Frames into the overlap; `None` until it starts.
    progress: Option<u64>,
}

#[derive(Clone, Copy)]
struct Anchor {
    clock: u64,
    id: Option<EntryId>,
    ms: f64,
    playing: bool,
}

/// What has been rendered, mapped to what will be heard once the output's
/// latency has passed.
struct Timeline {
    anchors: [Anchor; ANCHORS],
    anchor_count: usize,
    next_anchor: usize,
    events: Vec<(u64, EngineEvent)>,
    last_position_ms: f64,
    last_position_id: Option<EntryId>,
}

impl Timeline {
    fn new() -> Self {
        Self {
            anchors: [Anchor {
                clock: 0,
                id: None,
                ms: 0.0,
                playing: false,
            }; ANCHORS],
            anchor_count: 0,
            next_anchor: 0,
            events: Vec::with_capacity(PENDING_EVENTS),
            last_position_ms: f64::NEG_INFINITY,
            last_position_id: None,
        }
    }

    fn anchor(&mut self, anchor: Anchor) {
        self.anchors[self.next_anchor] = anchor;
        self.next_anchor = (self.next_anchor + 1) % ANCHORS;
        self.anchor_count = (self.anchor_count + 1).min(ANCHORS);
    }

    fn event(&mut self, clock: u64, event: EngineEvent, out: &SyncSender<EngineEvent>) {
        if self.events.len() == self.events.capacity() {
            // Too many in one latency window: send the oldest now.
            let (_, oldest) = self.events.remove(0);
            let _ = out.try_send(oldest);
        }
        self.events.push((clock, event));
    }

    /// The anchor in force at `clock`.
    fn at(&self, clock: u64) -> Option<Anchor> {
        (0..self.anchor_count)
            .map(|back| self.anchors[(self.next_anchor + ANCHORS - 1 - back) % ANCHORS])
            .find(|anchor| anchor.clock <= clock)
    }
}

pub struct Engine {
    config: EngineConfig,
    commands: Receiver<Command>,
    events: SyncSender<EngineEvent>,
    reaper: Option<SyncSender<Garbage>>,
    shared: Arc<Shared>,
    transport: Transport,
    /// Pause and resume ramp, 0 to 1.
    fade: f32,
    fade_step: f32,
    volume: f32,
    volume_target: f32,
    primary: Option<Deck>,
    blend: Option<Blend>,
    /// The blend after the one under way.
    queued: Option<Blend>,
    /// Frames ever rendered, silence included.
    clock: u64,
    latency: u64,
    timeline: Timeline,
    scratch: Vec<f32>,
    eq: Equalizer,
    compressor: Compressor,
    /// The loaded track and those chained since, newest last.
    remembered: VecDeque<TrackSpec>,
    /// What follows the current track, and the overlap when it blends in.
    chained: Option<(TrackSpec, Option<u64>)>,
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
        let reaper = match config.decode {
            DecodeMode::Inline => None,
            DecodeMode::Threaded => {
                let (tx, rx) = mpsc::sync_channel::<Garbage>(16);
                thread::Builder::new()
                    .name("aoide-reaper".into())
                    .spawn(move || while rx.recv().is_ok() {})
                    .ok()
                    .map(|_| tx)
            }
        };
        let engine = Engine {
            config,
            commands,
            events,
            reaper,
            shared: Arc::clone(&shared),
            transport: Transport::Stopped,
            fade: 1.0,
            fade_step: 1.0 / (DECLICK_SECONDS * config.sample_rate as f32),
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
            remembered: VecDeque::with_capacity(REMEMBERED),
            chained: None,
        };
        let handle = EngineHandle {
            config,
            commands: commands_tx,
            shared,
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

    fn apply(&mut self, command: Command) {
        match command {
            Command::Load { feed, spec, play } => {
                let audible = self.transport == Transport::Playing && self.primary.is_some();
                self.drop_decks();
                self.primary = Some(Deck::new(feed));
                while let Some(old) = self.remembered.pop_front() {
                    self.reap_spec(old);
                }
                self.remember(spec);
                if let Some((old, _)) = self.chained.take() {
                    self.reap_spec(old);
                }
                if play {
                    self.transport = Transport::Playing;
                    // Replacing something audible fades the new one in;
                    // starting from silence is sample-exact from frame one.
                    self.fade = if audible { 0.0 } else { 1.0 };
                } else {
                    // Loaded and held: nothing sounds until play.
                    self.transport = Transport::Paused;
                    self.fade = 0.0;
                }
            }
            Command::SetNext(plan) => {
                let chained = plan.as_ref().map(|plan| {
                    let overlap = plan.blend.as_ref().map(|(_, overlap)| (*overlap).max(1));
                    (plan.spec.clone(), overlap)
                });
                if let Some((spec, _)) = &chained {
                    self.remember(spec.clone());
                }
                if let Some((old, _)) = std::mem::replace(&mut self.chained, chained) {
                    self.reap_spec(old);
                }
                // A blend not yet started is replaced; one under way plays
                // out, and what follows is chained to its incoming track.
                if let Some(blend) = self.blend.take_if(|blend| blend.progress.is_none()) {
                    self.reap(blend.deck.feed);
                }
                if let Some(queued) = self.queued.take() {
                    self.reap(queued.deck.feed);
                }
                let blending = self.blend.is_some();
                let target = match self.blend.as_mut() {
                    Some(blend) => Some(&mut blend.deck),
                    None => self.primary.as_mut(),
                };
                let Some(target) = target else {
                    if let Some(NextPlan {
                        blend: Some((feed, _)),
                        ..
                    }) = plan
                    {
                        self.reap(feed);
                    }
                    return;
                };
                let next = match plan {
                    None => {
                        target.feed.chain(None);
                        None
                    }
                    Some(NextPlan { spec, blend: None }) => {
                        target.feed.chain(Some(spec));
                        None
                    }
                    Some(NextPlan {
                        blend: Some((feed, overlap)),
                        ..
                    }) => {
                        target.feed.chain(None);
                        Some(Blend {
                            deck: Deck::new(feed),
                            overlap: overlap.max(1),
                            progress: None,
                        })
                    }
                };
                if blending {
                    self.queued = next;
                } else {
                    self.blend = next;
                }
            }
            Command::Play => {
                if self.primary.is_some() && self.transport != Transport::Playing {
                    self.transport = Transport::Playing;
                    self.anchor_here(true);
                }
            }
            Command::Pause => {
                if self.transport == Transport::Playing {
                    // The fade runs out in render, which then pauses.
                    self.transport = Transport::Paused;
                    if self.fade <= 0.0 {
                        self.anchor_here(false);
                    }
                }
            }
            Command::Stop => {
                self.drop_decks();
                self.transport = Transport::Stopped;
                self.timeline.anchor(Anchor {
                    clock: self.clock,
                    id: None,
                    ms: 0.0,
                    playing: false,
                });
            }
            Command::Volume(volume) => self.volume_target = volume,
            Command::Equalizer(params) => self.eq.set(params),
            Command::Compressor(settings) => self.compressor.set(settings),
        }
    }

    fn drop_decks(&mut self) {
        if let Some(deck) = self.primary.take() {
            self.reap(deck.feed);
        }
        if let Some(blend) = self.blend.take() {
            self.reap(blend.deck.feed);
        }
        if let Some(queued) = self.queued.take() {
            self.reap(queued.deck.feed);
        }
    }

    fn reap(&mut self, feed: Box<dyn Feed>) {
        self.discard(Garbage::Feed(feed));
    }

    fn reap_spec(&mut self, spec: TrackSpec) {
        self.discard(Garbage::Spec(spec));
    }

    fn discard(&mut self, garbage: Garbage) {
        if let Some(reaper) = &self.reaper {
            match reaper.try_send(garbage) {
                Ok(()) => {}
                Err(TrySendError::Full(garbage)) | Err(TrySendError::Disconnected(garbage)) => {
                    drop(garbage)
                }
            }
        }
    }

    /// Keeps `spec` among the last few tracks, without allocating.
    fn remember(&mut self, spec: TrackSpec) {
        if self.remembered.len() == REMEMBERED
            && let Some(old) = self.remembered.pop_front()
        {
            self.reap_spec(old);
        }
        self.remembered.push_back(spec);
    }

    /// Plays on from what was last heard rather than from what was last
    /// rendered, after the output lost what it had buffered (its stream
    /// failed): the current track is opened again at the heard position, and
    /// what was chained after it is chained again, so the queue carries on.
    /// Then the output latency is zero until the next stream reports its own.
    ///
    /// Only for a track playing on its own: during a blend, or when a track
    /// boundary had been rendered but not yet heard, the engine simply plays
    /// on from what it rendered, losing at most one buffer of sound.
    ///
    /// Not for the device callback: it opens the track again.
    pub fn resume_from_heard(&mut self) {
        let unheard = self.latency.min(self.clock);
        self.latency = 0;
        if unheard == 0
            || self.transport != Transport::Playing
            || self
                .blend
                .as_ref()
                .is_some_and(|blend| blend.progress.is_some())
        {
            return;
        }
        let heard = self.clock - unheard;
        let Some(Anchor {
            id: Some(id),
            ms,
            playing: true,
            clock,
        }) = self.timeline.at(heard)
        else {
            return;
        };
        let current = self
            .primary
            .as_ref()
            .and_then(|deck| deck.track.as_ref())
            .map(|track| track.id);
        if current != Some(id) {
            return;
        }
        let Some(spec) = self.remembered.iter().rev().find(|spec| spec.id == id) else {
            return;
        };
        let rate = f64::from(self.config.sample_rate);
        let heard_ms = ms + (heard - clock) as f64 * 1000.0 / rate;
        let Ok(mut feed) = make_feed(self.config, spec.clone(), heard_ms.round() as u64) else {
            return;
        };

        // Chained again, unless it is the track being resumed (the join to it
        // has been heard and nothing follows it yet).
        let mut blend = None;
        match self.chained.clone() {
            Some((next, _)) if next.id == id => {}
            Some((next, None)) => feed.chain(Some(next)),
            Some((next, Some(overlap))) => {
                if let Ok(next_feed) = make_feed(self.config, next, 0) {
                    blend = Some(Blend {
                        deck: Deck::new(next_feed),
                        overlap,
                        progress: None,
                    });
                }
            }
            None => {}
        }
        self.drop_decks();
        self.primary = Some(Deck::new(feed));
        self.blend = blend;
        // What had been rendered after the heard frame is rendered again.
        self.timeline.events.retain(|(at, _)| *at <= heard);
    }

    /// Records the foreground track's position at the current clock.
    fn anchor_here(&mut self, playing: bool) {
        let deck = match &self.blend {
            Some(blend) if blend.progress.is_some() => Some(&blend.deck),
            _ => self.primary.as_ref(),
        };
        let (id, ms) = match deck.and_then(|deck| deck.track.as_ref()) {
            Some(track) => (Some(track.id), position_ms(track, self.config.sample_rate)),
            None => (None, 0.0),
        };
        self.timeline.anchor(Anchor {
            clock: self.clock,
            id,
            ms,
            playing,
        });
    }

    fn render_block(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let channels = self.config.channels;
        let frames = out.len() / channels;

        // Decks run while playing, and while a pause is still fading out.
        let fading_out = self.transport == Transport::Paused && self.fade > 0.0;
        let running =
            self.primary.is_some() && (self.transport == Transport::Playing || fading_out);
        if running {
            let mut done = 0;
            while done < frames {
                match self.render_span(out, done, frames - done) {
                    Some(advanced) => done += advanced,
                    None => break,
                }
            }
        }

        // The equaliser and the compressor on the mix, before the fades and
        // the volume, so neither changes what they do.
        self.eq.process(out);
        self.compressor.process(out);

        // Pause and resume fade, then master volume, per frame.
        let fade_target = if self.transport == Transport::Paused {
            0.0
        } else {
            1.0
        };
        let volume_step = (self.volume_target - self.volume) / frames.max(1) as f32;
        for frame in out.chunks_exact_mut(channels) {
            if self.fade < fade_target {
                self.fade = (self.fade + self.fade_step).min(1.0);
            } else if self.fade > fade_target {
                self.fade = (self.fade - self.fade_step).max(0.0);
            }
            self.volume += volume_step;
            let gain = self.fade * self.volume;
            if gain != 1.0 {
                for sample in frame {
                    *sample *= gain;
                }
            }
        }
        self.volume = self.volume_target;
        self.clock += frames as u64;
        if fading_out && self.fade == 0.0 {
            // Paused: the position holds where the decks stopped.
            self.anchor_here(false);
        }
    }

    /// Renders up to `frames` frames at `offset` into `out`, stopping where a
    /// blend starts or ends so each lands on its exact frame. `None` means the
    /// rest of the block is silence.
    fn render_span(&mut self, out: &mut [f32], offset: usize, frames: usize) -> Option<usize> {
        let channels = self.config.channels;
        let rate = self.config.sample_rate;
        let clock = self.clock + offset as u64;

        if self
            .blend
            .as_ref()
            .is_some_and(|blend| blend.progress.is_some())
        {
            return Some(self.render_blend(out, offset, frames));
        }

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
                rate,
                &self.events,
            );
        }

        if primary.finished {
            match self.blend.take() {
                // Ended before the blend could start (its length was not known,
                // or was shorter than said): the next track simply follows.
                Some(blend) => {
                    if let Some(old) = self.primary.replace(blend.deck) {
                        self.reap(old.feed);
                    }
                    return Some(written);
                }
                None => {
                    self.finish(clock + written as u64);
                    return None;
                }
            }
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

    fn render_blend(&mut self, out: &mut [f32], offset: usize, frames: usize) -> usize {
        let channels = self.config.channels;
        let rate = self.config.sample_rate;
        let clock = self.clock + offset as u64;
        let blend = self.blend.as_mut().expect("a blend under way");
        let progress = blend.progress.unwrap_or(0);
        let span = frames.min((blend.overlap - progress) as usize);
        let region = &mut out[offset * channels..(offset + span) * channels];

        // The incoming track into the block, the outgoing one into scratch.
        pull(
            &mut blend.deck,
            region,
            channels,
            &mut self.timeline,
            clock,
            true,
            rate,
            &self.events,
        );
        let scratch = &mut self.scratch[..span * channels];
        scratch.fill(0.0);
        if let Some(primary) = self.primary.as_mut() {
            pull(
                primary,
                scratch,
                channels,
                &mut self.timeline,
                clock,
                false,
                rate,
                &self.events,
            );
        }
        for (index, (frame, old)) in region
            .chunks_exact_mut(channels)
            .zip(scratch.chunks_exact(channels))
            .enumerate()
        {
            let p = (progress + index as u64) as f64 / blend.overlap as f64;
            let (gain_in, gain_out) = (curve::incoming(p) as f32, curve::outgoing(p) as f32);
            for (sample, old) in frame.iter_mut().zip(old) {
                *sample = *sample * gain_in + old * gain_out;
            }
        }

        let progress = progress + span as u64;
        blend.progress = Some(progress);
        if progress >= blend.overlap {
            let blend = self.blend.take().expect("checked above");
            self.blend = self.queued.take();
            if let Some(old) = self.primary.replace(blend.deck) {
                // Its end had not been reached: it ends with the overlap.
                if let Some(track) = &old.track {
                    let ended = EngineEvent::TrackEnded { id: track.id };
                    self.timeline
                        .event(clock + span as u64, ended, &self.events);
                }
                self.reap(old.feed);
            }
        }
        span
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

    /// Sends what has become audible and updates the shared position.
    fn publish(&mut self) {
        let heard = self.clock.saturating_sub(self.latency);
        let events = &mut self.timeline.events;
        let due = events
            .iter()
            .take_while(|(clock, _)| *clock <= heard)
            .count();
        for (_, event) in events.drain(..due) {
            let _ = self.events.try_send(event);
        }

        let anchor = self.timeline.at(heard);
        let (id, ms) = match anchor {
            Some(Anchor {
                id: Some(id),
                ms,
                playing,
                clock,
            }) => {
                let elapsed = if playing {
                    (heard - clock) as f64 * 1000.0 / f64::from(self.config.sample_rate)
                } else {
                    0.0
                };
                (Some(id), ms + elapsed)
            }
            _ => (None, 0.0),
        };
        self.shared
            .current
            .store(id.map_or(0, |id| id.0), Ordering::Relaxed);
        self.shared.position_ms.store(ms as u64, Ordering::Relaxed);
        self.shared.transport.store(
            match self.transport {
                Transport::Stopped => 0,
                Transport::Playing => 1,
                Transport::Paused => 2,
            },
            Ordering::Relaxed,
        );
        if let Some(id) = id {
            let moved = self.timeline.last_position_id != Some(id)
                || (ms - self.timeline.last_position_ms).abs() >= POSITION_EVERY_MS;
            if moved {
                self.timeline.last_position_id = Some(id);
                self.timeline.last_position_ms = ms;
                let _ = self
                    .events
                    .try_send(EngineEvent::Position { id, ms: ms as u64 });
            }
        }
    }
}

fn position_ms(track: &DeckTrack, rate: u32) -> f64 {
    track.origin_ms + track.played as f64 * 1000.0 / f64::from(rate)
}

/// Reads a deck's audio into `out` (overwriting it), handling the markers met
/// on the way at their exact frame. `foreground` decks move the heard
/// position; a track fading out under a blend only reports its end. Returns
/// the frames of audio written; the rest of `out` is silence (starved or
/// finished).
#[allow(clippy::too_many_arguments)]
fn pull(
    deck: &mut Deck,
    out: &mut [f32],
    channels: usize,
    timeline: &mut Timeline,
    clock: u64,
    foreground: bool,
    rate: u32,
    events: &SyncSender<EngineEvent>,
) -> usize {
    let mut written = 0;
    let frames_total = out.len() / channels;
    while written < frames_total && !deck.finished {
        let at = clock + written as u64;
        match deck.feed.read(&mut out[written * channels..]) {
            FeedRead::Audio(frames) => {
                if deck.starved {
                    deck.starved = false;
                    if foreground && let Some(track) = &deck.track {
                        timeline.anchor(Anchor {
                            clock: at,
                            id: Some(track.id),
                            ms: position_ms(track, rate),
                            playing: true,
                        });
                    }
                }
                let region = &mut out[written * channels..(written + frames) * channels];
                match deck.track.as_mut() {
                    Some(track) => {
                        if track.gain != 1.0 {
                            for sample in region.iter_mut() {
                                *sample *= track.gain;
                            }
                        }
                        track.played += frames as u64;
                    }
                    None => region.fill(0.0),
                }
                written += frames;
            }
            FeedRead::Marker(Marker::Start {
                id,
                gain,
                origin_ms,
                length,
            }) => {
                deck.track = Some(DeckTrack {
                    id,
                    gain,
                    origin_ms,
                    played: 0,
                    length,
                });
                if foreground {
                    timeline.anchor(Anchor {
                        clock: at,
                        id: Some(id),
                        ms: origin_ms,
                        playing: true,
                    });
                }
                timeline.event(at, EngineEvent::TrackStarted { id }, events);
            }
            FeedRead::Marker(Marker::End { id }) => {
                if deck.track.as_ref().is_some_and(|track| track.id == id) {
                    deck.track = None;
                }
                timeline.event(at, EngineEvent::TrackEnded { id }, events);
            }
            FeedRead::Marker(Marker::Error { id, message }) => {
                timeline.event(at, EngineEvent::Error { id, message }, events);
            }
            FeedRead::Starved => {
                if !deck.starved {
                    deck.starved = true;
                    if foreground && let Some(track) = &deck.track {
                        timeline.anchor(Anchor {
                            clock: at,
                            id: Some(track.id),
                            ms: position_ms(track, rate),
                            playing: false,
                        });
                    }
                }
                break;
            }
            FeedRead::Finished => deck.finished = true,
        }
    }
    out[written * channels..].fill(0.0);
    written
}
