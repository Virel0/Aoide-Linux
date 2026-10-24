//! Desktop integration: **where aoide-mpris plugs in.**
//!
//! The app does not depend on the MPRIS crate yet. When it lands, wiring it in
//! takes three things, all here:
//!
//! 1. [`commands`] returns the crate's command stream (play, pause, next,
//!    previous, stop, seek, volume, shuffle, loop, raise) as a subscription.
//!    The app already turns every [`Command`] into the matching transport call
//!    (`App::desktop` in `app.rs`).
//! 2. [`publish`] hands the crate what is playing whenever it changes. The app
//!    already calls it after every playback change.
//! 3. Single instance: the crate owns the bus name `aoide`; a second launch
//!    that finds it taken should send `Raise` and exit before `main` opens a
//!    window (see the marked spot in `main.rs`).

use aoide_audio::RepeatMode;
use iced::Subscription;

/// What a desktop controller can ask for.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Play,
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    /// Relative, in milliseconds.
    SeekBy(i64),
    /// Absolute, in milliseconds into the current track.
    SetPosition(u64),
    /// 0 to 1.
    Volume(f32),
    Shuffle(bool),
    Loop(RepeatMode),
    /// Bring the window forward (a second launch, or a click in a panel).
    Raise,
}

/// What is playing, as a desktop controller shows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NowPlaying {
    pub track_id: Option<String>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art_url: Option<String>,
    pub length_ms: Option<u64>,
    pub position_ms: u64,
    pub playing: bool,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub volume: f32,
}

/// Commands from the desktop. Nothing yet.
// MPRIS: return the aoide-mpris command stream here, mapped to `Command`.
pub fn commands() -> Subscription<Command> {
    Subscription::none()
}

/// Tells the desktop what is playing. Nothing to tell yet.
// MPRIS: forward `now` to aoide-mpris here (metadata, status, position).
pub fn publish(_now: &NowPlaying) {}
