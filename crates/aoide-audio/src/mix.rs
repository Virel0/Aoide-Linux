//! How one song is handed over to the next when crossfading is on.
//!
//! A port of `MixTransition.swift` in the iOS app and `mix-transition.ts` in the
//! desktop app, rule for rule and constant for constant. The three are held
//! together by one parity table (`tests/parity.rs` here), so a change to any
//! side breaks its own copy.
//!
//! **This is not beat-matching.** The tempo Melete measures is one BPM
//! figure with no downbeat and no grid, so tempo does one job here: deciding
//! whether a long blend is worth attempting. The crossfade itself is a
//! crossfade.

use std::f64::consts::FRAC_PI_2;

/// Where the sound in a file starts and stops, in milliseconds from the top of
/// the file. Measured by Melete; the engine plays only this span.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SoundBounds {
    /// Where the sound starts.
    pub start_ms: u64,
    /// Where it stops; after `start_ms` in a measured track.
    pub end_ms: u64,
}

impl SoundBounds {
    /// Bounds from their two ends.
    pub fn new(start_ms: u64, end_ms: u64) -> Self {
        Self { start_ms, end_ms }
    }
}

/// The handover from one track to the next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MixTransition {
    /// Straight into the next track with nothing between them: an album run.
    Gapless,
    /// The outgoing track finishes, then the next one starts.
    Cut,
    /// The two overlap for this many seconds with equal-power curves.
    Blend {
        /// The overlap, in seconds; the engine plays zero or less as a gapless join.
        seconds: f64,
    },
}

impl MixTransition {
    /// Seconds of overlap, zero for the transitions that have none.
    pub fn overlap_seconds(&self) -> f64 {
        match self {
            MixTransition::Blend { seconds } => *seconds,
            _ => 0.0,
        }
    }

    /// The name both other apps use for this kind of transition.
    pub fn kind(&self) -> &'static str {
        match self {
            MixTransition::Gapless => "gapless",
            MixTransition::Cut => "cut",
            MixTransition::Blend { .. } => "blend",
        }
    }
}

/// What the planner needs to know about one track.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MixTrack {
    /// The album's name, empty when the track has none.
    pub album: String,
    /// Its position on the album, when tagged.
    pub track_number: Option<i64>,
    /// The file's whole length, in seconds.
    pub duration_seconds: Option<f64>,
    /// Where the sound is, when it has been measured.
    pub sound: Option<SoundBounds>,
    /// Beats per minute, when the measurement was confident enough to report one.
    pub bpm: Option<f64>,
    /// The fraction of twenty-second windows that agreed with `bpm`; `None` from
    /// a server too old to report it.
    pub bpm_stability: Option<f64>,
}

impl MixTrack {
    /// How much of this track is sound, in seconds: the measured span where
    /// there is one, the whole file otherwise. A length that is not finite is
    /// no length at all (the desktop's rule, which the phone never meets).
    fn sound_seconds(&self) -> Option<f64> {
        let seconds = match self.sound {
            Some(sound) => (sound.end_ms as f64 / 1000.0 - sound.start_ms as f64 / 1000.0).max(0.0),
            None => self.duration_seconds?,
        };
        seconds.is_finite().then_some(seconds)
    }
}

/// A blend when both tempi agree.
pub const MATCHED_BLEND_SECONDS: f64 = 8.0;
/// A blend when they do not agree, or when nothing was measured.
pub const UNMATCHED_BLEND_SECONDS: f64 = 4.0;
/// No more than this share of either track may be spent overlapping.
const MAXIMUM_SHARE_OF_TRACK: f64 = 0.25;
/// Below this there is not enough song to fade.
const MINIMUM_BLEND_SECONDS: f64 = 2.0;
/// Six per cent: about 7 BPM at 120.
const TEMPO_TOLERANCE: f64 = 0.06;
/// Below this, the BPM is an average of a track that was never at it.
pub const MINIMUM_STABILITY: f64 = 0.5;

/// Plans the handover from `outgoing` to `incoming`.
///
/// `automix` is the Crossfade toggle; off means the player behaves as it always
/// has. `album_lock` keeps album runs intact: two consecutive tracks of one
/// album hand over gaplessly whatever the toggle says.
pub fn plan_transition(
    outgoing: &MixTrack,
    incoming: &MixTrack,
    automix: bool,
    album_lock: bool,
) -> MixTransition {
    if album_lock && is_album_run(outgoing, incoming) {
        return MixTransition::Gapless;
    }
    if !automix {
        return MixTransition::Cut;
    }

    let wanted = if tracks_agree(outgoing, incoming) {
        MATCHED_BLEND_SECONDS
    } else {
        UNMATCHED_BLEND_SECONDS
    };

    // Neither track may give up more than its share, and a track whose length
    // is unknown gives up nothing: better a cut than a fade over a guess.
    let (Some(out_seconds), Some(in_seconds)) =
        (outgoing.sound_seconds(), incoming.sound_seconds())
    else {
        return MixTransition::Cut;
    };

    let allowed = out_seconds.min(in_seconds) * MAXIMUM_SHARE_OF_TRACK;
    let overlap = wanted.min(allowed);
    // A NaN lands on the cut, as in both other apps.
    if overlap.is_nan() || overlap < MINIMUM_BLEND_SECONDS {
        return MixTransition::Cut;
    }
    MixTransition::Blend { seconds: overlap }
}

/// Two tracks of the same named album, in order.
pub fn is_album_run(outgoing: &MixTrack, incoming: &MixTrack) -> bool {
    if outgoing.album.is_empty() || outgoing.album != incoming.album {
        return false;
    }
    match (outgoing.track_number, incoming.track_number) {
        (Some(from), Some(to)) => to == from + 1,
        _ => false,
    }
}

/// Whether a measured tempo describes its track well enough to compare. An
/// unmeasured stability is not held against a track; NaN does not hold.
pub fn tempo_holds(stability: Option<f64>) -> bool {
    match stability {
        None => true,
        Some(stability) => stability >= MINIMUM_STABILITY,
    }
}

fn tracks_agree(outgoing: &MixTrack, incoming: &MixTrack) -> bool {
    if !tempo_holds(outgoing.bpm_stability) || !tempo_holds(incoming.bpm_stability) {
        return false;
    }
    tempi_agree(outgoing.bpm, incoming.bpm)
}

/// Whether two tempi are near enough for a long blend, compared as a ratio
/// folded into one octave: 85 and 170 are the same groove counted twice.
pub fn tempi_agree(a: Option<f64>, b: Option<f64>) -> bool {
    let (Some(a), Some(b)) = (a, b) else {
        return false;
    };
    // NaN fails `is_finite`, so it is refused here too.
    if a <= 0.0 || b <= 0.0 || !a.is_finite() || !b.is_finite() {
        return false;
    }
    let mut ratio = a / b;
    while ratio > 1.5 {
        ratio /= 2.0;
    }
    while ratio < 1.0 / 1.5 {
        ratio *= 2.0;
    }
    (ratio - 1.0).abs() <= TEMPO_TOLERANCE
}

/// The equal-power crossfade shape. Two uncorrelated recordings at half volume
/// each are quieter than one at full volume, so a linear fade dips in the
/// middle; cos² + sin² = 1 keeps the power flat.
pub mod curve {
    use super::FRAC_PI_2;

    fn clamp(progress: f64) -> f64 {
        if !progress.is_finite() {
            return 0.0;
        }
        progress.clamp(0.0, 1.0)
    }

    /// Level for the track on its way out, `progress` running 0 → 1.
    pub fn outgoing(progress: f64) -> f64 {
        (clamp(progress) * FRAC_PI_2).cos()
    }

    /// Level for the track on its way in.
    pub fn incoming(progress: f64) -> f64 {
        (clamp(progress) * FRAC_PI_2).sin()
    }
}
