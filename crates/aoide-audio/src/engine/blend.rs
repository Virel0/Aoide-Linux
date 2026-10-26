//! Crossfades: the incoming track over the outgoing one on equal-power curves.

use super::EngineEvent;
use super::deck::{Deck, pull};
use super::render::Engine;
use crate::mix::curve;

pub(super) struct Blend {
    pub(super) deck: Deck,
    pub(super) overlap: u64,
    /// Frames into the overlap; `None` until it starts.
    pub(super) progress: Option<u64>,
}

impl Engine {
    /// Renders up to `frames` frames of the blend under way at `offset` into
    /// `out`. When the overlap is done, the incoming deck becomes the primary
    /// and the queued blend, if any, is next. Returns the frames rendered.
    pub(super) fn render_blend(&mut self, out: &mut [f32], offset: usize, frames: usize) -> usize {
        let channels = self.config.channels;
        let rate = self.config.sample_rate;
        let clock = self.clock + offset as u64;
        let blend = self.blend.as_mut().expect("a blend under way");
        let progress = blend.progress.unwrap_or(0);
        let span = frames.min((blend.overlap - progress) as usize);
        let region = &mut out[offset * channels..(offset + span) * channels];

        // The incoming track into the block, the outgoing one into scratch.
        pull(
            &mut blend.deck,
            region,
            channels,
            &mut self.timeline,
            clock,
            true,
            rate,
            &self.events,
        );
        let scratch = &mut self.scratch[..span * channels];
        scratch.fill(0.0);
        if let Some(primary) = self.primary.as_mut() {
            pull(
                primary,
                scratch,
                channels,
                &mut self.timeline,
                clock,
                false,
                rate,
                &self.events,
            );
        }
        crossfade(region, scratch, channels, progress, blend.overlap);

        let progress = progress + span as u64;
        blend.progress = Some(progress);
        if progress >= blend.overlap {
            let blend = self.blend.take().expect("checked above");
            self.blend = self.queued.take();
            if let Some(old) = self.primary.replace(blend.deck) {
                // Its end had not been reached: it ends with the overlap.
                if let Some(track) = &old.track {
                    let ended = EngineEvent::TrackEnded { id: track.id };
                    self.timeline
                        .event(clock + span as u64, ended, &self.events);
                }
                self.reap(old.feed);
            }
        }
        span
    }
}

/// Mixes `outgoing` into `incoming` (both interleaved, the same length), the
/// first frame being `progress` frames into an overlap of `overlap`.
fn crossfade(incoming: &mut [f32], outgoing: &[f32], channels: usize, progress: u64, overlap: u64) {
    for (index, (frame, old)) in incoming
        .chunks_exact_mut(channels)
        .zip(outgoing.chunks_exact(channels))
        .enumerate()
    {
        let p = (progress + index as u64) as f64 / overlap as f64;
        let (gain_in, gain_out) = (curve::incoming(p) as f32, curve::outgoing(p) as f32);
        for (sample, old) in frame.iter_mut().zip(old) {
            *sample = *sample * gain_in + old * gain_out;
        }
    }
}
