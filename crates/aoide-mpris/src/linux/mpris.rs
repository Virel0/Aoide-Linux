//! `org.mpris.MediaPlayer2` and `org.mpris.MediaPlayer2.Player` on zbus.
//!
//! The interfaces read the last reported state from a shared [`Tracker`] and
//! turn incoming calls into [`Command`]s on a bounded channel. The app never
//! waits on D-Bus and D-Bus never waits on the app.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use tokio::sync::mpsc::{self, Receiver, Sender, error::TrySendError};
use zbus::fdo::{self, RequestNameFlags};
use zbus::names::InterfaceName;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{Dict, ObjectPath, OwnedValue, Value};
use zbus::{Connection, interface};

use super::builder;
use crate::diff::{Admit, Property, Tracker, admit, set_position_command};
use crate::metadata::{Entry, entries, micros};
use crate::{
    Bus, Command, DESKTOP_ENTRY, Error, IDENTITY, LoopStatus, MPRIS_BUS_NAME, Metadata,
    PlayerState, Result,
};

const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_INTERFACE: InterfaceName<'static> =
    InterfaceName::from_static_str_unchecked("org.mpris.MediaPlayer2.Player");

/// Requests from the desktop waiting for the app. Media keys come one at a
/// time; a client flooding the bus gets an error rather than growing memory.
const COMMAND_QUEUE: usize = 32;

type Shared = Arc<Mutex<Tracker>>;

fn lock(shared: &Shared) -> MutexGuard<'_, Tracker> {
    // The tracker holds plain data; a panic elsewhere cannot leave it torn.
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The MPRIS server. Dropping it leaves the bus.
#[derive(Debug)]
pub struct Mpris {
    connection: Connection,
    shared: Shared,
}

impl Mpris {
    /// Connects to `bus`, serves the MPRIS interfaces and takes the name
    /// `org.mpris.MediaPlayer2.aoide_linux`. Requests arrive on the receiver.
    ///
    /// Must be awaited inside a tokio runtime. Fails with
    /// [`Error::NameTaken`] if another process already holds the name.
    pub async fn start(bus: &Bus) -> Result<(Mpris, Receiver<Command>)> {
        let shared: Shared = Arc::new(Mutex::new(Tracker::new(Instant::now())));
        let (tx, rx) = mpsc::channel(COMMAND_QUEUE);
        let requests = Requests {
            tx,
            shared: shared.clone(),
        };
        let connection = builder(bus)?
            .serve_at(
                MPRIS_PATH,
                Root {
                    requests: requests.clone(),
                },
            )?
            .serve_at(MPRIS_PATH, Player { requests })?
            .build()
            .await?;
        match connection
            .request_name_with_flags(MPRIS_BUS_NAME, RequestNameFlags::DoNotQueue.into())
            .await
        {
            Ok(_) => Ok((Mpris { connection, shared }, rx)),
            Err(zbus::Error::NameTaken) => Err(Error::NameTaken(MPRIS_BUS_NAME)),
            Err(e) => Err(e.into()),
        }
    }

    /// Publishes the player's state. Emits one `PropertiesChanged` with only
    /// the properties that differ from the last update, and `Seeked` if the
    /// position jumped. Calling it on every position tick is fine: steady
    /// playback emits nothing.
    pub async fn update(&self, state: PlayerState) -> Result<()> {
        let (changed, seeked) = {
            let mut tracker = lock(&self.shared);
            let changes = tracker.update(state, Instant::now());
            let state = tracker.state();
            let changed: HashMap<&'static str, Value<'static>> = changes
                .properties
                .iter()
                .map(|&property| (property.name(), property_value(property, state)))
                .collect();
            (changed, changes.seeked)
        };
        let emitter = SignalEmitter::new(&self.connection, MPRIS_PATH)?;
        if !changed.is_empty() {
            fdo::Properties::properties_changed(
                &emitter,
                PLAYER_INTERFACE,
                changed,
                Cow::Borrowed(&[]),
            )
            .await?;
        }
        if let Some(position) = seeked {
            Player::seeked(&emitter, micros(position)).await?;
        }
        Ok(())
    }
}

fn property_value(property: Property, state: &PlayerState) -> Value<'static> {
    match property {
        Property::PlaybackStatus => Value::from(state.status.as_mpris()),
        Property::LoopStatus => Value::from(state.loop_status.as_mpris()),
        Property::Shuffle => Value::from(state.shuffle),
        Property::Metadata => Value::Dict(Dict::from(metadata_map(state.metadata.as_ref()))),
        Property::Volume => Value::from(state.volume),
        Property::CanGoNext => Value::from(state.can_go_next),
        Property::CanGoPrevious => Value::from(state.can_go_previous),
        Property::CanPlay => Value::from(state.can_play),
        Property::CanPause => Value::from(state.can_pause),
        Property::CanSeek => Value::from(state.can_seek),
    }
}

/// The `a{sv}` metadata map; empty when nothing is loaded.
fn metadata_map(metadata: Option<&Metadata>) -> HashMap<String, OwnedValue> {
    metadata
        .map(entries)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(key, entry)| Some((key.to_owned(), entry_value(entry)?)))
        .collect()
}

fn entry_value(entry: Entry) -> Option<OwnedValue> {
    let value = match entry {
        // Always valid: track ids come from `track_object_path`.
        Entry::ObjectPath(path) => Value::from(ObjectPath::try_from(path).ok()?),
        Entry::Str(text) => Value::from(text),
        Entry::StrList(texts) => Value::from(texts),
        Entry::I64(number) => Value::from(number),
        Entry::I32(number) => Value::from(number),
    };
    // Only fails for file descriptors, which metadata never holds.
    OwnedValue::try_from(value).ok()
}

/// What both interfaces share: the last state, to answer reads and check
/// requests, and the way to the app.
#[derive(Debug, Clone)]
struct Requests {
    tx: Sender<Command>,
    shared: Shared,
}

impl Requests {
    fn send(&self, command: Command) -> fdo::Result<()> {
        let admitted = admit(command, lock(&self.shared).state());
        match admitted {
            Admit::Forward(command) => match self.tx.try_send(command) {
                Ok(()) => Ok(()),
                Err(TrySendError::Full(_)) => Err(fdo::Error::LimitsExceeded(
                    "too many requests are waiting".into(),
                )),
                Err(TrySendError::Closed(_)) => {
                    Err(fdo::Error::Failed("the player is shutting down".into()))
                }
            },
            Admit::Ignore => Ok(()),
            Admit::Reject(reason) => Err(fdo::Error::Failed(reason.into())),
        }
    }

    fn read<T>(&self, f: impl FnOnce(&Tracker) -> T) -> T {
        f(&lock(&self.shared))
    }
}

struct Root {
    requests: Requests,
}

#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) -> fdo::Result<()> {
        self.requests.send(Command::Raise)
    }

    fn quit(&self) -> fdo::Result<()> {
        self.requests.send(Command::Quit)
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn can_quit(&self) -> bool {
        true
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn can_raise(&self) -> bool {
        true
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn identity(&self) -> String {
        IDENTITY.to_owned()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn desktop_entry(&self) -> String {
        DESKTOP_ENTRY.to_owned()
    }

    /// Aoide plays its Jellyfin library, not URIs handed to it.
    #[zbus(property(emits_changed_signal = "const"))]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

struct Player {
    requests: Requests,
}

// Writable properties (LoopStatus, Shuffle, Volume) only forward the request:
// the value changes when the app reports it through `Mpris::update`. zbus
// announces the property after each successful set, so a client sees the
// current value first and the new one once the app has applied it.
#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn next(&self) -> fdo::Result<()> {
        self.requests.send(Command::Next)
    }

    fn previous(&self) -> fdo::Result<()> {
        self.requests.send(Command::Previous)
    }

    fn pause(&self) -> fdo::Result<()> {
        self.requests.send(Command::Pause)
    }

    fn play_pause(&self) -> fdo::Result<()> {
        self.requests.send(Command::PlayPause)
    }

    fn stop(&self) -> fdo::Result<()> {
        self.requests.send(Command::Stop)
    }

    fn play(&self) -> fdo::Result<()> {
        self.requests.send(Command::Play)
    }

    fn seek(&self, offset: i64) -> fdo::Result<()> {
        self.requests.send(Command::Seek(offset))
    }

    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) -> fdo::Result<()> {
        // A stale track id, a foreign one or a negative position: the spec
        // says to ignore the call.
        match set_position_command(track_id.as_str(), position) {
            Some(command) => self.requests.send(command),
            None => Ok(()),
        }
    }

    fn open_uri(&self, _uri: String) -> fdo::Result<()> {
        Err(fdo::Error::NotSupported(
            "Aoide plays from its Jellyfin library only".into(),
        ))
    }

    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> String {
        self.requests
            .read(|t| t.state().status.as_mpris().to_owned())
    }

    #[zbus(property)]
    fn loop_status(&self) -> String {
        self.requests
            .read(|t| t.state().loop_status.as_mpris().to_owned())
    }

    #[zbus(property)]
    fn set_loop_status(&self, value: String) -> fdo::Result<()> {
        let status = LoopStatus::from_mpris(&value).ok_or_else(|| {
            fdo::Error::InvalidArgs("LoopStatus is None, Track or Playlist".into())
        })?;
        self.requests.send(Command::SetLoop(status))
    }

    /// Aoide plays at normal speed only.
    #[zbus(property(emits_changed_signal = "const"))]
    fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn set_rate(&self, _rate: f64) -> fdo::Result<()> {
        Ok(())
    }

    #[zbus(property)]
    fn shuffle(&self) -> bool {
        self.requests.read(|t| t.state().shuffle)
    }

    #[zbus(property)]
    fn set_shuffle(&self, shuffle: bool) -> fdo::Result<()> {
        self.requests.send(Command::SetShuffle(shuffle))
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        self.requests
            .read(|t| metadata_map(t.state().metadata.as_ref()))
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.requests.read(|t| t.state().volume)
    }

    #[zbus(property)]
    fn set_volume(&self, volume: f64) -> fdo::Result<()> {
        self.requests.send(Command::SetVolume(volume))
    }

    /// Never announced, as the spec says: clients interpolate, and `Seeked`
    /// tells them when to stop trusting that.
    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        self.requests
            .read(|t| micros(t.position_at(Instant::now())))
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.requests.read(|t| t.state().can_go_next)
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.requests.read(|t| t.state().can_go_previous)
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.requests.read(|t| t.state().can_play)
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.requests.read(|t| t.state().can_pause)
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.requests.read(|t| t.state().can_seek)
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn can_control(&self) -> bool {
        true
    }
}
