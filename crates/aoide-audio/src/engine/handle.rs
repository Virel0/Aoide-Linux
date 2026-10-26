//! The control side: commands into the engine, and what it has published.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;

use super::commands::{Command, NextPlan};
use super::{EngineConfig, Transport, make_feed};
use crate::compressor::CompressorSettings;
use crate::eq::{EqCurve, EqParams};
use crate::error::{Error, Result};
use crate::mix::MixTransition;
use crate::pipeline::{Feed, TrackSpec};
use crate::queue::EntryId;
use crate::visualizer::{TapRing, VisualizerTap};

/// State the handle can read without a round trip.
pub(super) struct Shared {
    pub(super) position_ms: AtomicU64,
    /// Zero for none.
    pub(super) current: AtomicU64,
    pub(super) transport: AtomicU8,
}

/// The control side. Cheap to clone; every method returns at once.
#[derive(Clone)]
pub struct EngineHandle {
    pub(super) config: EngineConfig,
    pub(super) commands: SyncSender<Command>,
    pub(super) shared: Arc<Shared>,
    pub(super) tap: Arc<TapRing>,
}

impl EngineHandle {
    /// The rate, channel count and decode mode the engine renders at.
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
    ///
    /// # Errors
    ///
    /// [`Error::Output`] when the engine has been dropped, or (threaded decoding) no
    /// decode thread can be started. A track that fails to open is not an error here: it
    /// arrives as an [`EngineEvent::Error`](super::EngineEvent::Error).
    pub fn load(&self, spec: TrackSpec, start_ms: u64, play: bool) -> Result<()> {
        let feed = self.feed(spec.clone(), start_ms)?;
        self.send(Command::Load { feed, spec, play })
    }

    /// What follows the current track, and how. `Gapless` and `Cut` join it to
    /// the current track's last frame; `Blend` overlaps the two with
    /// equal-power curves. `None` clears it.
    ///
    /// # Errors
    ///
    /// As [`Self::load`].
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

    /// Plays what is loaded, fading in from wherever a pause left it. Nothing loaded,
    /// nothing happens.
    ///
    /// # Errors
    ///
    /// [`Error::Output`] when the engine has been dropped.
    pub fn play(&self) -> Result<()> {
        self.send(Command::Play)
    }

    /// Pauses after a short fade ([`super::PAUSE_FADE_SECONDS`]); the position holds
    /// where the sound stopped.
    ///
    /// # Errors
    ///
    /// [`Error::Output`] when the engine has been dropped.
    pub fn pause(&self) -> Result<()> {
        self.send(Command::Pause)
    }

    /// Drops the current and chained tracks; silence until the next load.
    ///
    /// # Errors
    ///
    /// [`Error::Output`] when the engine has been dropped.
    pub fn stop(&self) -> Result<()> {
        self.send(Command::Stop)
    }

    /// Master volume, 0 to 1, ramped over one block.
    ///
    /// # Errors
    ///
    /// [`Error::Output`] when the engine has been dropped.
    pub fn set_volume(&self, volume: f32) -> Result<()> {
        self.send(Command::Volume(volume.clamp(0.0, 1.0)))
    }

    /// The equaliser's curve, or `None` to turn it off. The change glides in
    /// over about 15 ms, so moving a slider never clicks; off glides to flat
    /// and then the equaliser is not run at all. Refuses a curve of more than
    /// [`crate::eq::MAX_BANDS`] bands or one with a number that is not one;
    /// values out of range are held to it.
    ///
    /// # Errors
    ///
    /// [`Error::Settings`] for a curve it refuses, or [`Error::Output`] when the engine
    /// has been dropped.
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
    ///
    /// # Errors
    ///
    /// [`Error::Settings`] for a value that is not a number, or [`Error::Output`] when the
    /// engine has been dropped.
    pub fn set_compressor(&self, settings: Option<&CompressorSettings>) -> Result<()> {
        let settings = settings.map(CompressorSettings::validated).transpose()?;
        self.send(Command::Compressor(settings))
    }

    /// What a [`crate::visualizer::Visualizer`] reads: every rendered frame,
    /// mono, aligned to the heard position.
    pub fn visualizer_tap(&self) -> VisualizerTap {
        VisualizerTap::new(Arc::clone(&self.tap))
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

    /// Playing, paused or stopped, as of the last render.
    pub fn transport(&self) -> Transport {
        match self.shared.transport.load(Ordering::Relaxed) {
            1 => Transport::Playing,
            2 => Transport::Paused,
            _ => Transport::Stopped,
        }
    }
}
