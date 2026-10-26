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

mod blend;
mod commands;
mod deck;
mod fade;
mod handle;
mod reaper;
mod render;
mod resume;
mod timeline;

pub use handle::EngineHandle;
pub use render::Engine;

use crate::error::Result;
use crate::pipeline::{Feed, InlineFeed, ThreadFeed, TrackSpec};
use crate::queue::EntryId;

/// Frames rendered per inner step; any caller buffer is split into these.
const BLOCK: usize = 4096;
/// A skip or a seek while playing fades the new track in over this long, so
/// the jump does not click.
const DECLICK_SECONDS: f64 = 0.010;
/// A pause fades out, and a resume fades in, over this long, on equal-power
/// curves: the music eases off rather than stopping dead.
pub const PAUSE_FADE_SECONDS: f64 = 0.150;
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

/// What an engine renders: the device's format, and where decoding happens.
#[derive(Clone, Copy, Debug)]
pub struct EngineConfig {
    /// Frames per second, in hertz; tracks at other rates are resampled to it.
    pub sample_rate: u32,
    /// Interleaved channels per frame; tracks are mapped to it.
    pub channels: usize,
    /// Where tracks are decoded.
    pub decode: DecodeMode,
}

/// What the engine says about what is heard.
#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    /// A track's first frame has been heard.
    TrackStarted {
        /// The track's queue entry.
        id: EntryId,
    },
    /// A track's last frame has been heard, or a blend took it out.
    TrackEnded {
        /// The track's queue entry.
        id: EntryId,
    },
    /// The heard position in the current track, in milliseconds into the file.
    Position {
        /// The track's queue entry.
        id: EntryId,
        /// Milliseconds into the file.
        ms: u64,
    },
    /// A track could not be opened or decoded; playback moves on.
    Error {
        /// The track's queue entry.
        id: EntryId,
        /// What went wrong, for the listener; never a stream's URL.
        message: String,
    },
    /// Nothing more to play.
    Finished,
    /// The output device reported a problem (an underrun, a lost device).
    DeviceError {
        /// What the device or backend said.
        message: String,
    },
    /// The output stream failed and has been opened again; playback carries
    /// on from what was last heard.
    DeviceRestored,
}

/// What the transport is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// Nothing to play: never loaded, stopped, or the last track ended.
    Stopped,
    /// Rendering sound.
    Playing,
    /// Holding a track at a position (or fading out to it).
    Paused,
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

fn frames_of(seconds: f64, rate: u32) -> u32 {
    ((seconds * f64::from(rate)).round() as u32).max(1)
}
