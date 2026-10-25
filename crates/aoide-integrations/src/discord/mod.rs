//! Discord Rich Presence over Discord's local socket: "Listening to Aoide",
//! the song, the artist, a time bar and an image, on the listener's profile.
//!
//! [`Presence::spawn`] starts a task on the current tokio runtime. The app
//! hands it the options ([`PresenceOptions`]) and what plays
//! ([`NowPlaying`]) whenever either changes; the task does the rest:
//!
//! - **Nothing is sent unless enabled.** While [`PresenceOptions::enabled`] is
//!   off the task opens no socket at all. Turning it off clears the presence
//!   and closes the connection.
//! - It finds Discord's socket (`discord-ipc-0`…`9` under `XDG_RUNTIME_DIR`,
//!   including the Flatpak and snap subdirectories; see
//!   [`ipc::socket_candidates`]), handshakes with Aoide's application id
//!   [`APPLICATION_ID`], and sends `SET_ACTIVITY`.
//! - At most one update a second; a burst of changes sends only the last.
//!   An update Discord would draw the same (the time bar moved by less than
//!   two seconds) is not sent.
//! - If Discord is not running, or quits, it tries again every 15 seconds,
//!   so a Discord started after Aoide picks the presence up.
//!
//! Dropping the [`Presence`] closes the socket, and Discord clears the
//! activity of a client that went away.
//!
//! What leaves the machine, and only to the local Discord client: the
//! song's title, artists and album, the time bar, and, only if the options
//! ask for them, a public cover URL and clean links (see [`Track`]).

mod activity;
pub mod ipc;

use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;

pub use activity::{
    DisplayType, ICON_ASSET, NowPlaying, PAUSED_ASSET, PLAYING_ASSET, PresenceOptions, Track,
    activity,
};

/// Aoide's Discord application id. Public by nature: every client sends it
/// in the handshake, and the other Aoide apps carry the same one.
pub const APPLICATION_ID: &str = "1547687515279069215";

/// Where to look for Discord.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Endpoint {
    /// The usual places, from the environment ([`ipc::socket_candidates`]).
    #[default]
    Discover,
    /// These sockets, in order (tests, or an unusual setup).
    Paths(Vec<PathBuf>),
}

impl Endpoint {
    fn candidates(&self) -> Vec<PathBuf> {
        match self {
            Self::Discover => ipc::socket_candidates(|name| std::env::var_os(name)),
            Self::Paths(paths) => paths.clone(),
        }
    }
}

/// Where the presence stands, for the settings screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceStatus {
    /// Disabled; no socket is open.
    Off,
    /// Enabled, looking for Discord (it may not be running).
    Searching,
    /// Talking to Discord.
    Connected,
    /// This platform has no Discord socket.
    Unsupported,
}

#[derive(Debug, Clone)]
struct Desired {
    options: PresenceOptions,
    now: Option<NowPlaying>,
}

#[derive(Debug, Clone, Copy)]
struct Timing {
    /// Between attempts to find Discord.
    retry: Duration,
    /// Between two `SET_ACTIVITY`s.
    min_interval: Duration,
}

const TIMING: Timing = Timing {
    retry: Duration::from_secs(15),
    min_interval: Duration::from_secs(1),
};

/// The Rich Presence task's handle. See the module docs.
#[derive(Debug)]
pub struct Presence {
    desired: watch::Sender<Desired>,
    status: watch::Receiver<PresenceStatus>,
    task: JoinHandle<()>,
}

impl Drop for Presence {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Presence {
    /// Starts the task with `options`. Must be called inside a tokio
    /// runtime. With `options.enabled` off it waits, connecting to nothing.
    pub fn spawn(endpoint: Endpoint, options: PresenceOptions) -> Presence {
        Self::spawn_with(endpoint, options, TIMING)
    }

    fn spawn_with(endpoint: Endpoint, options: PresenceOptions, timing: Timing) -> Presence {
        let (desired, desired_rx) = watch::channel(Desired { options, now: None });
        let (status_tx, status) = watch::channel(PresenceStatus::Off);
        let task = tokio::spawn(run(endpoint, desired_rx, status_tx, timing));
        Presence {
            desired,
            status,
            task,
        }
    }

    /// New settings; takes effect at once (disabling clears the presence).
    pub fn set_options(&self, options: PresenceOptions) {
        self.desired.send_if_modified(|d| {
            let changed = d.options != options;
            d.options = options;
            changed
        });
    }

    /// What plays now, or `None` when nothing does. Call it on a track change,
    /// pause, resume and seek; calling it more often costs nothing.
    pub fn set_now_playing(&self, now: Option<NowPlaying>) {
        self.desired.send_if_modified(|d| {
            let changed = d.now != now;
            d.now = now;
            changed
        });
    }

    pub fn status(&self) -> PresenceStatus {
        *self.status.borrow()
    }

    /// Follows [`Presence::status`] as it changes.
    pub fn subscribe_status(&self) -> watch::Receiver<PresenceStatus> {
        self.status.clone()
    }
}

#[cfg(unix)]
mod session;
#[cfg(unix)]
use session::run;

#[cfg(not(unix))]
async fn run(
    _endpoint: Endpoint,
    _desired: watch::Receiver<Desired>,
    status: watch::Sender<PresenceStatus>,
    _timing: Timing,
) {
    status.send_replace(PresenceStatus::Unsupported);
}

#[cfg(all(test, unix))]
mod tests;
