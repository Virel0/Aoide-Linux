//! Streaming sample-rate conversion to the device rate, with rubato's FFT
//! resampler (a fixed ratio, which a file-to-device conversion is).
//!
//! The resampler's start-up delay is cut off the front and its tail is flushed
//! at the end, so `n` input frames come out as exactly `round(n × ratio)`
//! output frames aligned with the input. That is what lets a gapless join
//! through one resampler land on a known output frame.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};

use crate::error::{Error, Result};

const CHUNK: usize = 1024;

pub struct StreamResampler {
    inner: Fft<f32>,
    channels: usize,
    input_rate: u32,
    output_rate: u32,
    /// Input waiting for a whole chunk, interleaved.
    pending: Vec<f32>,
    out: Vec<f32>,
    /// Output frames still to drop for the start-up delay.
    delay_left: usize,
    in_total: u64,
    out_total: u64,
}

impl StreamResampler {
    pub fn new(input_rate: u32, output_rate: u32, channels: usize) -> Result<Self> {
        let inner = Fft::<f32>::new(
            input_rate as usize,
            output_rate as usize,
            CHUNK,
            channels,
            FixedSync::Input,
        )
        .map_err(|e| Error::Resample(e.to_string()))?;
        let delay_left = inner.output_delay();
        let out = vec![0.0; inner.output_frames_max() * channels];
        Ok(Self {
            inner,
            channels,
            input_rate,
            output_rate,
            pending: Vec::with_capacity(CHUNK * channels),
            out,
            delay_left,
            in_total: 0,
            out_total: 0,
        })
    }

    pub fn input_rate(&self) -> u32 {
        self.input_rate
    }

    /// Input frames taken so far.
    pub fn input_frames(&self) -> u64 {
        self.in_total
    }

    /// The output frame that input frame `n` lands on.
    pub fn output_index(&self, input_frame: u64) -> u64 {
        scale(input_frame, self.input_rate, self.output_rate)
    }

    /// Feeds interleaved input; whatever output is ready goes to `emit`.
    pub fn push(&mut self, mut input: &[f32], emit: &mut dyn FnMut(&[f32])) -> Result<()> {
        self.in_total += (input.len() / self.channels) as u64;
        while !input.is_empty() {
            let want = CHUNK * self.channels - self.pending.len();
            let take = want.min(input.len());
            self.pending.extend_from_slice(&input[..take]);
            input = &input[take..];
            if self.pending.len() == CHUNK * self.channels {
                self.process(None, None, emit)?;
                self.pending.clear();
            }
        }
        Ok(())
    }

    /// Pushes out everything still inside, so the output totals
    /// `round(input × ratio)` frames.
    pub fn flush(&mut self, emit: &mut dyn FnMut(&[f32])) -> Result<()> {
        let target = self.output_index(self.in_total);
        let partial = self.pending.len() / self.channels;
        if partial > 0 {
            self.pending.resize(CHUNK * self.channels, 0.0);
            self.process(Some(partial), Some(target), emit)?;
            self.pending.clear();
        }
        while self.out_total < target {
            self.process(Some(0), Some(target), emit)?;
        }
        Ok(())
    }

    fn process(
        &mut self,
        partial: Option<usize>,
        limit: Option<u64>,
        emit: &mut dyn FnMut(&[f32]),
    ) -> Result<()> {
        let channels = self.channels;
        let input = InterleavedSlice::new(&self.pending[..], channels, CHUNK)
            .map_err(|e| Error::Resample(e.to_string()))?;
        let frames_out = self.out.len() / channels;
        let mut output = InterleavedSlice::new_mut(&mut self.out[..], channels, frames_out)
            .map_err(|e| Error::Resample(e.to_string()))?;
        let indexing = partial.map(|n| Indexing::new().partial_len(n));
        let (_, written) = self
            .inner
            .process_into_buffer(&input, &mut output, indexing.as_ref())
            .map_err(|e| Error::Resample(e.to_string()))?;

        let skip = self.delay_left.min(written);
        self.delay_left -= skip;
        let mut usable = written - skip;
        if let Some(limit) = limit {
            usable = usable.min(limit.saturating_sub(self.out_total) as usize);
        }
        if usable > 0 {
            emit(&self.out[skip * channels..(skip + usable) * channels]);
            self.out_total += usable as u64;
        }
        Ok(())
    }
}

/// `frames` at `from` Hz expressed at `to` Hz, rounded to the nearest frame.
pub fn scale(frames: u64, from: u32, to: u32) -> u64 {
    if from == to {
        return frames;
    }
    ((u128::from(frames) * u128::from(to) * 2 + u128::from(from)) / (u128::from(from) * 2)) as u64
}
