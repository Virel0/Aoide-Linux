//! Playback for Aoide on Linux: decoding, gapless joins, crossfade, loudness
//! levelling and the queue, rendered by a pull-model core that the output
//! device's callback (or a test) calls for frames.
//!
//! - [`Engine`] renders f32 frames; [`EngineHandle`] drives it.
//! - [`Player`] adds the queue, the crossfade planner and the transport rules.
//! - [`Output`] runs an engine on a cpal device, the default or a chosen one
//!   ([`output_devices`], [`DeviceChoice`]).
//! - [`eq`] and [`compressor`] are what the engine runs on the mix, in that
//!   order, before the pause fade and the volume.
//! - [`visualizer`] reads what is being heard, for a spectrum and a waveform.
//! - [`mix`], [`loudness`] and [`queue`] are the rules shared with the iOS and
//!   desktop apps, held to the same parity tables.

#![warn(missing_docs)]
#![warn(clippy::missing_errors_doc)]

pub mod compressor;
pub mod decode;
pub mod engine;
pub mod eq;
pub mod error;
pub mod http;
pub mod loudness;
pub mod mix;
pub mod output;
mod pipeline;
pub mod player;
pub mod queue;
mod resample;
pub mod visualizer;

pub use compressor::{CompressorPreset, CompressorSettings};
pub use decode::MediaSource;
pub use engine::{DecodeMode, Engine, EngineConfig, EngineEvent, EngineHandle, Transport};
pub use eq::{BandKind, EqBand, EqCurve, EqPreset};
pub use error::{Error, Result};
pub use mix::{MixTrack, MixTransition, SoundBounds};
pub use output::{
    CpalOutput, DeviceChoice, DeviceFormat, OpenStream, Output, OutputDevice, Render, StreamReport,
    output_devices,
};
pub use pipeline::TrackSpec;
pub use player::{Player, PlayerSettings, PlayerTrack};
pub use queue::{EntryId, PlayQueue, RepeatMode};
pub use visualizer::{VisualFrame, Visualizer, VisualizerConfig, VisualizerTap};
