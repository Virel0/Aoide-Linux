//! What changed between two player states: which properties go in
//! `PropertiesChanged`, and whether the position jumped (`Seeked`).
//!
//! MPRIS clients interpolate the position themselves from `PlaybackStatus` and
//! `Rate`, so `Position` never appears in `PropertiesChanged`. They only need
//! `Seeked` when the position moves in a way they could not have predicted.

use std::time::{Duration, Instant};

use crate::track_id::jellyfin_id_from_track_path;
use crate::types::{Command, PlaybackStatus, PlayerState};

/// How far the reported position may drift from the predicted one before it
/// counts as a seek. Covers update jitter and output buffering; a real seek is
/// almost always several seconds.
pub const SEEK_TOLERANCE: Duration = Duration::from_secs(1);

/// A `org.mpris.MediaPlayer2.Player` property that announces its changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Property {
    PlaybackStatus,
    LoopStatus,
    Shuffle,
    Metadata,
    Volume,
    CanGoNext,
    CanGoPrevious,
    CanPlay,
    CanPause,
    CanSeek,
}

impl Property {
    /// The D-Bus property name.
    pub fn name(self) -> &'static str {
        match self {
            Self::PlaybackStatus => "PlaybackStatus",
            Self::LoopStatus => "LoopStatus",
            Self::Shuffle => "Shuffle",
            Self::Metadata => "Metadata",
            Self::Volume => "Volume",
            Self::CanGoNext => "CanGoNext",
            Self::CanGoPrevious => "CanGoPrevious",
            Self::CanPlay => "CanPlay",
            Self::CanPause => "CanPause",
            Self::CanSeek => "CanSeek",
        }
    }
}

/// The signals one update calls for.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Changes {
    /// Properties for one `PropertiesChanged`, empty if none changed.
    pub properties: Vec<Property>,
    /// The new position, if `Seeked` is due.
    pub seeked: Option<Duration>,
}

/// Compares two states `elapsed` apart.
pub fn diff(prev: &PlayerState, next: &PlayerState, elapsed: Duration) -> Changes {
    let mut properties = Vec::new();
    let mut check = |changed: bool, property| {
        if changed {
            properties.push(property);
        }
    };
    check(prev.status != next.status, Property::PlaybackStatus);
    check(prev.loop_status != next.loop_status, Property::LoopStatus);
    check(prev.shuffle != next.shuffle, Property::Shuffle);
    check(prev.metadata != next.metadata, Property::Metadata);
    check(prev.volume != next.volume, Property::Volume);
    check(prev.can_go_next != next.can_go_next, Property::CanGoNext);
    check(
        prev.can_go_previous != next.can_go_previous,
        Property::CanGoPrevious,
    );
    check(prev.can_play != next.can_play, Property::CanPlay);
    check(prev.can_pause != next.can_pause, Property::CanPause);
    check(prev.can_seek != next.can_seek, Property::CanSeek);

    Changes {
        properties,
        seeked: seek(prev, next, elapsed).then_some(next.position),
    }
}

/// Whether `next.position` is one a client interpolating from `prev` could not
/// have predicted.
fn seek(prev: &PlayerState, next: &PlayerState, elapsed: Duration) -> bool {
    if next.status == PlaybackStatus::Stopped || next.metadata.is_none() {
        // No position to speak of.
        return false;
    }
    let was_playing = prev.status == PlaybackStatus::Playing;
    let is_playing = next.status == PlaybackStatus::Playing;

    if prev.track_id() != next.track_id() {
        // A new track. The Metadata change tells clients it started; it is
        // only a seek if it did not start from the top during this interval.
        let latest_start = if is_playing { elapsed } else { Duration::ZERO };
        return next.position > latest_start.saturating_add(SEEK_TOLERANCE);
    }

    // Same track: the position may have advanced by `elapsed` while playing.
    // If it started or stopped playing in between, anything from no progress to
    // a full `elapsed` is consistent.
    let (least, most) = match (was_playing, is_playing) {
        (true, true) => (elapsed, elapsed),
        (false, false) => (Duration::ZERO, Duration::ZERO),
        _ => (Duration::ZERO, elapsed),
    };
    let low = prev
        .position
        .saturating_add(least)
        .saturating_sub(SEEK_TOLERANCE);
    let high = prev
        .position
        .saturating_add(most)
        .saturating_add(SEEK_TOLERANCE);
    next.position < low || next.position > high
}

/// The last state the app reported and when, for diffing the next one and for
/// answering `Position` reads in between.
#[derive(Debug)]
pub struct Tracker {
    state: PlayerState,
    at: Instant,
}

impl Tracker {
    pub fn new(now: Instant) -> Self {
        Self {
            state: PlayerState::default(),
            at: now,
        }
    }

    pub fn state(&self) -> &PlayerState {
        &self.state
    }

    /// Records `next` as of `now` and says what changed.
    pub fn update(&mut self, next: PlayerState, now: Instant) -> Changes {
        let changes = diff(&self.state, &next, now.saturating_duration_since(self.at));
        self.state = next;
        self.at = now;
        changes
    }

    /// The position now: the reported one, advanced while playing, never past
    /// the track's length.
    pub fn position_at(&self, now: Instant) -> Duration {
        let mut position = self.state.position;
        if self.state.status == PlaybackStatus::Playing {
            position = position.saturating_add(now.saturating_duration_since(self.at));
        }
        match self.state.metadata.as_ref().and_then(|m| m.length) {
            Some(length) => position.min(length),
            None => position,
        }
    }
}

/// What to do with a request from the bus.
#[derive(Debug, Clone, PartialEq)]
pub enum Admit {
    /// Pass it to the app.
    Forward(Command),
    /// The spec says it has no effect in this state; reply success and drop it.
    Ignore,
    /// The spec says it has no effect and is an error in this state.
    Reject(&'static str),
}

/// Checks a request against the current state, as the MPRIS spec asks.
pub fn admit(command: Command, state: &PlayerState) -> Admit {
    let allowed = match &command {
        Command::Play => state.can_play,
        Command::Pause => state.can_pause,
        Command::PlayPause => {
            if !state.can_pause {
                return Admit::Reject("CanPause is false");
            }
            true
        }
        Command::Next => state.can_go_next,
        Command::Previous => state.can_go_previous,
        Command::Seek(_) => state.can_seek && state.metadata.is_some(),
        Command::SetPosition(id, position) => {
            state.can_seek
                && state.metadata.as_ref().is_some_and(|m| {
                    m.jellyfin_id == *id && m.length.is_none_or(|length| *position <= length)
                })
        }
        Command::SetVolume(volume) => {
            if volume.is_nan() {
                return Admit::Reject("volume is not a number");
            }
            return Admit::Forward(Command::SetVolume(volume.clamp(0.0, 1.0)));
        }
        Command::Stop
        | Command::SetShuffle(_)
        | Command::SetLoop(_)
        | Command::Raise
        | Command::Quit => true,
    };
    if allowed {
        Admit::Forward(command)
    } else {
        Admit::Ignore
    }
}

/// Turns MPRIS `SetPosition` arguments into a command: the track path must be
/// one of ours and the position not negative (the spec says to ignore those).
pub fn set_position_command(track_path: &str, position_micros: i64) -> Option<Command> {
    let id = jellyfin_id_from_track_path(track_path)?;
    let micros = u64::try_from(position_micros).ok()?;
    Some(Command::SetPosition(id, Duration::from_micros(micros)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track_id::track_object_path;
    use crate::types::{LoopStatus, Metadata};

    fn secs(s: f64) -> Duration {
        Duration::from_secs_f64(s)
    }

    fn playing(id: &str, position: f64) -> PlayerState {
        PlayerState {
            status: PlaybackStatus::Playing,
            metadata: Some(Metadata {
                jellyfin_id: id.into(),
                title: format!("Track {id}"),
                length: Some(secs(240.0)),
                ..Metadata::default()
            }),
            position: secs(position),
            can_play: true,
            can_pause: true,
            can_seek: true,
            can_go_next: true,
            can_go_previous: true,
            ..PlayerState::default()
        }
    }

    fn paused(id: &str, position: f64) -> PlayerState {
        PlayerState {
            status: PlaybackStatus::Paused,
            ..playing(id, position)
        }
    }

    #[test]
    fn no_change_means_no_signals() {
        let s = paused("a", 10.0);
        assert_eq!(diff(&s, &s, secs(5.0)), Changes::default());
    }

    #[test]
    fn steady_playback_is_not_a_seek() {
        // Ticks a second apart, a little early or late.
        let a = playing("a", 10.0);
        for (position, elapsed) in [(11.0, 1.0), (10.8, 1.0), (11.3, 1.0), (70.0, 60.0)] {
            let b = playing("a", position);
            assert_eq!(
                diff(&a, &b, secs(elapsed)),
                Changes::default(),
                "{position}"
            );
        }
    }

    #[test]
    fn a_jump_forward_or_back_is_a_seek() {
        let a = playing("a", 10.0);
        for position in [40.0, 2.0, 0.0, 12.5] {
            let b = playing("a", position);
            assert_eq!(
                diff(&a, &b, secs(1.0)).seeked,
                Some(secs(position)),
                "{position}"
            );
        }
    }

    #[test]
    fn the_tolerance_is_inclusive() {
        let a = playing("a", 10.0);
        assert_eq!(diff(&a, &playing("a", 12.0), secs(1.0)).seeked, None);
        assert_eq!(diff(&a, &playing("a", 10.0), secs(1.0)).seeked, None);
        assert!(diff(&a, &playing("a", 12.01), secs(1.0)).seeked.is_some());
        assert!(diff(&a, &playing("a", 9.99), secs(1.0)).seeked.is_some());
    }

    #[test]
    fn a_seek_while_paused_is_a_seek_and_time_alone_is_not() {
        let a = paused("a", 10.0);
        assert_eq!(diff(&a, &paused("a", 10.0), secs(30.0)).seeked, None);
        assert_eq!(
            diff(&a, &paused("a", 60.0), secs(1.0)).seeked,
            Some(secs(60.0))
        );
    }

    #[test]
    fn pausing_or_resuming_part_way_through_the_interval_is_not_a_seek() {
        let p = playing("a", 10.0);
        // Paused somewhere in the last two seconds.
        let changes = diff(&p, &paused("a", 11.2), secs(2.0));
        assert_eq!(changes.properties, vec![Property::PlaybackStatus]);
        assert_eq!(changes.seeked, None);
        // Resumed somewhere in the last two seconds.
        let changes = diff(&paused("a", 10.0), &playing("a", 10.4), secs(2.0));
        assert_eq!(changes.seeked, None);
        // But resuming somewhere else is.
        assert!(
            diff(&paused("a", 10.0), &playing("a", 30.0), secs(2.0))
                .seeked
                .is_some()
        );
    }

    #[test]
    fn a_new_track_from_the_top_is_not_a_seek() {
        let changes = diff(&playing("a", 238.0), &playing("b", 0.6), secs(1.0));
        assert_eq!(changes.properties, vec![Property::Metadata]);
        assert_eq!(changes.seeked, None);
    }

    #[test]
    fn a_new_track_resumed_part_way_is_a_seek() {
        let changes = diff(&playing("a", 238.0), &playing("b", 95.0), secs(1.0));
        assert_eq!(changes.seeked, Some(secs(95.0)));
        let changes = diff(&playing("a", 10.0), &paused("b", 95.0), secs(1.0));
        assert_eq!(changes.seeked, Some(secs(95.0)));
    }

    #[test]
    fn repeating_the_same_track_is_a_seek() {
        // Loop Track: same id, back to the start.
        let changes = diff(&playing("a", 239.5), &playing("a", 0.3), secs(1.0));
        assert_eq!(changes.properties, vec![]);
        assert_eq!(changes.seeked, Some(secs(0.3)));
    }

    #[test]
    fn stopping_is_never_a_seek() {
        let stopped = PlayerState {
            status: PlaybackStatus::Stopped,
            position: Duration::ZERO,
            ..playing("a", 0.0)
        };
        assert_eq!(diff(&playing("a", 100.0), &stopped, secs(1.0)).seeked, None);
        assert_eq!(
            diff(&playing("a", 100.0), &PlayerState::default(), secs(1.0)).seeked,
            None
        );
    }

    #[test]
    fn only_changed_properties_are_listed() {
        let a = paused("a", 10.0);
        let b = PlayerState {
            volume: 0.5,
            shuffle: true,
            loop_status: LoopStatus::Playlist,
            can_go_next: false,
            ..a.clone()
        };
        assert_eq!(
            diff(&a, &b, secs(0.1)).properties,
            vec![
                Property::LoopStatus,
                Property::Shuffle,
                Property::Volume,
                Property::CanGoNext
            ]
        );
    }

    #[test]
    fn every_property_is_detected() {
        let a = paused("a", 10.0);
        let mut b = playing("b", 0.0);
        b.loop_status = LoopStatus::Track;
        b.shuffle = true;
        b.volume = 0.2;
        b.can_go_next = false;
        b.can_go_previous = false;
        b.can_play = false;
        b.can_pause = false;
        b.can_seek = false;
        let names: Vec<_> = diff(&a, &b, secs(0.1))
            .properties
            .into_iter()
            .map(Property::name)
            .collect();
        assert_eq!(
            names,
            [
                "PlaybackStatus",
                "LoopStatus",
                "Shuffle",
                "Metadata",
                "Volume",
                "CanGoNext",
                "CanGoPrevious",
                "CanPlay",
                "CanPause",
                "CanSeek"
            ]
        );
    }

    #[test]
    fn a_metadata_change_within_one_track_is_announced() {
        let a = playing("a", 10.0);
        let mut b = playing("a", 11.0);
        b.metadata.as_mut().unwrap().art_url = Some("file:///cache/a.jpg".into());
        assert_eq!(diff(&a, &b, secs(1.0)).properties, vec![Property::Metadata]);
    }

    #[test]
    fn the_tracker_measures_elapsed_time_itself() {
        let t0 = Instant::now();
        let mut tracker = Tracker::new(t0);
        tracker.update(playing("a", 10.0), t0);
        let steady = tracker.update(playing("a", 15.0), t0 + secs(5.0));
        assert_eq!(steady, Changes::default());
        let jump = tracker.update(playing("a", 120.0), t0 + secs(6.0));
        assert_eq!(jump.seeked, Some(secs(120.0)));
    }

    #[test]
    fn the_tracker_interpolates_position_while_playing_only() {
        let t0 = Instant::now();
        let mut tracker = Tracker::new(t0);
        tracker.update(playing("a", 10.0), t0);
        assert_eq!(tracker.position_at(t0 + secs(2.5)), secs(12.5));
        // Never past the end of the track.
        assert_eq!(tracker.position_at(t0 + secs(1000.0)), secs(240.0));
        tracker.update(paused("a", 20.0), t0 + secs(3.0));
        assert_eq!(tracker.position_at(t0 + secs(100.0)), secs(20.0));
    }

    #[test]
    fn the_first_update_announces_everything_that_differs_from_stopped() {
        let t0 = Instant::now();
        let mut tracker = Tracker::new(t0);
        let changes = tracker.update(playing("a", 0.0), t0);
        assert!(changes.properties.contains(&Property::PlaybackStatus));
        assert!(changes.properties.contains(&Property::Metadata));
        assert_eq!(changes.seeked, None);
    }

    #[test]
    fn requests_the_state_does_not_allow_are_dropped() {
        let none = PlayerState::default();
        for command in [
            Command::Play,
            Command::Pause,
            Command::Next,
            Command::Previous,
            Command::Seek(5_000_000),
            Command::SetPosition("a".into(), secs(1.0)),
        ] {
            assert_eq!(admit(command.clone(), &none), Admit::Ignore, "{command:?}");
        }
        assert!(matches!(admit(Command::PlayPause, &none), Admit::Reject(_)));
    }

    #[test]
    fn requests_the_state_allows_are_forwarded() {
        let state = playing("a", 10.0);
        for command in [
            Command::Play,
            Command::Pause,
            Command::PlayPause,
            Command::Stop,
            Command::Next,
            Command::Previous,
            Command::Seek(-5_000_000),
            Command::SetPosition("a".into(), secs(30.0)),
            Command::SetShuffle(true),
            Command::SetLoop(LoopStatus::Playlist),
            Command::Raise,
            Command::Quit,
        ] {
            assert_eq!(
                admit(command.clone(), &state),
                Admit::Forward(command.clone()),
                "{command:?}"
            );
        }
    }

    #[test]
    fn set_position_for_another_track_or_past_the_end_is_ignored() {
        let state = playing("a", 10.0);
        assert_eq!(
            admit(Command::SetPosition("b".into(), secs(30.0)), &state),
            Admit::Ignore
        );
        assert_eq!(
            admit(Command::SetPosition("a".into(), secs(241.0)), &state),
            Admit::Ignore
        );
        assert_eq!(
            admit(Command::SetPosition("a".into(), secs(240.0)), &state),
            Admit::Forward(Command::SetPosition("a".into(), secs(240.0)))
        );
    }

    #[test]
    fn volume_is_clamped_and_nan_rejected() {
        let state = PlayerState::default();
        assert_eq!(
            admit(Command::SetVolume(-0.5), &state),
            Admit::Forward(Command::SetVolume(0.0))
        );
        assert_eq!(
            admit(Command::SetVolume(3.0), &state),
            Admit::Forward(Command::SetVolume(1.0))
        );
        assert_eq!(
            admit(Command::SetVolume(0.25), &state),
            Admit::Forward(Command::SetVolume(0.25))
        );
        assert!(matches!(
            admit(Command::SetVolume(f64::NAN), &state),
            Admit::Reject(_)
        ));
    }

    #[test]
    fn set_position_arguments_are_decoded() {
        let path = track_object_path("abc");
        assert_eq!(
            set_position_command(&path, 1_500_000),
            Some(Command::SetPosition("abc".into(), secs(1.5)))
        );
        assert_eq!(set_position_command(&path, -1), None);
        assert_eq!(
            set_position_command("/org/mpris/MediaPlayer2/TrackList/NoTrack", 0),
            None
        );
    }
}
