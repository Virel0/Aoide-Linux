//! The tray icon: aoide-mpris's StatusNotifierItem, run on the desktop
//! integration's runtime beside MPRIS, and what the window does around it
//! (hide to the tray on minimise or close, start hidden).
//!
//! The tray runs only while Settings → System → Show in Tray is on. Its task
//! starts the item, passes on what plays, and sends the menu's clicks and
//! whether a panel shows the icon back to the app as [`Message`]s.
//!
//! The window is only ever hidden to a tray a panel is showing: with no
//! panel (GNOME without the AppIndicator extension, a bare window manager,
//! macOS) a hidden window could not be brought back, so minimise and close
//! then behave as they would without a tray, and a start-minimised launch
//! shows the window after all.

use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use aoide_mpris::{Bus, Pixmap, TrayCommand, TrayIcon, TrayState};
use iced::futures::{Stream, StreamExt};
use iced::widget::{column, toggler};
use iced::{Element, Fill, Subscription, Task, window};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::app::{App, Message as AppMessage, Transport};
use crate::integration::NowPlaying;

/// The icon name the packaged desktop entry installs.
pub const ICON_NAME: &str = "io.github.Virel0.Aoide.Linux";

/// For panels that draw pixmaps rather than follow the icon theme.
const PIXMAPS: [&[u8]; 3] = [
    include_bytes!("../icons/tray-16.png"),
    include_bytes!("../icons/tray-32.png"),
    include_bytes!("../icons/tray-48.png"),
];

/// How often the task asks whether a panel has the icon now (one started,
/// or went away, after Aoide).
const SHOWN_POLL: Duration = Duration::from_secs(5);

/// Clicks waiting for the app.
const QUEUE: usize = 32;

/// The tray's settings, kept in the config file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TraySettings {
    /// Put Aoide's icon in the panel's tray.
    pub show: bool,
    /// Minimising the window hides it to the tray.
    pub minimise_to_tray: bool,
    /// Closing the window hides it to the tray instead of quitting.
    pub close_to_tray: bool,
    /// Launch with the window hidden in the tray.
    pub start_minimised: bool,
}

impl TraySettings {
    /// Whether the window should open hidden. Only with the tray on; if no
    /// panel takes the icon the window is shown once the tray says so.
    pub fn start_hidden(&self) -> bool {
        self.show && self.start_minimised
    }
}

/// What closing or minimising the window does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowAction {
    /// What it would do without a tray: quit on close, stay minimised.
    Default,
    HideToTray,
}

/// Closing the window: hide it only to a tray that is on screen.
pub fn on_close(settings: &TraySettings, shown: bool) -> WindowAction {
    if settings.show && settings.close_to_tray && shown {
        WindowAction::HideToTray
    } else {
        WindowAction::Default
    }
}

/// Minimising the window, the same way.
pub fn on_minimise(settings: &TraySettings, shown: bool) -> WindowAction {
    if settings.show && settings.minimise_to_tray && shown {
        WindowAction::HideToTray
    } else {
        WindowAction::Default
    }
}

/// Where the tray stands, for Settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Off,
    Starting,
    /// A panel shows the icon.
    Shown,
    /// The icon is up but no panel took it.
    NoPanel,
    /// The tray could not start, in words.
    Unavailable(String),
}

impl Status {
    pub fn is_shown(&self) -> bool {
        *self == Status::Shown
    }

    pub fn words(&self) -> &str {
        match self {
            Status::Off => "",
            Status::Starting => "Starting…",
            Status::Shown => "In the panel's tray.",
            Status::NoPanel => {
                "No panel has taken the icon, so the window stays on screen. GNOME shows tray \
                 icons with the AppIndicator extension."
            }
            Status::Unavailable(reason) => reason,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Show(bool),
    MinimiseToTray(bool),
    CloseToTray(bool),
    StartMinimised(bool),
    /// The tray task with this generation started: whether a panel shows
    /// it, or why it could not start.
    Started(u64, Result<bool, String>),
    /// Whether a panel shows it changed.
    Shown(u64, bool),
    Command(TrayCommand),
    /// The window lost focus: whether it was minimised (`None` where the
    /// window system cannot say, as on Wayland).
    Minimised(Option<bool>),
}

/// What the tray shows for what plays.
pub fn tray_state(now: &NowPlaying) -> TrayState {
    let player = crate::integration::player_state(now);
    let title = now.title.trim();
    let artist = now.artist.trim();
    TrayState {
        status: player.status,
        now_playing: now.track_id.as_ref().map(|_| {
            if artist.is_empty() {
                title.to_owned()
            } else {
                format!("{title} — {artist}")
            }
        }),
        can_go_next: player.can_go_next,
        can_go_previous: player.can_go_previous,
        can_play: player.can_play,
    }
}

/// The app's command for a click in the tray's menu. The window commands
/// are the app's to resolve (it knows whether the window shows).
pub fn transport_for(command: TrayCommand) -> Option<Transport> {
    match command {
        TrayCommand::PlayPause => Some(Transport::Toggle),
        TrayCommand::Next => Some(Transport::Next),
        TrayCommand::Previous => Some(Transport::Previous),
        TrayCommand::Stop
        | TrayCommand::OpenWindow
        | TrayCommand::ToggleWindow
        | TrayCommand::Quit => None,
    }
}

fn icon() -> TrayIcon {
    TrayIcon {
        name: ICON_NAME.to_owned(),
        pixmaps: PIXMAPS
            .iter()
            .filter_map(|bytes| {
                let image = image::load_from_memory(bytes).ok()?.to_rgba8();
                Pixmap::from_rgba(image.width(), image.height(), image.as_raw())
            })
            .collect(),
    }
}

/// The tray's events, handed to the subscription once.
#[derive(Clone)]
struct Source {
    receiver: Arc<Mutex<Option<mpsc::Receiver<Message>>>>,
}

impl Hash for Source {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "aoide-tray-events".hash(state);
    }
}

/// The app's end of the tray.
pub struct Tray {
    runtime: Option<tokio::runtime::Handle>,
    events: mpsc::Sender<Message>,
    source: Source,
    running: Option<(JoinHandle<()>, watch::Sender<TrayState>)>,
    generation: u64,
    pub status: Status,
    /// Whether the window is on screen (false while hidden to the tray).
    pub visible: bool,
}

impl Tray {
    /// Starts the tray now if `settings` say so.
    pub fn new(runtime: Option<tokio::runtime::Handle>, settings: &TraySettings) -> Self {
        let (events, receiver) = mpsc::channel(QUEUE);
        let mut tray = Self {
            runtime,
            events,
            source: Source {
                receiver: Arc::new(Mutex::new(Some(receiver))),
            },
            running: None,
            generation: 0,
            status: Status::Off,
            visible: !settings.start_hidden(),
        };
        tray.set_enabled(settings.show);
        tray
    }

    /// Starts or stops the icon.
    pub fn set_enabled(&mut self, on: bool) {
        if on == self.running.is_some() {
            return;
        }
        if let Some((task, _)) = self.running.take() {
            // Dropping the task's `Tray` takes the icon down.
            task.abort();
        }
        self.generation += 1;
        if !on {
            self.status = Status::Off;
            return;
        }
        let Some(runtime) = &self.runtime else {
            self.status = Status::Unavailable("The tray couldn't start.".to_owned());
            return;
        };
        self.status = Status::Starting;
        let (state, states) = watch::channel(TrayState::default());
        let task = runtime.spawn(serve(self.generation, states, self.events.clone()));
        self.running = Some((task, state));
    }

    /// What the tray should show; only a change reaches the bus.
    pub fn update(&self, state: TrayState) {
        if let Some((_, sender)) = &self.running {
            sender.send_if_modified(|last| {
                if *last == state {
                    return false;
                }
                *last = state;
                true
            });
        }
    }

    /// Applies a report from the tray task; stale ones (from a tray since
    /// turned off) are dropped.
    fn report(&mut self, generation: u64, status: Status) {
        if generation == self.generation && self.running.is_some() {
            self.status = status;
        }
    }
}

/// The tray task: starts the icon, passes on states, reports clicks and
/// whether a panel has it. Ends when aborted or when the app goes.
async fn serve(
    generation: u64,
    mut states: watch::Receiver<TrayState>,
    out: mpsc::Sender<Message>,
) {
    let (tray, mut commands) = match aoide_mpris::Tray::start(&Bus::Session, icon()).await {
        Ok(started) => started,
        Err(error) => {
            let reason = match error {
                aoide_mpris::Error::Unavailable => {
                    "The tray needs a Linux desktop with D-Bus.".to_owned()
                }
                other => format!("The tray couldn't start: {other}."),
            };
            let _ = out.send(Message::Started(generation, Err(reason))).await;
            return;
        }
    };
    let first = states.borrow_and_update().clone();
    let _ = tray.update(first).await;
    let mut shown = tray.is_shown();
    if out
        .send(Message::Started(generation, Ok(shown)))
        .await
        .is_err()
    {
        return;
    }
    let mut poll = tokio::time::interval(SHOWN_POLL);
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break };
                if out.send(Message::Command(command)).await.is_err() {
                    break;
                }
            }
            changed = states.changed() => {
                if changed.is_err() {
                    break;
                }
                let state = states.borrow_and_update().clone();
                let _ = tray.update(state).await;
            }
            _ = poll.tick() => {
                let now = tray.is_shown();
                if now != shown {
                    shown = now;
                    if out.send(Message::Shown(generation, now)).await.is_err() {
                        break;
                    }
                }
            }
        }
    }
}

/// The tray's events, as a subscription.
pub fn events(tray: &Tray) -> Subscription<Message> {
    Subscription::run_with(tray.source.clone(), stream_events)
}

fn stream_events(source: &Source) -> impl Stream<Item = Message> + use<> {
    let receiver = source
        .receiver
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    iced::futures::stream::unfold(receiver, |receiver| async move {
        let mut receiver = receiver?;
        let message = receiver.recv().await?;
        Some((message, Some(receiver)))
    })
    .boxed()
}

/// The first (only) window, for the hide and show calls.
fn with_window(task: fn(window::Id) -> Task<AppMessage>) -> Task<AppMessage> {
    window::oldest().then(move |id| id.map_or_else(Task::none, task))
}

impl App {
    pub(crate) fn tray_message(&mut self, message: Message) -> Task<AppMessage> {
        let settings = &mut self.config.system.tray;
        match message {
            Message::Show(on) => {
                settings.show = on;
                self.system.tray.set_enabled(on);
                self.save_config();
                // Turning the tray off must not strand a hidden window.
                if !on && !self.system.tray.visible {
                    return self.show_window();
                }
                Task::none()
            }
            Message::MinimiseToTray(on) => {
                settings.minimise_to_tray = on;
                self.save_config();
                Task::none()
            }
            Message::CloseToTray(on) => {
                settings.close_to_tray = on;
                self.save_config();
                Task::none()
            }
            Message::StartMinimised(on) => {
                settings.start_minimised = on;
                self.save_config();
                Task::none()
            }
            Message::Started(generation, result) => {
                let status = match result {
                    Ok(true) => Status::Shown,
                    Ok(false) => Status::NoPanel,
                    Err(reason) => Status::Unavailable(reason),
                };
                self.system.tray.report(generation, status);
                self.publish_tray();
                self.unstrand()
            }
            Message::Shown(generation, shown) => {
                let status = if shown {
                    Status::Shown
                } else {
                    Status::NoPanel
                };
                self.system.tray.report(generation, status);
                self.unstrand()
            }
            Message::Command(command) => match command {
                TrayCommand::OpenWindow => self.show_window(),
                TrayCommand::ToggleWindow => {
                    if self.system.tray.visible {
                        self.hide_window()
                    } else {
                        self.show_window()
                    }
                }
                TrayCommand::Stop => self.desktop(crate::integration::Command::Stop),
                TrayCommand::Quit => self.quit(),
                other => {
                    transport_for(other).map_or_else(Task::none, |control| self.transport(control))
                }
            },
            Message::Minimised(minimised) => {
                if minimised == Some(true)
                    && on_minimise(&self.config.system.tray, self.system.tray.status.is_shown())
                        == WindowAction::HideToTray
                {
                    self.hide_window()
                } else {
                    Task::none()
                }
            }
        }
    }

    /// A hidden window with no tray on screen to bring it back: show it.
    fn unstrand(&mut self) -> Task<AppMessage> {
        if self.system.tray.visible || self.system.tray.status.is_shown() {
            Task::none()
        } else {
            self.show_window()
        }
    }

    pub(crate) fn publish_tray(&self) {
        self.system
            .tray
            .update(tray_state(&self.desktop.now_playing()));
    }

    /// The window's close button.
    pub(crate) fn close_requested(&mut self) -> Task<AppMessage> {
        match on_close(&self.config.system.tray, self.system.tray.status.is_shown()) {
            WindowAction::HideToTray => self.hide_window(),
            WindowAction::Default => self.quit(),
        }
    }

    /// Focus left the window: if minimise-to-tray applies, ask whether it
    /// was minimised.
    pub(crate) fn window_unfocused(&mut self) -> Task<AppMessage> {
        if on_minimise(&self.config.system.tray, self.system.tray.status.is_shown())
            == WindowAction::Default
        {
            return Task::none();
        }
        window::oldest().then(|id| match id {
            Some(id) => window::is_minimized(id).map(|m| AppMessage::Tray(Message::Minimised(m))),
            None => Task::none(),
        })
    }

    pub(crate) fn show_window(&mut self) -> Task<AppMessage> {
        self.system.tray.visible = true;
        with_window(|id| {
            window::set_mode(id, window::Mode::Windowed)
                .chain(window::minimize(id, false))
                .chain(window::gain_focus(id))
        })
    }

    pub(crate) fn hide_window(&mut self) -> Task<AppMessage> {
        self.system.tray.visible = false;
        with_window(|id| window::set_mode(id, window::Mode::Hidden))
    }
}

/// Settings → System → Tray.
pub fn settings_section<'a>(
    settings: &TraySettings,
    status: &'a Status,
) -> Element<'a, AppMessage> {
    let switch = |label: &'a str, value: bool, message: fn(bool) -> Message| {
        toggler(value)
            .label(label)
            .width(Fill)
            .on_toggle(move |on| AppMessage::Tray(message(on)))
    };
    let mut rows = column![switch("Show in Tray", settings.show, Message::Show)];
    if settings.show {
        rows = rows
            .push(switch(
                "Minimise to Tray",
                settings.minimise_to_tray,
                Message::MinimiseToTray,
            ))
            .push(switch(
                "Close to Tray",
                settings.close_to_tray,
                Message::CloseToTray,
            ))
            .push(switch(
                "Start Minimised",
                settings.start_minimised,
                Message::StartMinimised,
            ));
        if !status.words().is_empty() {
            rows = rows.push(crate::widgets::secondary(status.words()).size(13));
        }
    }
    crate::screens::settings::section(
        "Tray",
        rows,
        "The tray icon plays, pauses and skips, and a click on it shows or hides the window. \
         With Close to Tray, closing the window leaves Aoide playing; Quit in the tray's menu \
         ends it. Minimise to Tray needs a window system that says when a window is minimised, \
         which Wayland doesn't.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoide_audio::RepeatMode;
    use aoide_mpris::PlaybackStatus;

    fn all_on() -> TraySettings {
        TraySettings {
            show: true,
            minimise_to_tray: true,
            close_to_tray: true,
            start_minimised: true,
        }
    }

    #[test]
    fn the_window_hides_only_to_a_tray_on_screen() {
        assert_eq!(on_close(&all_on(), true), WindowAction::HideToTray);
        assert_eq!(on_minimise(&all_on(), true), WindowAction::HideToTray);
        // No panel took the icon: closing quits, minimising stays minimised.
        assert_eq!(on_close(&all_on(), false), WindowAction::Default);
        assert_eq!(on_minimise(&all_on(), false), WindowAction::Default);
        // The tray off overrules the rest.
        let off = TraySettings {
            show: false,
            ..all_on()
        };
        assert_eq!(on_close(&off, true), WindowAction::Default);
        assert_eq!(on_minimise(&off, true), WindowAction::Default);
        assert!(!off.start_hidden());
    }

    #[test]
    fn each_behaviour_has_its_own_switch() {
        let close_only = TraySettings {
            show: true,
            close_to_tray: true,
            ..TraySettings::default()
        };
        assert_eq!(on_close(&close_only, true), WindowAction::HideToTray);
        assert_eq!(on_minimise(&close_only, true), WindowAction::Default);
        assert!(!close_only.start_hidden());
        assert!(all_on().start_hidden());
    }

    #[test]
    fn settings_default_to_no_tray() {
        assert_eq!(TraySettings::default(), serde_json::from_str("{}").unwrap());
        assert!(!TraySettings::default().show);
    }

    #[test]
    fn the_tray_shows_what_plays() {
        let now = NowPlaying {
            track_id: Some("t1".into()),
            title: "Harbour Lights".into(),
            artist: "The Quiet Engines".into(),
            playing: false,
            has_next: true,
            length_ms: Some(1_000),
            repeat: RepeatMode::Off,
            ..NowPlaying::default()
        };
        let state = tray_state(&now);
        assert_eq!(state.status, PlaybackStatus::Paused);
        assert_eq!(
            state.now_playing.as_deref(),
            Some("Harbour Lights — The Quiet Engines")
        );
        assert!(state.can_play && state.can_go_next && state.can_go_previous);
        let no_artist = tray_state(&NowPlaying {
            artist: String::new(),
            ..now
        });
        assert_eq!(no_artist.now_playing.as_deref(), Some("Harbour Lights"));
        let idle = tray_state(&NowPlaying::default());
        assert_eq!(idle.status, PlaybackStatus::Stopped);
        assert_eq!(idle.now_playing, None);
        assert!(!idle.can_play && !idle.can_go_next);
    }

    #[test]
    fn menu_clicks_map_to_the_player() {
        assert_eq!(
            transport_for(TrayCommand::PlayPause),
            Some(Transport::Toggle)
        );
        assert_eq!(transport_for(TrayCommand::Next), Some(Transport::Next));
        assert_eq!(
            transport_for(TrayCommand::Previous),
            Some(Transport::Previous)
        );
        for window in [
            TrayCommand::Stop,
            TrayCommand::OpenWindow,
            TrayCommand::ToggleWindow,
            TrayCommand::Quit,
        ] {
            assert_eq!(transport_for(window), None);
        }
    }

    #[test]
    fn the_icon_has_every_bundled_size() {
        let icon = icon();
        assert_eq!(icon.name, ICON_NAME);
        let sizes: Vec<i32> = icon.pixmaps.iter().map(|p| p.width).collect();
        assert_eq!(sizes, [16, 32, 48]);
        for pixmap in &icon.pixmaps {
            assert_eq!(
                pixmap.argb.len(),
                (pixmap.width * pixmap.height * 4) as usize
            );
        }
    }

    #[tokio::test]
    async fn a_tray_where_there_is_none_says_so_and_stays_out_of_the_way() {
        let runtime = tokio::runtime::Handle::current();
        let mut tray = Tray::new(Some(runtime), &all_on());
        assert!(!tray.visible, "start minimised opens hidden");
        assert_eq!(tray.status, Status::Starting);
        let mut events = tray.source.receiver.lock().unwrap().take().unwrap();
        let Some(Message::Started(generation, result)) = events.recv().await else {
            panic!("the tray reports its start");
        };
        assert_eq!(generation, tray.generation);
        #[cfg(not(target_os = "linux"))]
        assert_eq!(
            result,
            Err("The tray needs a Linux desktop with D-Bus.".to_owned())
        );
        #[cfg(target_os = "linux")]
        let _ = result;
        tray.set_enabled(false);
        assert_eq!(tray.status, Status::Off);
        // A report from the tray just stopped is dropped.
        tray.report(generation, Status::Shown);
        assert_eq!(tray.status, Status::Off);
    }
}
