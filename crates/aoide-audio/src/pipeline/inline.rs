//! The inline feed: decoding on the caller's thread, on demand.

use super::gapless::Pipeline;
use super::{Feed, FeedRead, Staging, TrackSpec};

/// Decodes on demand, on the caller's thread. Deterministic, for offline
/// rendering and tests; not for the audio callback, where decoding a network
/// stream would block it.
pub(crate) struct InlineFeed {
    pipeline: Pipeline,
    staging: Staging,
    channels: usize,
    read: u64,
}

impl InlineFeed {
    pub(crate) fn new(rate: u32, channels: usize, spec: TrackSpec, start_ms: u64) -> Self {
        let mut staging = Staging::default();
        let pipeline = Pipeline::new(rate, channels, spec, start_ms, &mut staging);
        Self {
            pipeline,
            staging,
            channels,
            read: 0,
        }
    }
}

impl Feed for InlineFeed {
    fn read(&mut self, out: &mut [f32]) -> FeedRead {
        loop {
            let read = self.read;
            if let Some((_, marker)) = self.staging.markers.pop_front_if(|(at, _)| *at <= read) {
                return FeedRead::Marker(marker);
            }
            let available = self.staging.frames(self.channels);
            if available > 0 {
                let until_marker = self
                    .staging
                    .markers
                    .front()
                    .map_or(u64::MAX, |&(at, _)| at - self.read);
                let frames = (out.len() / self.channels)
                    .min(available)
                    .min(until_marker as usize);
                if frames > 0 {
                    for (slot, sample) in out[..frames * self.channels]
                        .iter_mut()
                        .zip(self.staging.samples.drain(..frames * self.channels))
                    {
                        *slot = sample;
                    }
                    self.read += frames as u64;
                    return FeedRead::Audio(frames);
                }
            }
            if self.pipeline.is_idle() && self.staging.is_empty() {
                return FeedRead::Finished;
            }
            if self.pipeline.is_idle() && available == 0 {
                // Markers past the end of the audio: deliver them anyway.
                if let Some((_, marker)) = self.staging.markers.pop_front() {
                    return FeedRead::Marker(marker);
                }
            }
            self.pipeline.step(&mut self.staging);
        }
    }

    fn chain(&mut self, spec: Option<TrackSpec>) {
        self.pipeline.chain(spec);
    }
}
