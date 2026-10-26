//! The pipeline: one track's decoder, then whatever is chained after it,
//! through a resampler shared across a join at the same rate.

use std::sync::mpsc::{self, Receiver};
use std::thread;

use super::{Marker, Staging, TrackSpec};
use crate::decode::Decoder;
use crate::error::{Error, Result};
use crate::loudness::level_gain;
use crate::queue::EntryId;
use crate::resample::{StreamResampler, scale};

struct Active {
    spec: TrackSpec,
    decoder: Decoder,
    rate: u32,
    /// First and one-past-last source frame to play.
    start: u64,
    end: Option<u64>,
}

enum Next {
    None,
    Opening(TrackSpec, Receiver<Result<Decoder>>),
}

pub(super) struct Pipeline {
    rate: u32,
    channels: usize,
    current: Option<Active>,
    next: Next,
    resampler: Option<StreamResampler>,
    /// Output frames emitted before the current resampler was made.
    resampler_base: u64,
    /// Output frames emitted so far, resampler output included.
    emitted: u64,
    mapped: Vec<f32>,
}

impl Pipeline {
    /// A pipeline starting `spec` at `start_ms` into the file (clamped to its
    /// sound bounds). Open errors arrive as an error marker, not here.
    pub(crate) fn new(
        rate: u32,
        channels: usize,
        spec: TrackSpec,
        start_ms: u64,
        out: &mut Staging,
    ) -> Self {
        let mut pipeline = Self {
            rate,
            channels,
            current: None,
            next: Next::None,
            resampler: None,
            resampler_base: 0,
            emitted: 0,
            mapped: Vec::new(),
        };
        match Decoder::open(&spec.source) {
            Ok(decoder) => pipeline.begin(spec, decoder, start_ms, out),
            Err(error) => pipeline.report(spec.id, error, out),
        }
        pipeline
    }

    /// Chains `spec` after the current track, replacing anything chained
    /// before. Opening starts at once, on its own thread, so a stream's first
    /// bytes are on their way long before they are needed.
    pub(crate) fn chain(&mut self, spec: Option<TrackSpec>) {
        self.next = match spec {
            None => Next::None,
            Some(spec) => {
                let (tx, rx) = mpsc::sync_channel(1);
                let source = spec.source.clone();
                let opened = thread::Builder::new()
                    .name("aoide-open".into())
                    .spawn(move || {
                        let _ = tx.send(Decoder::open(&source));
                    });
                if opened.is_err() {
                    // No thread to spare: the open happens when it is needed.
                    let (tx, rx) = mpsc::sync_channel(1);
                    let _ = tx.send(Decoder::open(&spec.source));
                    Next::Opening(spec, rx)
                } else {
                    Next::Opening(spec, rx)
                }
            }
        };
    }

    /// Nothing playing and nothing chained.
    pub(crate) fn is_idle(&self) -> bool {
        self.current.is_none() && matches!(self.next, Next::None)
    }

    /// Decodes one packet, or moves on to the chained track.
    pub(crate) fn step(&mut self, out: &mut Staging) {
        let Some(active) = self.current.as_mut() else {
            self.advance(out);
            return;
        };
        let decoded = match active.decoder.next_packet() {
            Ok(Some(decoded)) => decoded,
            Ok(None) => return self.finish_current(out),
            Err(error) => {
                let id = active.spec.id;
                self.report(id, error, out);
                return self.finish_current(out);
            }
        };

        // Keep only the frames inside the sound bounds.
        let frames = decoded.frames() as u64;
        let from = active.start.max(decoded.first_frame);
        let to = match active.end {
            Some(end) => end.min(decoded.first_frame + frames),
            None => decoded.first_frame + frames,
        };
        let channels_in = decoded.channels;
        let past_end = active
            .end
            .is_some_and(|end| decoded.first_frame + frames >= end);
        if to > from {
            let a = ((from - decoded.first_frame) as usize) * channels_in;
            let b = ((to - decoded.first_frame) as usize) * channels_in;
            map_channels(
                &decoded.samples[a..b],
                channels_in,
                self.channels,
                &mut self.mapped,
            );
            let rate = active.rate;
            self.emit_mapped(rate, out);
        }
        if past_end {
            self.finish_current(out);
        }
    }

    fn begin(&mut self, spec: TrackSpec, mut decoder: Decoder, start_ms: u64, out: &mut Staging) {
        let rate = decoder.sample_rate();
        let ms_to_frames = |ms: u64| scale(ms, 1000, rate);
        let bound_start = spec.bounds.map_or(0, |b| ms_to_frames(b.start_ms));
        let mut end = spec.bounds.map(|b| ms_to_frames(b.end_ms));
        if let Some(total) = decoder.total_frames() {
            end = Some(end.map_or(total, |end| end.min(total)));
        }
        let start = bound_start.max(ms_to_frames(start_ms));

        if start > 0 && decoder.seek(start).is_err() {
            // Not seekable: decode from the top and drop what comes before.
            match Decoder::open(&spec.source) {
                Ok(fresh) => decoder = fresh,
                Err(error) => return self.report(spec.id, error, out),
            }
        }

        if let Err(error) = self.prepare_resampler(rate, out) {
            return self.report(spec.id, error, out);
        }

        let at = self.output_position();
        let length = end.map(|end| scale(end.saturating_sub(start), rate, self.rate));
        let tags = spec.replay_gain.then(|| decoder.replay_gain());
        out.marker(
            at,
            Marker::Start {
                id: spec.id,
                gain: level_gain(true, tags.as_ref(), spec.gain_db),
                origin_ms: start as f64 * 1000.0 / f64::from(rate),
                length,
            },
        );
        self.current = Some(Active {
            spec,
            decoder,
            rate,
            start,
            end,
        });
    }

    /// Readies the resampler for a track at `rate`, which needs one when it
    /// differs from the output's.
    fn prepare_resampler(&mut self, rate: u32, out: &mut Staging) -> Result<()> {
        // A change of rate ends the old resampler here, flushed, so the
        // previous track's last frames are not left inside it.
        if self
            .resampler
            .as_ref()
            .is_some_and(|r| r.input_rate() != rate)
            || rate == self.rate
        {
            self.flush_resampler(out);
        }
        if rate != self.rate && self.resampler.is_none() {
            let resampler = StreamResampler::new(rate, self.rate, self.channels)?;
            self.resampler_base = self.emitted;
            self.resampler = Some(resampler);
        }
        Ok(())
    }

    /// Where the next input frame will come out, counting the resampler's lag.
    fn output_position(&self) -> u64 {
        match &self.resampler {
            Some(resampler) => {
                self.resampler_base + resampler.output_index(resampler.input_frames())
            }
            None => self.emitted,
        }
    }

    fn finish_current(&mut self, out: &mut Staging) {
        let Some(active) = self.current.take() else {
            return;
        };
        let chained = matches!(self.next, Next::Opening(..));
        if !chained {
            self.flush_resampler(out);
        }
        out.marker(self.output_position(), Marker::End { id: active.spec.id });
        if chained {
            self.advance(out);
        }
    }

    fn advance(&mut self, out: &mut Staging) {
        match std::mem::replace(&mut self.next, Next::None) {
            Next::None => self.flush_resampler(out),
            Next::Opening(spec, opening) => {
                let opened = opening
                    .recv()
                    .unwrap_or_else(|_| Err(Error::Open("the open was abandoned".into())));
                match opened {
                    Ok(decoder) => self.begin(spec, decoder, 0, out),
                    Err(error) => {
                        let id = spec.id;
                        self.report(id, error, out);
                        out.marker(self.output_position(), Marker::End { id });
                        self.flush_resampler(out);
                    }
                }
            }
        }
    }

    fn report(&mut self, id: EntryId, error: Error, out: &mut Staging) {
        out.marker(
            self.output_position(),
            Marker::Error {
                id,
                message: error.to_string(),
            },
        );
    }

    fn emit_mapped(&mut self, rate: u32, out: &mut Staging) {
        let channels = self.channels;
        if rate == self.rate {
            out.audio(&self.mapped, channels);
            self.emitted += (self.mapped.len() / channels) as u64;
            return;
        }
        let Some(resampler) = self.resampler.as_mut() else {
            return;
        };
        let mut produced = 0u64;
        let result = resampler.push(&self.mapped, &mut |samples| {
            out.audio(samples, channels);
            produced += (samples.len() / channels) as u64;
        });
        self.emitted += produced;
        if let (Err(error), Some(active)) = (result, self.current.as_ref()) {
            let id = active.spec.id;
            self.report(id, error, out);
        }
    }

    fn flush_resampler(&mut self, out: &mut Staging) {
        let Some(mut resampler) = self.resampler.take() else {
            return;
        };
        let channels = self.channels;
        let mut produced = 0u64;
        let _ = resampler.flush(&mut |samples| {
            out.audio(samples, channels);
            produced += (samples.len() / channels) as u64;
        });
        self.emitted += produced;
    }
}

/// Maps interleaved `input` with `from` channels to `to` channels: mono is
/// copied to every output channel, more channels than the output keep the
/// first ones (front left and right), and a mono output averages them all.
fn map_channels(input: &[f32], from: usize, to: usize, out: &mut Vec<f32>) {
    out.clear();
    if from == to {
        out.extend_from_slice(input);
        return;
    }
    let frames = input.len() / from;
    out.reserve(frames * to);
    for frame in input.chunks_exact(from) {
        if to == 1 {
            out.push(frame.iter().sum::<f32>() / from as f32);
        } else if from == 1 {
            out.extend(std::iter::repeat_n(frame[0], to));
        } else {
            out.extend(frame.iter().copied().chain(std::iter::repeat(0.0)).take(to));
        }
    }
}
