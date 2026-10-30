//! Desktop integration through aoide-mpris: MPRIS media controls, single
//! instance and sleep inhibit.
//!
//! aoide-mpris runs on tokio and has to outlive the window, and whether this
//! is the first Aoide must be known before iced opens one. So [`start`] builds
//! a small runtime of its own, asks the bus for the app's name and either says
//! another Aoide is running (`main` has asked it to raise, and exits) or keeps
//! the name and serves MPRIS and the inhibitor on that runtime.
//!
//! The app reaches it through a [`Link`]: [`commands`] is a subscription of
//! what the desktop asks for, already in the app's own [`Command`]s, and
//! [`publish`] hands over what is playing whenever it changes.
//!
//! Where there is no D-Bus (macOS, or a Linux session without a bus) each part
//! is simply missing: nothing is published, nothing arrives, and the app runs
//! as if it were the only one.

use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use aoide_audio::RepeatMode;
use aoide_mpris::{Bus, Inhibitor, Instance, LoopStatus, Metadata, Mpris, PlaybackStatus};
use iced::Subscription;
use iced::futures::{Stream, StreamExt};
use tokio::sync::{mpsc, watch};

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
    /// Close the app, as closing its window does.
    Quit,
}

/// What is playing, as a desktop controller shows it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NowPlaying {
    /// The id of the current track.
    pub track_id: Option<String>,
    pub title: String,
    pub artist: String,
    pub album_artist: Option<String>,
    pub album: String,
    pub track_number: Option<u32>,
    /// A `file://` URL of the cached cover; never a server URL, which would
    /// carry the token to every process on the bus.
    pub art_url: Option<String>,
    pub length_ms: Option<u64>,
    pub position_ms: u64,
    pub playing: bool,
    /// Something follows the current track (Next would move).
    pub has_next: bool,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub volume: f32,
}

/// How many desktop requests may wait for the app.
const COMMAND_QUEUE: usize = 32;

/// The app's end of the integration. Cheap to clone.
#[derive(Clone)]
pub struct Link {
    now: watch::Sender<NowPlaying>,
    commands: CommandSource,
    /// The integration's runtime, for the other desktop services (the tray,
    /// Discord) to run on; `None` if it could not be built.
    runtime: Option<tokio::runtime::Handle>,
}

/// The command channel, handed to the subscription once.
#[derive(Clone)]
struct CommandSource {
    receiver: Arc<Mutex<Option<mpsc::Receiver<Command>>>>,
}

impl Hash for CommandSource {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "aoide-desktop-commands".hash(state);
    }
}

impl Link {
    fn new() -> (Self, watch::Receiver<NowPlaying>, mpsc::Sender<Command>) {
        let (now, now_rx) = watch::channel(NowPlaying::default());
        let (tx, rx) = mpsc::channel(COMMAND_QUEUE);
        let link = Link {
            now,
            commands: CommandSource {
                receiver: Arc::new(Mutex::new(Some(rx))),
            },
            runtime: None,
        };
        (link, now_rx, tx)
    }

    /// The runtime the desktop services run on.
    pub fn runtime(&self) -> Option<&tokio::runtime::Handle> {
        self.runtime.as_ref()
    }

    /// What was last published.
    pub fn now_playing(&self) -> NowPlaying {
        self.now.borrow().clone()
    }

    /// A link to nothing: what a failed start gets.
    pub fn detached() -> Self {
        Self::new().0
    }
}

/// The running integration. Keep it for the life of the program: dropping it
/// stops the runtime, gives up the bus names and lifts the inhibition.
pub struct Desktop {
    _runtime: Option<tokio::runtime::Runtime>,
    pub link: Link,
}

/// What [`start`] found.
pub enum Start {
    /// This is the first Aoide.
    First(Desktop),
    /// Another Aoide is running and was asked to come forward; `raised` says
    /// whether it answered.
    Other { raised: bool },
}

/// Decides single instance, then starts MPRIS and the inhibitor. Call it
/// before iced opens a window. Never fails: what cannot start is left out.
pub fn start(bus: &Bus) -> Start {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("aoide-desktop")
        .enable_all()
        .build();
    let Ok(runtime) = runtime else {
        return Start::First(Desktop {
            _runtime: None,
            link: Link::detached(),
        });
    };
    let (guard, raise) = match runtime.block_on(aoide_mpris::single_instance(bus)) {
        Ok(Instance::Other { raised }) => return Start::Other { raised },
        Ok(Instance::First { guard, raise }) => (Some(guard), Some(raise)),
        // No session bus: nobody else could be found either.
        Err(_) => (None, None),
    };
    let (mut link, now, commands) = Link::new();
    link.runtime = Some(runtime.handle().clone());
    runtime.spawn(serve(bus.clone(), guard, raise, now, commands));
    Start::First(Desktop {
        _runtime: Some(runtime),
        link,
    })
}

/// The bridge, on the integration's runtime: what plays goes to MPRIS and the
/// inhibitor, what the desktop asks goes to the app. Ends with the app.
async fn serve(
    bus: Bus,
    guard: Option<aoide_mpris::InstanceGuard>,
    mut raise: Option<mpsc::Receiver<()>>,
    mut now: watch::Receiver<NowPlaying>,
    commands: mpsc::Sender<Command>,
) {
    // Held here so the name stays ours while the app runs.
    let _guard = guard;
    let (mpris, mut requests) = match Mpris::start(&bus).await {
        Ok((mpris, requests)) => (Some(mpris), Some(requests)),
        Err(_) => (None, None),
    };
    let mut inhibitor = Inhibitor::new(&bus).await.ok();
    let mut current: Option<String> = None;
    let mut inhibiting = false;
    loop {
        tokio::select! {
            Some(request) = next(&mut requests) => {
                if let Some(command) = from_mpris(request, current.as_deref())
                    && commands.send(command).await.is_err()
                {
                    break;
                }
            }
            Some(()) = next(&mut raise) => {
                if commands.send(Command::Raise).await.is_err() {
                    break;
                }
            }
            changed = now.changed() => {
                if changed.is_err() {
                    break;
                }
                let playing = now.borrow_and_update().clone();
                current.clone_from(&playing.track_id);
                if let Some(mpris) = &mpris {
                    let _ = mpris.update(player_state(&playing)).await;
                }
                // Only on a change: a failed inhibit is not retried every tick.
                if playing.playing != inhibiting {
                    inhibiting = playing.playing;
                    if let Some(inhibitor) = inhibitor.as_mut() {
                        let _ = inhibitor.set(inhibiting).await;
                    }
                }
            }
        }
    }
}

/// The next message on a channel that may be missing or closed; a missing one
/// never answers.
async fn next<T>(receiver: &mut Option<mpsc::Receiver<T>>) -> Option<T> {
    let Some(open) = receiver.as_mut() else {
        return std::future::pending().await;
    };
    let message = open.recv().await;
    if message.is_none() {
        *receiver = None;
    }
    message
}

/// Commands from the desktop.
pub fn commands(link: &Link) -> Subscription<Command> {
    Subscription::run_with(link.commands.clone(), stream_commands)
}

fn stream_commands(source: &CommandSource) -> impl Stream<Item = Command> + use<> {
    let receiver = source
        .receiver
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    iced::futures::stream::unfold(receiver, |receiver| async move {
        let mut receiver = receiver?;
        let command = receiver.recv().await?;
        Some((command, Some(receiver)))
    })
    .boxed()
}

/// Tells the desktop what is playing. Only a change is passed on.
pub fn publish(link: &Link, now: &NowPlaying) {
    link.now.send_if_modified(|last| {
        if last == now {
            return false;
        }
        last.clone_from(now);
        true
    });
}

/// The cached cover as a `file://` URL, if it is on disk.
pub fn art_url(cached: &Path) -> Option<String> {
    if !cached.is_file() {
        return None;
    }
    aoide_melete::Url::from_file_path(cached)
        .ok()
        .map(String::from)
}

/// A desktop request as an app command. `current` is the id of what
/// plays now: a position for another track is dropped.
pub fn from_mpris(command: aoide_mpris::Command, current: Option<&str>) -> Option<Command> {
    use aoide_mpris::Command as M;
    Some(match command {
        M::Play => Command::Play,
        M::Pause => Command::Pause,
        M::PlayPause => Command::PlayPause,
        M::Stop => Command::Stop,
        M::Next => Command::Next,
        M::Previous => Command::Previous,
        // Microseconds to milliseconds.
        M::Seek(micros) => Command::SeekBy(micros / 1_000),
        M::SetPosition(track, position) => {
            if current != Some(track.as_str()) {
                return None;
            }
            Command::SetPosition(u64::try_from(position.as_millis()).unwrap_or(u64::MAX))
        }
        M::SetVolume(volume) if volume.is_nan() => return None,
        M::SetVolume(volume) => Command::Volume(volume.clamp(0.0, 1.0) as f32),
        M::SetShuffle(on) => Command::Shuffle(on),
        M::SetLoop(status) => Command::Loop(repeat_mode(status)),
        M::Raise => Command::Raise,
        M::Quit => Command::Quit,
    })
}

pub fn repeat_mode(status: LoopStatus) -> RepeatMode {
    match status {
        LoopStatus::None => RepeatMode::Off,
        LoopStatus::Track => RepeatMode::One,
        LoopStatus::Playlist => RepeatMode::All,
    }
}

pub fn loop_status(mode: RepeatMode) -> LoopStatus {
    match mode {
        RepeatMode::Off => LoopStatus::None,
        RepeatMode::One => LoopStatus::Track,
        RepeatMode::All => LoopStatus::Playlist,
    }
}

/// What is playing, as MPRIS sees it.
pub fn player_state(now: &NowPlaying) -> aoide_mpris::PlayerState {
    let metadata = now.track_id.as_ref().map(|id| Metadata {
        jellyfin_id: id.clone(),
        title: now.title.clone(),
        artists: non_empty(&now.artist).into_iter().collect(),
        album: non_empty(&now.album),
        album_artists: now
            .album_artist
            .as_deref()
            .and_then(non_empty)
            .into_iter()
            .collect(),
        length: now.length_ms.map(Duration::from_millis),
        art_url: now.art_url.clone(),
        track_number: now.track_number,
    });
    let loaded = metadata.is_some();
    aoide_mpris::PlayerState {
        status: match (loaded, now.playing) {
            (false, _) => PlaybackStatus::Stopped,
            (true, true) => PlaybackStatus::Playing,
            (true, false) => PlaybackStatus::Paused,
        },
        metadata,
        position: Duration::from_millis(now.position_ms),
        volume: f64::from(now.volume.clamp(0.0, 1.0)),
        shuffle: now.shuffle,
        loop_status: loop_status(now.repeat),
        can_go_next: loaded && (now.has_next || now.repeat != RepeatMode::Off),
        // Previous restarts the track when there is none before it.
        can_go_previous: loaded,
        can_play: loaded,
        can_pause: loaded,
        can_seek: loaded && now.length_ms.is_some(),
    }
}

fn non_empty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoide_mpris::Command as M;

    fn playing() -> NowPlaying {
        NowPlaying {
            track_id: Some("t1".into()),
            title: "Harbour Lights".into(),
            artist: "The Quiet Engines".into(),
            album_artist: Some("The Quiet Engines".into()),
            album: "Low Tide".into(),
            track_number: Some(3),
            art_url: Some("file:///home/listener/.cache/aoide/art/a.img".into()),
            length_ms: Some(243_500),
            position_ms: 61_250,
            playing: true,
            has_next: true,
            shuffle: false,
            repeat: RepeatMode::Off,
            volume: 0.5,
        }
    }

    #[test]
    fn statuses_follow_what_is_loaded_and_playing() {
        assert_eq!(player_state(&playing()).status, PlaybackStatus::Playing);
        let paused = NowPlaying {
            playing: false,
            ..playing()
        };
        assert_eq!(player_state(&paused).status, PlaybackStatus::Paused);
        let empty = player_state(&NowPlaying::default());
        assert_eq!(empty.status, PlaybackStatus::Stopped);
        assert!(empty.metadata.is_none());
        assert!(!empty.can_play && !empty.can_pause && !empty.can_seek);
        assert!(!empty.can_go_next && !empty.can_go_previous);
    }

    #[test]
    fn metadata_carries_the_track_and_its_id() {
        let state = player_state(&playing());
        let metadata = state.metadata.unwrap();
        assert_eq!(metadata.jellyfin_id, "t1");
        assert_eq!(state.position, Duration::from_millis(61_250));
        assert_eq!(metadata.length, Some(Duration::from_millis(243_500)));
        assert_eq!(metadata.artists, ["The Quiet Engines"]);
        assert_eq!(metadata.album.as_deref(), Some("Low Tide"));
        assert_eq!(metadata.track_number, Some(3));
        assert!(metadata.art_url.unwrap().starts_with("file://"));
        let bare = player_state(&NowPlaying {
            artist: String::new(),
            album: " ".into(),
            album_artist: None,
            ..playing()
        })
        .metadata
        .unwrap();
        assert!(bare.artists.is_empty() && bare.album.is_none() && bare.album_artists.is_empty());
    }

    #[test]
    fn loop_and_repeat_map_both_ways() {
        for (mode, status) in [
            (RepeatMode::Off, LoopStatus::None),
            (RepeatMode::One, LoopStatus::Track),
            (RepeatMode::All, LoopStatus::Playlist),
        ] {
            assert_eq!(loop_status(mode), status);
            assert_eq!(repeat_mode(status), mode);
            assert_eq!(
                from_mpris(M::SetLoop(status), None),
                Some(Command::Loop(mode))
            );
            let state = player_state(&NowPlaying {
                repeat: mode,
                ..playing()
            });
            assert_eq!(state.loop_status, status);
        }
    }

    #[test]
    fn next_is_offered_with_something_after_or_a_repeat() {
        let last = NowPlaying {
            has_next: false,
            ..playing()
        };
        assert!(!player_state(&last).can_go_next);
        assert!(player_state(&last).can_go_previous);
        let looping = NowPlaying {
            repeat: RepeatMode::All,
            ..last
        };
        assert!(player_state(&looping).can_go_next);
        assert!(player_state(&playing()).can_go_next);
    }

    #[test]
    fn shuffle_maps_both_ways() {
        for on in [false, true] {
            assert_eq!(
                from_mpris(M::SetShuffle(on), None),
                Some(Command::Shuffle(on))
            );
            let state = player_state(&NowPlaying {
                shuffle: on,
                ..playing()
            });
            assert_eq!(state.shuffle, on);
        }
    }

    #[test]
    fn volume_is_clamped_both_ways() {
        assert_eq!(
            from_mpris(M::SetVolume(0.25), None),
            Some(Command::Volume(0.25))
        );
        assert_eq!(
            from_mpris(M::SetVolume(1.7), None),
            Some(Command::Volume(1.0))
        );
        assert_eq!(
            from_mpris(M::SetVolume(-0.3), None),
            Some(Command::Volume(0.0))
        );
        assert_eq!(from_mpris(M::SetVolume(f64::NAN), None), None);
        for (given, published) in [(0.5, 0.5), (1.4, 1.0), (-1.0, 0.0)] {
            let state = player_state(&NowPlaying {
                volume: given,
                ..playing()
            });
            assert_eq!(state.volume, published);
        }
    }

    #[test]
    fn seeks_turn_microseconds_into_milliseconds() {
        assert_eq!(
            from_mpris(M::Seek(5_000_000), None),
            Some(Command::SeekBy(5_000))
        );
        assert_eq!(
            from_mpris(M::Seek(-2_500_000), None),
            Some(Command::SeekBy(-2_500))
        );
        assert_eq!(
            from_mpris(
                M::SetPosition("t1".into(), Duration::from_micros(61_250_000)),
                Some("t1")
            ),
            Some(Command::SetPosition(61_250))
        );
        // And back: milliseconds out as a Duration MPRIS sends in microseconds.
        assert_eq!(player_state(&playing()).position.as_micros(), 61_250_000);
    }

    #[test]
    fn a_position_for_another_track_is_dropped() {
        let at = Duration::from_secs(10);
        assert_eq!(
            from_mpris(M::SetPosition("t2".into(), at), Some("t1")),
            None
        );
        assert_eq!(from_mpris(M::SetPosition("t1".into(), at), None), None);
    }

    #[test]
    fn the_rest_map_one_to_one() {
        for (from, to) in [
            (M::Play, Command::Play),
            (M::Pause, Command::Pause),
            (M::PlayPause, Command::PlayPause),
            (M::Stop, Command::Stop),
            (M::Next, Command::Next),
            (M::Previous, Command::Previous),
            (M::Raise, Command::Raise),
            (M::Quit, Command::Quit),
        ] {
            assert_eq!(from_mpris(from, None), Some(to));
        }
    }

    #[test]
    fn only_a_cached_cover_on_disk_becomes_an_art_url() {
        let dir = tempfile::tempdir().unwrap();
        let cover = dir.path().join("a1-tag-480.img");
        assert_eq!(art_url(&cover), None);
        std::fs::write(&cover, b"cover").unwrap();
        let url = art_url(&cover).unwrap();
        assert!(url.starts_with("file:///"), "{url}");
        assert!(url.ends_with("/a1-tag-480.img"), "{url}");
    }

    #[test]
    fn publishing_passes_on_changes_only() {
        let (link, mut now, _commands) = Link::new();
        publish(&link, &playing());
        assert!(now.has_changed().unwrap());
        assert_eq!(now.borrow_and_update().track_id.as_deref(), Some("t1"));
        publish(&link, &playing());
        assert!(!now.has_changed().unwrap());
    }

    #[test]
    fn a_missing_integration_is_not_an_error() {
        // No bus at this address: as on macOS, or a session without D-Bus.
        let bus = Bus::Address("unix:path=/nonexistent/aoide-test-bus".into());
        let Start::First(desktop) = start(&bus) else {
            panic!("with no bus this is the first instance");
        };
        // Publishing to nobody, and a subscription that never fires, are fine.
        publish(&desktop.link, &playing());
        publish(&desktop.link, &NowPlaying::default());
        let _ = commands(&desktop.link);
        drop(desktop);
    }
}
