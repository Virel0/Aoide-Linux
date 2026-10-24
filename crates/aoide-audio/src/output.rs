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
//! Opening a stream is behind [`OpenStream`], so the recovery is tested
//! without a device.

use std::any::Any;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

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
    Shutdown,
}

/// A running output. Keep it alive for as long as there should be sound;
/// dropping it stops the device. The stream lives on a thread of its own,
/// which reopens it when it fails.
pub struct Output {
    config: EngineConfig,
    signals: SyncSender<Signal>,
    supervisor: Option<JoinHandle<()>>,
}

type Started = (EngineHandle, Receiver<EngineEvent>, EngineConfig);

impl Output {
    /// Opens the default output device at its own rate and channel count,
    /// starts an engine on it, and returns the handle and event channel.
    pub fn start_default() -> Result<(Output, EngineHandle, Receiver<EngineEvent>)> {
        Self::start(CpalOutput, DecodeMode::Threaded)
    }

    /// The same on any [`OpenStream`].
    pub fn start(
        backend: impl OpenStream,
        decode: DecodeMode,
    ) -> Result<(Output, EngineHandle, Receiver<EngineEvent>)> {
        let (signals_tx, signals) = mpsc::sync_channel(16);
        let (ready_tx, ready) = mpsc::sync_channel(1);
        let reports = signals_tx.clone();
        let supervisor = thread::Builder::new()
            .name("aoide-output".into())
            .spawn(move || supervise(backend, decode, reports, signals, ready_tx))
            .map_err(|e| Error::Output(e.to_string()))?;
        match ready.recv() {
            Ok(Ok((handle, events, config))) => Ok((
                Output {
                    config,
                    signals: signals_tx,
                    supervisor: Some(supervisor),
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
}

impl Drop for Output {
    fn drop(&mut self) {
        let _ = self.signals.send(Signal::Shutdown);
        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.join();
        }
    }
}

fn lock(engine: &Mutex<Engine>) -> MutexGuard<'_, Engine> {
    engine.lock().unwrap_or_else(|e| e.into_inner())
}

/// Owns the stream: opens the first, and a new one each time one fails.
fn supervise(
    mut backend: impl OpenStream,
    decode: DecodeMode,
    reports: SyncSender<Signal>,
    signals: Receiver<Signal>,
    ready: SyncSender<Result<Started>>,
) {
    let mut generation = 0;
    let first = backend.default_format().and_then(|format| {
        let config = EngineConfig {
            sample_rate: format.sample_rate,
            channels: format.channels,
            decode,
        };
        let (engine, handle, events) = Engine::new(config);
        let notices = engine.event_sender();
        let engine = Arc::new(Mutex::new(engine));
        let report = StreamReport {
            generation,
            signals: reports.clone(),
            events: notices.clone(),
        };
        let render = Render {
            engine: Arc::clone(&engine),
        };
        let stream = backend.open(format, render, report)?;
        Ok((format, engine, notices, stream, (handle, events, config)))
    });
    let (format, engine, notices, mut stream) = match first {
        Ok((format, engine, notices, stream, started)) => {
            if ready.send(Ok(started)).is_err() {
                return;
            }
            (format, engine, notices, Some(stream))
        }
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };

    loop {
        let message = match signals.recv() {
            Ok(Signal::Failed {
                generation: failed,
                message,
            }) if failed == generation => message,
            // A late report from a stream already replaced.
            Ok(Signal::Failed { .. }) => continue,
            Ok(Signal::Shutdown) | Err(_) => return,
        };
        let _ = notices.try_send(EngineEvent::DeviceError { message });
        drop(stream.take());
        lock(&engine).resume_from_heard();

        let mut attempt = 0;
        while stream.is_none() {
            generation += 1;
            let report = StreamReport {
                generation,
                signals: reports.clone(),
                events: notices.clone(),
            };
            let render = Render {
                engine: Arc::clone(&engine),
            };
            match backend.open(format, render, report) {
                Ok(opened) => {
                    stream = Some(opened);
                    let _ = notices.try_send(EngineEvent::DeviceRestored);
                }
                Err(error) => {
                    if attempt == 0 {
                        let _ = notices.try_send(EngineEvent::DeviceError {
                            message: error.to_string(),
                        });
                    }
                    match signals.recv_timeout(RETRY[attempt.min(RETRY.len() - 1)]) {
                        Ok(Signal::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
                        Ok(Signal::Failed { .. }) | Err(RecvTimeoutError::Timeout) => {}
                    }
                    attempt += 1;
                }
            }
        }
    }
}

/// The default cpal host's default output device.
pub struct CpalOutput;

impl OpenStream for CpalOutput {
    fn default_format(&mut self) -> Result<DeviceFormat> {
        let supported = default_device()?
            .default_output_config()
            .map_err(|e| Error::Output(e.to_string()))?;
        Ok(DeviceFormat {
            sample_rate: supported.sample_rate(),
            channels: usize::from(supported.channels()),
        })
    }

    fn open(
        &mut self,
        format: DeviceFormat,
        render: Render,
        report: StreamReport,
    ) -> Result<Box<dyn Any>> {
        let device = default_device()?;
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

fn default_device() -> Result<cpal::Device> {
    cpal::default_host()
        .default_output_device()
        .ok_or_else(|| Error::Output("no output device".into()))
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
