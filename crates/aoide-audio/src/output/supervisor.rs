//! The output and the thread that owns its stream: opening it, opening it
//! again when it fails, and moving it to another device.

use std::any::Any;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::backend::CpalOutput;
use super::{DeviceChoice, DeviceFormat, OpenStream, Render, Signal, StreamReport};
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
    ///
    /// # Errors
    ///
    /// As [`Self::start_with`].
    pub fn start_default() -> Result<(Output, EngineHandle, Receiver<EngineEvent>)> {
        Self::start(CpalOutput, DecodeMode::Threaded)
    }

    /// The same on a chosen device (the default if it cannot be opened), at
    /// that device's own rate and channel count.
    ///
    /// # Errors
    ///
    /// As [`Self::start_with`].
    pub fn start_on(device: DeviceChoice) -> Result<(Output, EngineHandle, Receiver<EngineEvent>)> {
        Self::start_with(CpalOutput, DecodeMode::Threaded, device)
    }

    /// The default device on any [`OpenStream`].
    ///
    /// # Errors
    ///
    /// As [`Self::start_with`].
    pub fn start(
        backend: impl OpenStream,
        decode: DecodeMode,
    ) -> Result<(Output, EngineHandle, Receiver<EngineEvent>)> {
        Self::start_with(backend, decode, DeviceChoice::FollowDefault)
    }

    /// A chosen device on any [`OpenStream`].
    ///
    /// # Errors
    ///
    /// [`Error::Output`] when the output thread cannot be started or stops, or whatever
    /// opening fails with when neither the chosen device nor the default will open.
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

    /// The engine's rate, channel count and decode mode: the device's format when the
    /// output started.
    pub fn config(&self) -> EngineConfig {
        self.config
    }

    /// Moves playback to `device`, carrying on from what was heard. Returns
    /// at once; the switch happens on the output's thread. A device that
    /// cannot be opened (gone, or unable to play the engine's rate and
    /// channel count) gives way to the default, with an
    /// [`EngineEvent::DeviceError`] saying so.
    ///
    /// # Errors
    ///
    /// [`Error::Output`] when the output thread has stopped.
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
        let mut running = match self.start(decode) {
            Ok((running, started)) => {
                if ready.send(Ok(started)).is_err() {
                    return;
                }
                running
            }
            Err(error) => {
                let _ = ready.send(Err(error));
                return;
            }
        };

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

    /// Makes the engine at the chosen device's format (the default's when it
    /// is not there) and opens its first stream.
    fn start(&mut self, decode: DecodeMode) -> Result<(Running, Started)> {
        let format =
            self.backend
                .device_format(&self.choice)
                .or_else(|error| match self.choice {
                    DeviceChoice::FollowDefault => Err(error),
                    DeviceChoice::Device { .. } => self.backend.default_format(),
                })?;
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
        self.open(&mut running)?;
        Ok((running, (handle, events, config)))
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
