//! The cpal backend: the default host's devices, and a stream on one of them
//! calling the engine.

use std::any::Any;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

use super::{
    DeviceChoice, DeviceFormat, OpenStream, OutputDevice, Render, StreamReport, no_such_device,
};
use crate::error::{Error, Result};

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
///
/// # Errors
///
/// [`Error::Output`] when the host cannot list its devices.
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
