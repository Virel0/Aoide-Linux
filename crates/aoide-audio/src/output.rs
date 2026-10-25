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

use std::any::Any;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use serde::{Deserialize, Serialize};

use crate::engine::{DecodeMode, Engine, EngineConfig, EngineEvent, EngineHandle};
use crate::error::{Error, Result};

/// Waits between attempts to open a device again; the last repeats.
const RETRY: [Duration; 5] = [
    Duration::from_millis(50),
    Duration::from_millis(100),
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_millis(1_000),
];

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

fn no_such_device(name: &str) -> Error {
    Error::Output(format!("no output device called {name}"))
}

/// What a stream's callback calls for frames.
pub struct Render {
    engine: Arc<Mutex<Engine>>,
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
    generation: u64,
    signals: SyncSender<Signal>,
    events: SyncSender<EngineEvent>,
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

enum Signal {
    Failed { generation: u64, message: String },
    Switch(DeviceChoice),
    Shutdown,
}

/// A running output. Keep it alive for as long as there should be sound;
/// dropping it stops the device. The stream lives on a thread of its own,
/// which reopens it when it fails.
pub struct Output {
    config: EngineConfig,
    signals: SyncSender<Signal>,
    supervisor: Option<JoinHandle<()>>,
    choice: DeviceChoice,
    playing_on: Arc<Mutex<DeviceChoice>>,
}

type Started = (EngineHandle, Receiver<EngineEvent>, EngineConfig);

impl Output {
    /// Opens the default output device at its own rate and channel count,
    /// starts an engine on it, and returns the handle and event channel.
    pub fn start_default() -> Result<(Output, EngineHandle, Receiver<EngineEvent>)> {
        Self::start(CpalOutput, DecodeMode::Threaded)
    }

    /// The same on a chosen device (the default if it cannot be opened), at
    /// that device's own rate and channel count.
    pub fn start_on(device: DeviceChoice) -> Result<(Output, EngineHandle, Receiver<EngineEvent>)> {
        Self::start_with(CpalOutput, DecodeMode::Threaded, device)
    }

    /// The default device on any [`OpenStream`].
    pub fn start(
        backend: impl OpenStream,
        decode: DecodeMode,
    ) -> Result<(Output, EngineHandle, Receiver<EngineEvent>)> {
        Self::start_with(backend, decode, DeviceChoice::FollowDefault)
    }

    /// A chosen device on any [`OpenStream`].
    pub fn start_with(
        backend: impl OpenStream,
        decode: DecodeMode,
        device: DeviceChoice,
    ) -> Result<(Output, EngineHandle, Receiver<EngineEvent>)> {
        let (signals_tx, signals) = mpsc::sync_channel(16);
        let (ready_tx, ready) = mpsc::sync_channel(1);
        let reports = signals_tx.clone();
        let playing_on = Arc::new(Mutex::new(DeviceChoice::FollowDefault));
        let supervisor_playing_on = Arc::clone(&playing_on);
        let choice = device.clone();
        let supervisor = thread::Builder::new()
            .name("aoide-output".into())
            .spawn(move || {
                Supervisor {
                    backend,
                    reports,
                    signals,
                    choice,
                    playing_on: supervisor_playing_on,
                }
                .run(decode, ready_tx)
            })
            .map_err(|e| Error::Output(e.to_string()))?;
        match ready.recv() {
            Ok(Ok((handle, events, config))) => Ok((
                Output {
                    config,
                    signals: signals_tx,
                    supervisor: Some(supervisor),
                    choice: device,
                    playing_on,
                },
                handle,
                events,
            )),
            Ok(Err(error)) => {
                let _ = supervisor.join();
                Err(error)
            }
            Err(_) => Err(Error::Output("the output thread stopped".into())),
        }
    }

    pub fn config(&self) -> EngineConfig {
        self.config
    }

    /// Moves playback to `device`, carrying on from what was heard. Returns
    /// at once; the switch happens on the output's thread. A device that
    /// cannot be opened (gone, or unable to play the engine's rate and
    /// channel count) gives way to the default, with an
    /// [`EngineEvent::DeviceError`] saying so.
    pub fn set_device(&mut self, device: DeviceChoice) -> Result<()> {
        self.choice = device.clone();
        self.signals
            .send(Signal::Switch(device))
            .map_err(|_| Error::Output("the output thread stopped".into()))
    }

    /// The device asked for, by [`Output::start_on`] or the last
    /// [`Output::set_device`].
    pub fn device(&self) -> &DeviceChoice {
        &self.choice
    }

    /// The device the stream is on now: the one asked for, or
    /// [`DeviceChoice::FollowDefault`] when that could not be opened.
    pub fn playing_on(&self) -> DeviceChoice {
        lock(&self.playing_on).clone()
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        let _ = self.signals.send(Signal::Shutdown);
        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.join();
        }
    }
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}

/// Owns the stream on its own thread: opens the first, a new one each time
/// one fails, and another device's when asked.
struct Supervisor<B> {
    backend: B,
    reports: SyncSender<Signal>,
    signals: Receiver<Signal>,
    choice: DeviceChoice,
    playing_on: Arc<Mutex<DeviceChoice>>,
}

/// What a running stream needs from its supervisor.
struct Running {
    format: DeviceFormat,
    engine: Arc<Mutex<Engine>>,
    notices: SyncSender<EngineEvent>,
    generation: u64,
    stream: Option<Box<dyn Any>>,
}

impl<B: OpenStream> Supervisor<B> {
    fn run(mut self, decode: DecodeMode, ready: SyncSender<Result<Started>>) {
        // The chosen device's format, or the default's when it is not there.
        let format = self
            .backend
            .device_format(&self.choice)
            .or_else(|error| match self.choice {
                DeviceChoice::FollowDefault => Err(error),
                DeviceChoice::Device { .. } => self.backend.default_format(),
            });
        let format = match format {
            Ok(format) => format,
            Err(error) => {
                let _ = ready.send(Err(error));
                return;
            }
        };
        let config = EngineConfig {
            sample_rate: format.sample_rate,
            channels: format.channels,
            decode,
        };
        let (engine, handle, events) = Engine::new(config);
        let mut running = Running {
            format,
            notices: engine.event_sender(),
            engine: Arc::new(Mutex::new(engine)),
            generation: 0,
            stream: None,
        };
        match self.open(&mut running) {
            Ok(()) => {
                if ready.send(Ok((handle, events, config))).is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = ready.send(Err(error));
                return;
            }
        }

        loop {
            match self.signals.recv() {
                Ok(Signal::Failed {
                    generation: failed,
                    message,
                }) if failed == running.generation => {
                    let _ = running
                        .notices
                        .try_send(EngineEvent::DeviceError { message });
                    drop(running.stream.take());
                    lock(&running.engine).resume_from_heard();
                    if !self.reopen(&mut running, true) {
                        return;
                    }
                }
                // A late report from a stream already replaced.
                Ok(Signal::Failed { .. }) => {}
                Ok(Signal::Switch(choice)) => {
                    self.choice = choice;
                    drop(running.stream.take());
                    lock(&running.engine).resume_from_heard();
                    if !self.reopen(&mut running, false) {
                        return;
                    }
                }
                Ok(Signal::Shutdown) | Err(_) => return,
            }
        }
    }

    /// Opens the chosen device, or the default when it will not open, and
    /// records which it is on.
    fn open(&mut self, running: &mut Running) -> Result<()> {
        let choice = self.choice.clone();
        let stream = match self.open_on(&choice, running) {
            Ok(stream) => {
                *lock(&self.playing_on) = choice;
                stream
            }
            Err(error) if choice == DeviceChoice::FollowDefault => return Err(error),
            Err(error) => {
                let stream = self.open_on(&DeviceChoice::FollowDefault, running)?;
                let _ = running.notices.try_send(EngineEvent::DeviceError {
                    message: format!("{error}; playing on the default device"),
                });
                *lock(&self.playing_on) = DeviceChoice::FollowDefault;
                stream
            }
        };
        running.stream = Some(stream);
        Ok(())
    }

    fn open_on(&mut self, device: &DeviceChoice, running: &mut Running) -> Result<Box<dyn Any>> {
        running.generation += 1;
        let report = StreamReport {
            generation: running.generation,
            signals: self.reports.clone(),
            events: running.notices.clone(),
        };
        let render = Render {
            engine: Arc::clone(&running.engine),
        };
        self.backend
            .open_device(device, running.format, render, report)
    }

    /// Opens a stream again, trying until one opens; announces it restored
    /// after a failure, or after a switch that took more than one attempt.
    /// False when the output was shut down meanwhile.
    fn reopen(&mut self, running: &mut Running, after_failure: bool) -> bool {
        let mut attempt = 0;
        loop {
            match self.open(running) {
                Ok(()) => {
                    if after_failure || attempt > 0 {
                        let _ = running.notices.try_send(EngineEvent::DeviceRestored);
                    }
                    return true;
                }
                Err(error) => {
                    if attempt == 0 {
                        let _ = running.notices.try_send(EngineEvent::DeviceError {
                            message: error.to_string(),
                        });
                    }
                    match self
                        .signals
                        .recv_timeout(RETRY[attempt.min(RETRY.len() - 1)])
                    {
                        Ok(Signal::Shutdown) | Err(RecvTimeoutError::Disconnected) => return false,
                        Ok(Signal::Switch(choice)) => self.choice = choice,
                        Ok(Signal::Failed { .. }) | Err(RecvTimeoutError::Timeout) => {}
                    }
                    attempt += 1;
                }
            }
        }
    }
}

/// The default cpal host's output devices.
pub struct CpalOutput;

impl OpenStream for CpalOutput {
    fn default_format(&mut self) -> Result<DeviceFormat> {
        self.device_format(&DeviceChoice::FollowDefault)
    }

    fn open(
        &mut self,
        format: DeviceFormat,
        render: Render,
        report: StreamReport,
    ) -> Result<Box<dyn Any>> {
        self.open_device(&DeviceChoice::FollowDefault, format, render, report)
    }

    fn device_format(&mut self, device: &DeviceChoice) -> Result<DeviceFormat> {
        let supported = find_device(device)?
            .default_output_config()
            .map_err(|e| Error::Output(e.to_string()))?;
        Ok(DeviceFormat {
            sample_rate: supported.sample_rate(),
            channels: usize::from(supported.channels()),
        })
    }

    fn open_device(
        &mut self,
        device: &DeviceChoice,
        format: DeviceFormat,
        render: Render,
        report: StreamReport,
    ) -> Result<Box<dyn Any>> {
        let device = find_device(device)?;
        let default = device
            .default_output_config()
            .map_err(|e| Error::Output(e.to_string()))?;
        let supported = if default.sample_rate() == format.sample_rate
            && usize::from(default.channels()) == format.channels
        {
            default
        } else {
            // Another device than the engine was made for: the engine's rate
            // and channels on it, in f32 if it can.
            device
                .supported_output_configs()
                .map_err(|e| Error::Output(e.to_string()))?
                .filter(|range| usize::from(range.channels()) == format.channels)
                .filter_map(|range| range.try_with_sample_rate(format.sample_rate))
                .max_by_key(|config| config.sample_format() == SampleFormat::F32)
                .ok_or_else(|| {
                    Error::Output(format!(
                        "the output device does not play {} Hz in {} channels",
                        format.sample_rate, format.channels
                    ))
                })?
        };
        let sample_format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let stream = match sample_format {
            SampleFormat::F32 => build::<f32>(&device, &config, render, report),
            SampleFormat::I16 => build::<i16>(&device, &config, render, report),
            SampleFormat::I32 => build::<i32>(&device, &config, render, report),
            SampleFormat::U16 => build::<u16>(&device, &config, render, report),
            SampleFormat::F64 => build::<f64>(&device, &config, render, report),
            other => Err(Error::Output(format!("unsupported sample format {other}"))),
        }?;
        stream.play().map_err(|e| Error::Output(e.to_string()))?;
        Ok(Box::new(stream))
    }
}

/// The output devices of the default cpal host (ALSA on Linux, where
/// PipeWire and PulseAudio appear as its `pipewire`, `pulse` and `default`
/// devices), the default among them marked. A device that will not say its
/// id or name is left out.
pub fn output_devices() -> Result<Vec<OutputDevice>> {
    let host = cpal::default_host();
    let default = host.default_output_device().and_then(|d| d.id().ok());
    let devices = host
        .output_devices()
        .map_err(|e| Error::Output(e.to_string()))?;
    Ok(devices
        .filter_map(|device| {
            let id = device.id().ok()?;
            let name = device.description().ok()?.name().to_string();
            Some(OutputDevice {
                is_default: default.as_ref() == Some(&id),
                id: id.to_string(),
                name,
            })
        })
        .collect())
}

fn find_device(device: &DeviceChoice) -> Result<cpal::Device> {
    let host = cpal::default_host();
    match device {
        DeviceChoice::FollowDefault => host
            .default_output_device()
            .ok_or_else(|| Error::Output("no output device".into())),
        DeviceChoice::Device { id, name } => {
            if let Ok(id) = id.parse::<cpal::DeviceId>()
                && let Some(device) = host.device_by_id(&id)
            {
                return Ok(device);
            }
            host.output_devices()
                .map_err(|e| Error::Output(e.to_string()))?
                .find(|device| {
                    device
                        .description()
                        .is_ok_and(|description| description.name() == name)
                })
                .ok_or_else(|| no_such_device(name))
        }
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut render: Render,
    report: StreamReport,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let rate = f64::from(config.sample_rate);
    // Room for a large callback; grown once if a device asks for more.
    let mut scratch: Vec<f32> = vec![0.0; 16_384 * usize::from(config.channels)];
    device
        .build_output_stream(
            *config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                let timestamp = info.timestamp();
                let latency = timestamp
                    .playback
                    .saturating_duration_since(timestamp.callback);
                if scratch.len() < data.len() {
                    scratch.resize(data.len(), 0.0);
                }
                let buffer = &mut scratch[..data.len()];
                render.render(buffer, (latency.as_secs_f64() * rate).round() as u64);
                for (out, sample) in data.iter_mut().zip(buffer.iter()) {
                    *out = T::from_sample(*sample);
                }
            },
            move |error: cpal::Error| match error.kind() {
                // The stream lives through these.
                cpal::ErrorKind::Xrun
                | cpal::ErrorKind::DeviceChanged
                | cpal::ErrorKind::RealtimeDenied => report.glitch(error.to_string()),
                _ => report.failed(error.to_string()),
            },
            None,
        )
        .map_err(|e| Error::Output(e.to_string()))
}
