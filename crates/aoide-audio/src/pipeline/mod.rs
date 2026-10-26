//! From a source to frames at the device's rate, with markers saying where each
//! track starts and ends.
//!
//! A pipeline plays one track and then, gaplessly, whatever is chained after
//! it. Consecutive tracks at the same rate share one resampler, so a join
//! through a rate conversion is as continuous as the music. The output is a
//! plain run of frames plus markers at absolute frame positions in that run;
//! the render core reads the frames up to each marker and switches tracks
//! exactly there.
//!
//! A [`Feed`] carries a pipeline's output to the render core. The threaded
//! feed decodes on its own thread into lock-free rings, which is what the
//! audio callback reads; the inline feed decodes on demand, which is what the
//! offline tests use so their output does not depend on timing.

mod gapless;
mod inline;
mod spec;
mod threaded;

use std::collections::VecDeque;

pub(crate) use inline::InlineFeed;
pub use spec::TrackSpec;
pub(crate) use threaded::ThreadFeed;

use crate::queue::EntryId;

#[derive(Debug)]
pub(crate) enum Marker {
    Start {
        id: EntryId,
        gain: f32,
        /// Milliseconds into the file of the first frame after this marker.
        origin_ms: f64,
        /// Output frames from here to the track's end, when known.
        length: Option<u64>,
    },
    End {
        id: EntryId,
    },
    Error {
        id: EntryId,
        message: String,
    },
}

/// Where a pipeline's output goes.
#[derive(Default)]
pub(crate) struct Staging {
    pub(super) samples: VecDeque<f32>,
    pub(super) markers: VecDeque<(u64, Marker)>,
    /// Frames ever staged.
    staged: u64,
}

impl Staging {
    pub(super) fn audio(&mut self, samples: &[f32], channels: usize) {
        self.samples.extend(samples.iter().copied());
        self.staged += (samples.len() / channels) as u64;
    }

    pub(super) fn marker(&mut self, at: u64, marker: Marker) {
        self.markers.push_back((at, marker));
    }

    pub(super) fn is_empty(&self) -> bool {
        self.samples.is_empty() && self.markers.is_empty()
    }

    pub(super) fn frames(&self, channels: usize) -> usize {
        self.samples.len() / channels
    }
}

/// What a feed has for the render core next.
#[derive(Debug)]
pub(crate) enum FeedRead {
    /// This many frames were written, all before the next marker.
    Audio(usize),
    Marker(Marker),
    /// Nothing ready yet; the stream is still coming.
    Starved,
    /// Nothing more will come.
    Finished,
}

/// The consumer side of a pipeline.
pub(crate) trait Feed: Send {
    /// Writes up to `out.len() / channels` frames, stopping at a marker.
    fn read(&mut self, out: &mut [f32]) -> FeedRead;
    /// Chains a track after the current one, or clears what was chained.
    fn chain(&mut self, spec: Option<TrackSpec>);
}
