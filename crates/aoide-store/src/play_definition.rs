//! What a play is, and what a skip is — in one place, in both languages the store
//! needs them in.
//!
//! A server may count a play the moment playback starts, so a four-second skip counts
//! there exactly as much as a full listen. The classifier that *writes* the flags and
//! the SQL that *recomputes* them are derived from the same constants below, so the
//! count printed beside a track and the membership of "played more than five times"
//! cannot disagree.

/// What a finished play amounted to: a play, a skip, or neither. The middle — a third
/// of a track and moved on — is counted as neither on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlayOutcome {
    /// The track reached its end rather than being skipped away from.
    pub completed: bool,
    /// Qualifies as a play for counting purposes. Not a stored column.
    pub counts_as_play: bool,
    /// Qualifies as a skip: abandoned inside the first fifth.
    pub skipped: bool,
}

/// Where the line between a play and a skip falls: **half the track, or four minutes,
/// whichever comes first**, and a skip only inside the first fifth.
pub struct PlayClassifier;

impl PlayClassifier {
    /// Four minutes of listening is a play however long the track is.
    pub const SCROBBLE_CEILING_MS: i64 = 4 * 60 * 1_000;
    /// A listen of at least the track's length divided by this is a play: half of it.
    pub const PLAY_DIVISOR: i64 = 2;
    /// Below a fifth of a track, the listener rejected it.
    pub const SKIP_DIVISOR: i64 = 5;
    /// How far short of the duration a listen may land and still count as reaching
    /// the end; the last elapsed reading routinely stops a beat early.
    pub const COMPLETION_TOLERANCE_MS: i64 = 1_500;
    /// The smallest number of decided outcomes that makes a skip *rate* mean anything.
    pub const MINIMUM_SKIP_SAMPLE: i64 = 5;

    /// A play needs at least this much listening. Integer division, so the boundary is
    /// an exact millisecond the SQL form reproduces. An unknown or zero duration falls
    /// back to the ceiling alone.
    pub fn play_threshold(track_duration_ms: Option<i64>) -> i64 {
        match track_duration_ms {
            Some(duration) if duration > 0 => {
                (duration / Self::PLAY_DIVISOR).min(Self::SCROBBLE_CEILING_MS)
            }
            _ => Self::SCROBBLE_CEILING_MS,
        }
    }

    /// Below this much listening the track was skipped. With no duration, a fifth of
    /// the ceiling — 48 seconds.
    pub fn skip_threshold(track_duration_ms: Option<i64>) -> i64 {
        match track_duration_ms {
            Some(duration) if duration > 0 => duration / Self::SKIP_DIVISOR,
            _ => Self::SCROBBLE_CEILING_MS / Self::SKIP_DIVISOR,
        }
    }

    /// Classifies a finished play. `completed` is the player's word, which overrides
    /// the arithmetic: a crossfade ends a track for real while leaving elapsed short.
    pub fn classify(
        ms_played: i64,
        track_duration_ms: Option<i64>,
        completed: Option<bool>,
    ) -> PlayOutcome {
        let reached_end = match (completed, track_duration_ms) {
            (Some(completed), _) => completed,
            (None, Some(duration)) if duration > 0 => {
                ms_played >= duration - Self::COMPLETION_TOLERANCE_MS
            }
            // Nothing to have reached the end *of*.
            (None, _) => false,
        };
        let counts_as_play = reached_end || ms_played >= Self::play_threshold(track_duration_ms);
        // The thresholds cannot overlap, so the three outcomes stay exclusive.
        let skipped = !reached_end && ms_played < Self::skip_threshold(track_duration_ms);
        PlayOutcome {
            completed: reached_end,
            counts_as_play,
            skipped,
        }
    }
}

/// The classifier's verdicts for SQLite.
///
/// A stored verdict is honoured where it is decisive — the player had the real asset
/// open, and the track cache's duration may be stale — and the thresholds decide only
/// what the flags leave open, which also judges imported history and rows from older
/// builds that carry `(0, 0)`. The duration comes through a LEFT join to `track`, so an
/// uncached track falls back to the duration-free half of the rule rather than being
/// dropped.
pub(crate) struct PlayDefinition;

impl PlayDefinition {
    /// `MIN` here is SQLite's two-argument scalar minimum. The `CASE` keeps a duration
    /// of `0` routed to the ceiling, as the classifier does.
    pub(crate) fn play_threshold(track: &str) -> String {
        format!(
            "CASE WHEN {track}.durationMs IS NULL OR {track}.durationMs <= 0 \
             THEN {ceiling} \
             ELSE MIN({track}.durationMs / {divisor}, {ceiling}) END",
            ceiling = PlayClassifier::SCROBBLE_CEILING_MS,
            divisor = PlayClassifier::PLAY_DIVISOR,
        )
    }

    pub(crate) fn skip_threshold(track: &str) -> String {
        format!(
            "CASE WHEN {track}.durationMs IS NULL OR {track}.durationMs <= 0 \
             THEN {fallback} \
             ELSE {track}.durationMs / {divisor} END",
            fallback = PlayClassifier::SCROBBLE_CEILING_MS / PlayClassifier::SKIP_DIVISOR,
            divisor = PlayClassifier::SKIP_DIVISOR,
        )
    }

    /// An event `begin_play` opened and nothing closed — the app was killed mid-track.
    /// Its outcome is unknown, and unknown is neither.
    pub(crate) fn has_outcome(event: &str) -> String {
        format!("{event}.endedAt IS NOT NULL")
    }

    /// Counts towards a play count. A stored `skipped = 1` vetoes the threshold arm.
    pub(crate) fn counts_as_play(event: &str, track: &str) -> String {
        format!(
            "({event}.completed = 1 OR ({event}.skipped = 0 AND {outcome} AND {event}.msPlayed >= {threshold}))",
            outcome = Self::has_outcome(event),
            threshold = Self::play_threshold(track),
        )
    }

    /// Counts towards a skip count; disjoint from `counts_as_play` by construction.
    pub(crate) fn counts_as_skip(event: &str, track: &str) -> String {
        format!(
            "({event}.completed = 0 AND ({event}.skipped = 1 OR ({outcome} AND {event}.msPlayed < {threshold})))",
            outcome = Self::has_outcome(event),
            threshold = Self::skip_threshold(track),
        )
    }
}
