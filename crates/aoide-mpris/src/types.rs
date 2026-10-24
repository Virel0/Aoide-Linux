//! The crate's own vocabulary. The app maps its player to and from these; nothing
//! here knows about iced, the audio engine or Jellyfin's client types.

use std::time::Duration;

/// Whether something is playing, as MPRIS spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    #[default]
    Stopped,
}

impl PlaybackStatus {
    /// The MPRIS `PlaybackStatus` string.
    pub fn as_mpris(self) -> &'static str {
        match self {
            Self::Playing => "Playing",
            Self::Paused => "Paused",
            Self::Stopped => "Stopped",
        }
    }
}

/// Repeat mode. MPRIS calls it `LoopStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoopStatus {
    /// Play the queue once.
    #[default]
    None,
    /// Repeat the current track.
    Track,
    /// Repeat the whole queue.
    Playlist,
}

impl LoopStatus {
    /// The MPRIS `LoopStatus` string.
    pub fn as_mpris(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Track => "Track",
            Self::Playlist => "Playlist",
        }
    }

    /// Parses an MPRIS `LoopStatus` string. The spec gives exactly three values
    /// and they are case-sensitive; anything else is `None` (the Rust one).
    pub fn from_mpris(value: &str) -> Option<Self> {
        match value {
            "None" => Some(Self::None),
            "Track" => Some(Self::Track),
            "Playlist" => Some(Self::Playlist),
            _ => None,
        }
    }
}

/// What is playing. Strings come from the Jellyfin server and are cleaned
/// before they go on the bus (see [`crate::metadata`]).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Metadata {
    /// The Jellyfin item id. It becomes the MPRIS track id through
    /// [`crate::track_object_path`].
    pub jellyfin_id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub album_artists: Vec<String>,
    pub length: Option<Duration>,
    /// Cover art. Only `https://` and `file://` URLs are published, and an
    /// `https` URL carrying credentials (user info, or an `api_key`/token query
    /// parameter) is dropped: every process on the session bus can read it.
    /// Jellyfin's image endpoints need no token, or the app can pass a cached
    /// `file://` copy.
    pub art_url: Option<String>,
    pub track_number: Option<u32>,
}

/// The whole player as MPRIS sees it. The app sends a fresh one to
/// [`crate::Mpris::update`] whenever anything changes; the crate works out what
/// changed.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerState {
    pub status: PlaybackStatus,
    /// `None` when nothing is loaded.
    pub metadata: Option<Metadata>,
    /// Position in the current track at the moment of the update.
    pub position: Duration,
    /// 0.0 to 1.0.
    pub volume: f64,
    pub shuffle: bool,
    pub loop_status: LoopStatus,
    pub can_go_next: bool,
    pub can_go_previous: bool,
    pub can_play: bool,
    pub can_pause: bool,
    pub can_seek: bool,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            status: PlaybackStatus::Stopped,
            metadata: None,
            position: Duration::ZERO,
            volume: 1.0,
            shuffle: false,
            loop_status: LoopStatus::None,
            can_go_next: false,
            can_go_previous: false,
            can_play: false,
            can_pause: false,
            can_seek: false,
        }
    }
}

impl PlayerState {
    /// The Jellyfin id of the loaded track, if any.
    pub fn track_id(&self) -> Option<&str> {
        self.metadata.as_ref().map(|m| m.jellyfin_id.as_str())
    }
}

/// A request from the desktop (media keys, a shell widget, `playerctl`...).
///
/// Requests the current [`PlayerState`] does not allow (`Next` while
/// `can_go_next` is false, a seek while `can_seek` is false, a `SetPosition` for
/// a track that is no longer current) are dropped before they reach the app, as
/// the MPRIS spec asks.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Play,
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    /// Move by this many microseconds, negative for backwards (MPRIS `Seek`).
    /// The spec asks a seek past the end to act like `Next`.
    Seek(i64),
    /// Jump to this position in the track with this Jellyfin id (MPRIS
    /// `SetPosition`). Already checked to be the current track and within its
    /// length.
    SetPosition(String, Duration),
    /// Already clamped to 0.0..=1.0.
    SetVolume(f64),
    SetShuffle(bool),
    SetLoop(LoopStatus),
    /// Bring the window to the front.
    Raise,
    Quit,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_status_round_trips_through_mpris_strings() {
        for status in [LoopStatus::None, LoopStatus::Track, LoopStatus::Playlist] {
            assert_eq!(LoopStatus::from_mpris(status.as_mpris()), Some(status));
        }
    }

    #[test]
    fn loop_status_rejects_other_spellings() {
        for bad in ["", "none", "TRACK", "playlist", "Repeat", "Track "] {
            assert_eq!(LoopStatus::from_mpris(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn playback_status_strings_match_the_spec() {
        assert_eq!(PlaybackStatus::Playing.as_mpris(), "Playing");
        assert_eq!(PlaybackStatus::Paused.as_mpris(), "Paused");
        assert_eq!(PlaybackStatus::Stopped.as_mpris(), "Stopped");
    }
}
