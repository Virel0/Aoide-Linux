//! Aoide's face to the Linux desktop, over D-Bus:
//!
//! - [`Mpris`]: the `org.mpris.MediaPlayer2` and `.Player` interfaces under the
//!   bus name `org.mpris.MediaPlayer2.aoide`, so media keys, the shell's media
//!   widget and `playerctl` can see and drive the player.
//! - [`single_instance`]: the app's well-known name `io.github.Virel0.Aoide`,
//!   so a second launch raises the first window instead of starting another
//!   player.
//! - [`Inhibitor`]: keeps the machine from suspending while music plays.
//! - [`Tray`]: the tray icon (StatusNotifierItem) with Play/Pause, Next,
//!   Previous, Stop, Open Aoide and Quit; a click on the icon toggles the
//!   window. Its clicks arrive as [`TrayCommand`]s on a channel, the same way
//!   MPRIS requests do.
//!
//! The crate speaks in its own small types ([`PlayerState`], [`Command`],
//! [`TrayState`], [`TrayCommand`]) and
//! knows nothing of iced or the other Aoide crates.
//!
//! # Runtime
//!
//! Everything async here runs on **tokio**: zbus is built with its `tokio`
//! feature, so [`Mpris::start`], [`single_instance`] and [`Inhibitor::new`]
//! must be awaited inside a tokio runtime, and the D-Bus connections they open
//! are served by tasks spawned on that runtime.
//!
//! # Platforms
//!
//! D-Bus is only used on Linux. Elsewhere the crate still builds with the same
//! API: [`Mpris::start`], [`Tray::start`] and [`Inhibitor::new`] return
//! [`Error::Unavailable`],
//! and [`single_instance`] always says this is the first instance.
//!
//! # Tests
//!
//! The pure logic (track ids, change detection, request checks, metadata) is
//! tested on every platform. `tests/dbus.rs` and `tests/tray.rs` drive the
//! real interfaces over a private bus; they are Linux-only and `#[ignore]`d
//! because they start `dbus-daemon`. On Linux (ubuntu-24.04 ships `dbus-daemon`):
//!
//! ```text
//! cargo test -p aoide-mpris -- --include-ignored
//! ```

// Only the Linux side uses these, but they are built and tested everywhere.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod diff;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod metadata;
mod track_id;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod tray;
mod types;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(not(target_os = "linux"))]
mod stub;
#[cfg(not(target_os = "linux"))]
use stub as platform;

pub use diff::SEEK_TOLERANCE;
pub use platform::{Inhibitor, InstanceGuard, Mpris, Tray};
pub use track_id::{NO_TRACK, TRACK_PATH_PREFIX, jellyfin_id_from_track_path, track_object_path};
pub use tray::{Pixmap, TOOLTIP_TITLE, TrayCommand, TrayIcon, TrayState};
pub use types::{Command, LoopStatus, Metadata, PlaybackStatus, PlayerState};

/// The MPRIS bus name.
pub const MPRIS_BUS_NAME: &str = "org.mpris.MediaPlayer2.aoide";
/// What the desktop shows as the player's name.
pub const IDENTITY: &str = "Aoide";
/// The `.desktop` file's basename.
pub const DESKTOP_ENTRY: &str = "io.github.Virel0.Aoide";
/// The app's own well-known name, held by the first instance.
pub const APP_BUS_NAME: &str = "io.github.Virel0.Aoide";
/// The object the first instance serves, with a `Raise` method.
pub const APP_OBJECT_PATH: &str = "/io/github/Virel0/Aoide";
/// The interface of that object.
pub const APP_INTERFACE: &str = "io.github.Virel0.Aoide";

/// Which D-Bus bus to connect to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Bus {
    /// The user's session bus, from `DBUS_SESSION_BUS_ADDRESS`.
    #[default]
    Session,
    /// A bus at this address, such as a private `dbus-daemon` in tests.
    Address(String),
}

/// Whether this process is the first Aoide.
#[derive(Debug)]
pub enum Instance {
    /// This is the first. Keep the guard for the life of the app: dropping it
    /// gives the name up. Each `()` on `raise` is a later launch asking for the
    /// window.
    First {
        guard: InstanceGuard,
        raise: tokio::sync::mpsc::Receiver<()>,
    },
    /// Another Aoide is running; this one should exit. `raised` says whether
    /// that one acknowledged the request to come to the front.
    Other { raised: bool },
}

/// Asks the bus for the app's name. On a platform without D-Bus, always
/// [`Instance::First`].
///
/// If the session bus cannot be reached this is an error; the app can then
/// carry on as if it were first.
pub async fn single_instance(bus: &Bus) -> Result<Instance> {
    platform::single_instance(bus).await
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("D-Bus is not available on this platform")]
    Unavailable,
    #[error("the bus name {0} is already taken")]
    NameTaken(&'static str),
    #[error("no sleep inhibitor answered (tried the desktop portal and the screensaver)")]
    NoInhibitor,
    #[cfg(target_os = "linux")]
    #[error("D-Bus: {0}")]
    DBus(#[from] zbus::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
