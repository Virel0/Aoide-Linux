//! One track, as the engine is told to play it.

use crate::decode::MediaSource;
use crate::mix::SoundBounds;
use crate::queue::EntryId;

#[derive(Clone, Debug)]
pub struct TrackSpec {
    pub id: EntryId,
    pub source: MediaSource,
    /// Play only this span of the file.
    pub bounds: Option<SoundBounds>,
    /// Loudness levelling from the sidecar, in dB; only a negative gain has
    /// any effect.
    pub gain_db: Option<f64>,
    /// Level volume over the file's own ReplayGain tags: when set, a file
    /// that has them plays by them (album gain first, peak-limited, see
    /// [`crate::loudness::ReplayGainTags::linear_gain`]) and `gain_db`
    /// stands down for it. Unset, the tags are ignored.
    pub replay_gain: bool,
}

impl TrackSpec {
    pub fn new(id: EntryId, source: MediaSource) -> Self {
        Self {
            id,
            source,
            bounds: None,
            gain_db: None,
            replay_gain: false,
        }
    }

    pub fn with_bounds(mut self, bounds: Option<SoundBounds>) -> Self {
        self.bounds = bounds;
        self
    }

    pub fn with_gain_db(mut self, gain_db: Option<f64>) -> Self {
        self.gain_db = gain_db;
        self
    }

    pub fn with_replay_gain(mut self, replay_gain: bool) -> Self {
        self.replay_gain = replay_gain;
        self
    }
}
