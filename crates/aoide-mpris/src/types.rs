//! The crate's own vocabulary. The app maps its player to and from these; nothing
//! here knows about iced, the audio engine or Melete's client types.

use std::time::Duration;

/// Whether something is playing, as MPRIS spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaybackStatus {
    /// A track is playing.
    Playing,
    /// A track is loaded and paused.
    Paused,
    /// Nothing plays. The default.
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

/// What is playing. Strings come from the Melete server and are cleaned
/// before they go on the bus: NUL characters, which a D-Bus string can't hold,
/// are removed. Empty fields are left out of the map rather than sent empty.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Metadata {
    /// The Melete track id. It becomes the MPRIS track id through
    /// [`crate::track_object_path`].
    pub track_id: String,
    /// `xesam:title`.
    pub title: String,
    /// `xesam:artist`, in credit order.
    pub artists: Vec<String>,
    /// `xesam:album`.
    pub album: Option<String>,
    /// `xesam:albumArtist`.
    pub album_artists: Vec<String>,
    /// `mpris:length`, sent in microseconds.
    pub length: Option<Duration>,
    /// Cover art. Only `https://` and `file://` URLs are published, and an
    /// `https` URL carrying credentials (user info, or an `api_key`/token query
    /// parameter) is dropped: every process on the session bus can read it.
    /// Melete's covers need the Bearer token, so the app passes a cached
    /// `file://` copy.
    pub art_url: Option<String>,
    /// `xesam:trackNumber`, the track's number on its disc. Left out above
    /// `i32::MAX`.
    pub track_number: Option<u32>,
}

/// The whole player as MPRIS sees it. The app sends a fresh one to
/// [`crate::Mpris::update`] whenever anything changes; the crate works out what
/// changed.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerState {
    /// Playing, paused or stopped.
    pub status: PlaybackStatus,
    /// `None` when nothing is loaded.
    pub metadata: Option<Metadata>,
    /// Position in the current track at the moment of the update.
    pub position: Duration,
    /// 0.0 to 1.0.
    pub volume: f64,
    /// Whether the queue plays shuffled.
    pub shuffle: bool,
    /// The repeat mode.
    pub loop_status: LoopStatus,
    /// There is a next track. False drops [`Command::Next`].
    pub can_go_next: bool,
    /// There is a previous track. False drops [`Command::Previous`].
    pub can_go_previous: bool,
    /// Something is loaded that could play. False drops [`Command::Play`].
    pub can_play: bool,
    /// Playback can be paused. False drops [`Command::Pause`] and refuses
    /// [`Command::PlayPause`].
    pub can_pause: bool,
    /// The track can be sought. False drops [`Command::Seek`] and
    /// [`Command::SetPosition`].
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
    /// The id of the loaded track, if any.
    pub fn track_id(&self) -> Option<&str> {
        self.metadata.as_ref().map(|m| m.track_id.as_str())
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
    /// Start or resume playback.
    Play,
    /// Pause playback.
    Pause,
    /// Pause if playing, play if not.
    PlayPause,
    /// Stop playback.
    Stop,
    /// Skip to the next track.
    Next,
    /// Go back to the previous track.
    Previous,
    /// Move by this many microseconds, negative for backwards (MPRIS `Seek`).
    /// The spec asks a seek past the end to act like `Next`.
    Seek(i64),
    /// Jump to this position in the track with this id (MPRIS
    /// `SetPosition`). Already checked to be the current track and within its
    /// length.
    SetPosition(String, Duration),
    /// Already clamped to 0.0..=1.0.
    SetVolume(f64),
    /// Turn shuffle on or off.
    SetShuffle(bool),
    /// Set the repeat mode.
    SetLoop(LoopStatus),
    /// Bring the window to the front.
    Raise,
    /// Quit the app.
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
