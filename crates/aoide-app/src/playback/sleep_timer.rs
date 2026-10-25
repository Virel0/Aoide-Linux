//! The sleep timer: pause at the end of this song, at the end of this album,
//! or after a while. A pause, not a stop: Play carries on from there.
//!
//! A state over the clock and the track boundaries the caller passes in, so
//! the rules are tested without a player.

use aoide_audio::EntryId;

/// What the listener picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    EndOfSong,
    EndOfAlbum,
    Minutes(u32),
}

impl Choice {
    /// The menu's timed choices.
    pub const PRESETS: [u32; 4] = [15, 30, 45, 60];
    /// The longest custom timer: twelve hours.
    pub const MAX_MINUTES: u32 = 720;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SleepTimer {
    /// Pause when the next track starts.
    EndOfSong { entry: EntryId },
    /// Pause when a track from another album (or none) starts.
    EndOfAlbum { album: String },
    /// Pause at this time on the app's clock, in milliseconds.
    At { deadline_ms: u64 },
}

/// What to do about the timer when something happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Keep,
    /// Pause now and forget the timer.
    Pause,
    /// Forget the timer: what it waited for won't come.
    Drop,
}

impl SleepTimer {
    /// The timer for `choice`, with what is playing now (its entry and album
    /// id) and the time. An end-of-song or end-of-album timer needs something
    /// playing; an album timer on a track with no album is an end-of-song one.
    pub fn arm(
        choice: Choice,
        current: Option<(EntryId, Option<&str>)>,
        now_ms: u64,
    ) -> Option<Self> {
        match choice {
            Choice::Minutes(minutes) => {
                let minutes = minutes.clamp(1, Choice::MAX_MINUTES);
                Some(SleepTimer::At {
                    deadline_ms: now_ms + u64::from(minutes) * 60_000,
                })
            }
            Choice::EndOfSong => current.map(|(entry, _)| SleepTimer::EndOfSong { entry }),
            Choice::EndOfAlbum => current.map(|(entry, album)| match album {
                Some(album) if !album.is_empty() => SleepTimer::EndOfAlbum {
                    album: album.to_owned(),
                },
                _ => SleepTimer::EndOfSong { entry },
            }),
        }
    }

    /// A track was heard to start: the boundary an end-of-song or
    /// end-of-album timer waits for.
    pub fn track_started(&self, entry: EntryId, album: Option<&str>) -> Verdict {
        match self {
            SleepTimer::EndOfSong { entry: armed } if *armed != entry => Verdict::Pause,
            SleepTimer::EndOfAlbum { album: armed } if album != Some(armed.as_str()) => {
                Verdict::Pause
            }
            _ => Verdict::Keep,
        }
    }

    /// The queue ran out: the music stopped by itself.
    pub fn finished(&self) -> Verdict {
        Verdict::Drop
    }

    /// The clock moved on.
    pub fn tick(&self, now_ms: u64) -> Verdict {
        match self {
            SleepTimer::At { deadline_ms } if now_ms >= *deadline_ms => Verdict::Pause,
            _ => Verdict::Keep,
        }
    }

    /// Whether this timer waits for a boundary between tracks: Crossfade is
    /// held off meanwhile, so there is a boundary to pause on rather than
    /// eight seconds of overlap.
    pub fn waits_for_boundary(&self) -> bool {
        !matches!(self, SleepTimer::At { .. })
    }

    /// Whether the clock needs watching.
    pub fn is_timed(&self) -> bool {
        matches!(self, SleepTimer::At { .. })
    }

    /// What the player bar shows: the time left, or what it waits for.
    pub fn label(&self, now_ms: u64) -> String {
        match self {
            SleepTimer::EndOfSong { .. } => "End of song".to_owned(),
            SleepTimer::EndOfAlbum { .. } => "End of album".to_owned(),
            SleepTimer::At { deadline_ms } => {
                // Rounded up, so "0:00" is never shown while still waiting.
                let seconds = deadline_ms.saturating_sub(now_ms).div_ceil(1_000);
                let (h, m, s) = (seconds / 3_600, seconds / 60 % 60, seconds % 60);
                if h > 0 {
                    format!("{h}:{m:02}:{s:02}")
                } else {
                    format!("{m}:{s:02}")
                }
            }
        }
    }
}

/// A custom timer's minutes as typed: a whole number from 1 to twelve hours.
pub fn parse_minutes(typed: &str) -> Option<u32> {
    typed
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|m| (1..=Choice::MAX_MINUTES).contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: EntryId = EntryId(1);
    const B: EntryId = EntryId(2);

    #[test]
    fn end_of_song_pauses_when_the_next_one_starts() {
        let timer = SleepTimer::arm(Choice::EndOfSong, Some((A, Some("al1"))), 0).unwrap();
        // The same entry starting again (a seek) is not the boundary.
        assert_eq!(timer.track_started(A, Some("al1")), Verdict::Keep);
        assert_eq!(timer.track_started(B, Some("al1")), Verdict::Pause);
        assert_eq!(timer.tick(u64::MAX), Verdict::Keep);
        assert!(timer.waits_for_boundary());
        assert_eq!(timer.label(0), "End of song");
    }

    #[test]
    fn end_of_album_lets_the_album_finish() {
        let timer = SleepTimer::arm(Choice::EndOfAlbum, Some((A, Some("al1"))), 0).unwrap();
        assert_eq!(timer.track_started(B, Some("al1")), Verdict::Keep);
        assert_eq!(timer.track_started(EntryId(3), Some("al2")), Verdict::Pause);
        assert_eq!(timer.track_started(EntryId(3), None), Verdict::Pause);
        assert_eq!(timer.label(0), "End of album");
    }

    #[test]
    fn a_track_with_no_album_ends_at_the_end_of_the_song() {
        assert_eq!(
            SleepTimer::arm(Choice::EndOfAlbum, Some((A, None)), 0),
            Some(SleepTimer::EndOfSong { entry: A })
        );
        assert_eq!(
            SleepTimer::arm(Choice::EndOfAlbum, Some((A, Some(""))), 0),
            Some(SleepTimer::EndOfSong { entry: A })
        );
    }

    #[test]
    fn nothing_playing_has_no_end_to_wait_for() {
        assert_eq!(SleepTimer::arm(Choice::EndOfSong, None, 0), None);
        assert_eq!(SleepTimer::arm(Choice::EndOfAlbum, None, 0), None);
        assert!(SleepTimer::arm(Choice::Minutes(15), None, 0).is_some());
    }

    #[test]
    fn a_timed_timer_counts_down_and_pauses_at_the_deadline() {
        let timer = SleepTimer::arm(Choice::Minutes(15), None, 1_000).unwrap();
        assert_eq!(timer.label(1_000), "15:00");
        assert_eq!(timer.label(1_000 + 60_500), "14:00");
        assert_eq!(timer.label(1_000 + 899_999), "0:01");
        assert_eq!(timer.tick(1_000 + 899_999), Verdict::Keep);
        assert_eq!(timer.tick(1_000 + 900_000), Verdict::Pause);
        // A boundary changes nothing for it.
        assert_eq!(timer.track_started(B, None), Verdict::Keep);
        assert!(!timer.waits_for_boundary());
        assert!(timer.is_timed());
        let long = SleepTimer::arm(Choice::Minutes(90), None, 0).unwrap();
        assert_eq!(long.label(0), "1:30:00");
    }

    #[test]
    fn the_queue_running_out_drops_any_timer() {
        let timer = SleepTimer::arm(Choice::Minutes(30), None, 0).unwrap();
        assert_eq!(timer.finished(), Verdict::Drop);
    }

    #[test]
    fn custom_minutes_are_whole_and_in_range() {
        assert_eq!(parse_minutes(" 20 "), Some(20));
        assert_eq!(parse_minutes("720"), Some(720));
        assert_eq!(parse_minutes("0"), None);
        assert_eq!(parse_minutes("721"), None);
        assert_eq!(parse_minutes("1.5"), None);
        assert_eq!(parse_minutes("soon"), None);
        let clamped = SleepTimer::arm(Choice::Minutes(10_000), None, 0).unwrap();
        assert_eq!(
            clamped,
            SleepTimer::At {
                deadline_ms: 720 * 60_000
            }
        );
    }
}
