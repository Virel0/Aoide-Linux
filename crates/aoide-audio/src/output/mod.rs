//! The audio device: cpal's default output (ALSA, or PipeWire through its ALSA
//! plugin, on Linux), calling the engine from its callback.
//!
//! The output outlives its stream. When the stream fails for good (the device
//! went away, or the backend invalidated the stream), it opens the default
//! device again, which is the new one if the default changed, at the engine's
//! rate and channel count, and the engine carries on from what was last heard
//! with its queue intact. Where a backend follows a change of default device
//! by itself, cpal reports that as a route change and the stream plays on
//! (CoreAudio, WASAPI; on Linux, ALSA's default device is PipeWire's or
//! PulseAudio's, which move the stream themselves); elsewhere the stream
//! fails with the device not available, and that is when this reopens.
//!
//! A device other than the default can be chosen ([`DeviceChoice`], from
//! [`output_devices`]), at the start or while playing
//! ([`Output::set_device`]). Switching keeps what was heard: the engine plays
//! on from the heard position on the new device, queue intact. A chosen
//! device that cannot be opened, at the start, on a switch, or when it fails
//! and is opened again, gives way to the default device, with a
//! [`EngineEvent::DeviceError`] saying so.
//!
//! Opening a stream is behind [`OpenStream`], so the recovery and the
//! switching are tested without a device.

mod backend;
mod supervisor;

use std::any::Any;
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

pub use backend::{CpalOutput, output_devices};
pub use supervisor::Output;

use crate::engine::{Engine, EngineEvent};
use crate::error::{Error, Result};

/// A device's rate and channel count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceFormat {
    pub sample_rate: u32,
    pub channels: usize,
}

/// One output device, as [`output_devices`] lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputDevice {
    /// cpal's stable id, `host:device` (`alsa:front:CARD=…`, say): stable
    /// across runs and reboots where the backend allows.
    pub id: String,
    /// What to show.
    pub name: String,
    /// Whether it is the default device now.
    pub is_default: bool,
}

/// Which device to play on: plain data, for the config file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum DeviceChoice {
    /// Whatever the system's default output is, following it when it
    /// changes.
    #[default]
    FollowDefault,
    /// One device: found by its id, or by its name when the id has changed
    /// (an ALSA card renumbered, say).
    Device { id: String, name: String },
}

impl From<&OutputDevice> for DeviceChoice {
    fn from(device: &OutputDevice) -> Self {
        DeviceChoice::Device {
            id: device.id.clone(),
            name: device.name.clone(),
        }
    }
}

/// The step that opens an output stream.
pub trait OpenStream: Send + 'static {
    /// The default device's own rate and channel count, which the engine is
    /// made for.
    fn default_format(&mut self) -> Result<DeviceFormat>;

    /// Opens the default device at `format` and starts it. Its callback calls
    /// `render`; it tells `report` when the stream fails. The stream stops
    /// when the returned value is dropped.
    fn open(
        &mut self,
        format: DeviceFormat,
        render: Render,
        report: StreamReport,
    ) -> Result<Box<dyn Any>>;

    /// A chosen device's own rate and channel count. Only the default exists
    /// unless this is implemented.
    fn device_format(&mut self, device: &DeviceChoice) -> Result<DeviceFormat> {
        match device {
            DeviceChoice::FollowDefault => self.default_format(),
            DeviceChoice::Device { name, .. } => Err(no_such_device(name)),
        }
    }

    /// Opens a chosen device, as [`OpenStream::open`] opens the default.
    /// Only the default exists unless this is implemented.
    fn open_device(
        &mut self,
        device: &DeviceChoice,
        format: DeviceFormat,
        render: Render,
        report: StreamReport,
    ) -> Result<Box<dyn Any>> {
        match device {
            DeviceChoice::FollowDefault => self.open(format, render, report),
            DeviceChoice::Device { name, .. } => Err(no_such_device(name)),
        }
    }
}

pub(super) fn no_such_device(name: &str) -> Error {
    Error::Output(format!("no output device called {name}"))
}

/// What a stream's callback calls for frames.
pub struct Render {
    pub(super) engine: Arc<Mutex<Engine>>,
}

impl Render {
    /// Fills `out` (interleaved) with the engine's next frames; they will be
    /// heard `latency_frames` from now. Never blocks: while the engine is
    /// being moved to a new stream, this is silence.
    pub fn render(&mut self, out: &mut [f32], latency_frames: u64) {
        match self.engine.try_lock() {
            Ok(mut engine) => {
                engine.set_output_latency_frames(latency_frames);
                engine.render(out);
            }
            Err(_) => out.fill(0.0),
        }
    }
}

/// How a stream says something went wrong. Never blocks.
#[derive(Clone)]
pub struct StreamReport {
    pub(super) generation: u64,
    pub(super) signals: SyncSender<Signal>,
    pub(super) events: SyncSender<EngineEvent>,
}

impl StreamReport {
    /// The stream has stopped for good: the output opens the device again.
    pub fn failed(&self, message: String) {
        let _ = self.signals.try_send(Signal::Failed {
            generation: self.generation,
            message,
        });
    }

    /// A problem the stream lives through (an underrun, a route change it
    /// followed): reported as a [`EngineEvent::DeviceError`], nothing reopened.
    pub fn glitch(&self, message: String) {
        let _ = self.events.try_send(EngineEvent::DeviceError { message });
    }
}

pub(super) enum Signal {
    Failed { generation: u64, message: String },
    Switch(DeviceChoice),
    Shutdown,
}
