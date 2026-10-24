//! Fixtures written by the tests themselves, and the measurements they make.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use aoide_audio::{DecodeMode, Engine, EngineConfig, EngineEvent, EngineHandle};

pub const RATE: u32 = 48_000;
pub const SCALE: f32 = 32_768.0;

/// A sawtooth of 16-bit integers that steps by one every frame, starting at
/// `start`: any gap, repeat or dropped frame in a render breaks the step.
/// Channel 1 is channel 0 negated, so swapped channels show too.
pub fn ramp(start: u64, frames: usize, channels: usize) -> Vec<i32> {
    let mut samples = Vec::with_capacity(frames * channels);
    for k in 0..frames as u64 {
        let value = ((start + k) % 20_000) as i32 - 10_000;
        for channel in 0..channels {
            samples.push(if channel % 2 == 0 { value } else { -value });
        }
    }
    samples
}

/// A sine at `freq` Hz with peak `amplitude` (of full scale).
pub fn tone(freq: f64, amplitude: f64, rate: u32, frames: usize, channels: usize) -> Vec<i32> {
    let mut samples = Vec::with_capacity(frames * channels);
    for k in 0..frames {
        let value = (amplitude
            * 32_767.0
            * (2.0 * std::f64::consts::PI * freq * k as f64 / f64::from(rate)).sin())
        .round() as i32;
        samples.extend(std::iter::repeat_n(value, channels));
    }
    samples
}

/// White noise, uniform in ±`amplitude` of full scale, from its own seed so two
/// seeds are uncorrelated (as two recordings are).
pub fn noise(seed: u64, amplitude: f64, frames: usize, channels: usize) -> Vec<i32> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut samples = Vec::with_capacity(frames * channels);
    for _ in 0..frames {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let unit = (state >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0;
        let value = (unit * amplitude * 32_767.0).round() as i32;
        samples.extend(std::iter::repeat_n(value, channels));
    }
    samples
}

pub fn write_wav(path: &Path, rate: u32, channels: usize, samples: &[i32]) {
    let data_len = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&(channels as u16).to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
    bytes.extend_from_slice(&(channels as u16 * 2).to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for &sample in samples {
        bytes.extend_from_slice(&(sample as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).expect("write wav");
}

/// 16-bit PCM AIFF: big-endian samples, the rate as an 80-bit float.
pub fn write_aiff(path: &Path, rate: u32, channels: usize, samples: &[i32]) {
    let frames = (samples.len() / channels) as u32;
    let data_len = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(54 + data_len as usize);
    bytes.extend_from_slice(b"FORM");
    bytes.extend_from_slice(&(46 + data_len).to_be_bytes());
    bytes.extend_from_slice(b"AIFFCOMM");
    bytes.extend_from_slice(&18u32.to_be_bytes());
    bytes.extend_from_slice(&(channels as u16).to_be_bytes());
    bytes.extend_from_slice(&frames.to_be_bytes());
    bytes.extend_from_slice(&16u16.to_be_bytes());
    // Extended precision: sign and exponent, then the mantissa with its
    // leading one explicit.
    let shift = rate.leading_zeros();
    bytes.extend_from_slice(&((16_383 + 31 - shift) as u16).to_be_bytes());
    bytes.extend_from_slice(&((u64::from(rate) << shift) << 32).to_be_bytes());
    bytes.extend_from_slice(b"SSND");
    bytes.extend_from_slice(&(8 + data_len).to_be_bytes());
    bytes.extend_from_slice(&[0; 8]); // offset and block size
    for &sample in samples {
        bytes.extend_from_slice(&(sample as i16).to_be_bytes());
    }
    std::fs::write(path, bytes).expect("write aiff");
}

pub fn flac_bytes(rate: u32, channels: usize, samples: &[i32]) -> Vec<u8> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;
    let config = flacenc::config::Encoder::default()
        .into_verified()
        .expect("flac config");
    let source = flacenc::source::MemSource::from_samples(samples, channels, 16, rate as usize);
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .expect("flac encode");
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream.write(&mut sink).expect("flac write");
    let mut bytes = sink.as_slice().to_vec();
    // flacenc records the short last block as STREAMINFO's minimum block size;
    // libFLAC records the fixed block size, and symphonia 0.6.1 refuses the
    // former with an unexpected end of file. Write what libFLAC writes.
    assert_eq!(&bytes[..4], b"fLaC");
    let max = [bytes[10], bytes[11]];
    bytes[8..10].copy_from_slice(&max);
    bytes
}

pub fn write_flac(path: &Path, rate: u32, channels: usize, samples: &[i32]) {
    std::fs::write(path, flac_bytes(rate, channels, samples)).expect("write flac");
}

pub fn to_f32(samples: &[i32]) -> Vec<f32> {
    samples.iter().map(|&s| s as f32 / SCALE).collect()
}

pub struct Dir(tempfile::TempDir);

impl Dir {
    pub fn new() -> Self {
        Dir(tempfile::tempdir().expect("temp dir"))
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.0.path().join(name)
    }
}

pub fn inline_engine(channels: usize) -> (Engine, EngineHandle, Receiver<EngineEvent>) {
    Engine::new(EngineConfig {
        sample_rate: RATE,
        channels,
        decode: DecodeMode::Inline,
    })
}

/// Renders `frames` frames in blocks of `block`, as a device callback would.
pub fn render(engine: &mut Engine, frames: usize, block: usize) -> Vec<f32> {
    let channels = engine.config().channels;
    let mut out = vec![0.0; frames * channels];
    for chunk in out.chunks_mut(block * channels) {
        engine.render(chunk);
    }
    out
}

pub fn drain(events: &Receiver<EngineEvent>) -> Vec<EngineEvent> {
    events
        .try_iter()
        .filter(|e| !matches!(e, EngineEvent::Position { .. }))
        .collect()
}

/// RMS of channel 0 over `frames`.
pub fn rms(samples: &[f32], channels: usize) -> f64 {
    let values: Vec<f64> = samples
        .chunks_exact(channels)
        .map(|f| f64::from(f[0]))
        .collect();
    (values.iter().map(|v| v * v).sum::<f64>() / values.len().max(1) as f64).sqrt()
}

pub fn db(ratio: f64) -> f64 {
    20.0 * ratio.log10()
}
