//! The audio device: cpal's default output (ALSA, or PipeWire through its ALSA
//! plugin, on Linux), calling the engine from its callback.

use std::sync::mpsc::Receiver;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

use crate::engine::{DecodeMode, Engine, EngineConfig, EngineEvent, EngineHandle};
use crate::error::{Error, Result};

/// A running output stream. Keep it alive for as long as there should be
/// sound; dropping it stops the device. Not `Send` on every platform, so keep
/// it on the thread that made it.
pub struct Output {
    _stream: cpal::Stream,
    config: EngineConfig,
}

impl Output {
    /// Opens the default output device at its own rate and channel count,
    /// starts an engine on it, and returns the handle and event channel.
    pub fn start_default() -> Result<(Output, EngineHandle, Receiver<EngineEvent>)> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| Error::Output("no output device".into()))?;
        let supported = device
            .default_output_config()
            .map_err(|e| Error::Output(e.to_string()))?;
        let format = supported.sample_format();
        let stream_config: cpal::StreamConfig = supported.into();
        let config = EngineConfig {
            sample_rate: stream_config.sample_rate,
            channels: usize::from(stream_config.channels),
            decode: DecodeMode::Threaded,
        };
        let (engine, handle, events) = Engine::new(config);
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, &stream_config, engine),
            SampleFormat::I16 => build::<i16>(&device, &stream_config, engine),
            SampleFormat::I32 => build::<i32>(&device, &stream_config, engine),
            SampleFormat::U16 => build::<u16>(&device, &stream_config, engine),
            SampleFormat::F64 => build::<f64>(&device, &stream_config, engine),
            other => Err(Error::Output(format!("unsupported sample format {other}"))),
        }?;
        stream.play().map_err(|e| Error::Output(e.to_string()))?;
        Ok((
            Output {
                _stream: stream,
                config,
            },
            handle,
            events,
        ))
    }

    pub fn config(&self) -> EngineConfig {
        self.config
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut engine: Engine,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let rate = f64::from(config.sample_rate);
    let errors = engine.event_sender();
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
                engine.set_output_latency_frames((latency.as_secs_f64() * rate).round() as u64);
                if scratch.len() < data.len() {
                    scratch.resize(data.len(), 0.0);
                }
                let buffer = &mut scratch[..data.len()];
                engine.render(buffer);
                for (out, sample) in data.iter_mut().zip(buffer.iter()) {
                    *out = T::from_sample(*sample);
                }
            },
            move |error| {
                let _ = errors.try_send(EngineEvent::DeviceError {
                    message: error.to_string(),
                });
            },
            None,
        )
        .map_err(|e| Error::Output(e.to_string()))
}
