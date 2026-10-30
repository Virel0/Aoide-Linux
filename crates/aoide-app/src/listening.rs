//! How the listening goes, past what plays: the player bar's menus, the
//! equaliser's way in, and (as they arrive) the output device, the sleep
//! timer, private listening and the full-screen player.
//!
//! One state and one message type for the app to hold and route, so these
//! features touch the shared files in one place each.

use std::time::Duration;

use aoide_audio::{DeviceChoice, EntryId, OutputDevice, PlayerSettings};
use iced::{Subscription, Task};

use crate::app::{App, Message};
use crate::playback::sleep_timer::{Choice, SleepTimer};
use crate::screens::Route;

mod output;
mod sleep;

use output::list_devices;

/// A popover over the player bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarMenu {
    /// The `…` at the end of the bar.
    More,
    /// The volume button's: the output device.
    Volume,
}

#[derive(Debug, Default)]
pub struct State {
    pub menu: Option<BarMenu>,
    /// The output devices, as last listed; `None` before the first listing.
    pub devices: Option<Result<Vec<OutputDevice>, String>>,
    pub sleep: Option<SleepTimer>,
    /// The custom sleep timer's minutes, as typed.
    pub sleep_minutes: String,
    /// The full-screen player, while it is open.
    pub full: Option<crate::full_player::FullPlayer>,
    /// Private listening: no play events in the history (so none reach
    /// Melete) while on. Lasts until switched off or quit.
    pub private: bool,
    /// The internet lyrics finder, made when first needed.
    pub finder: Option<std::sync::Arc<aoide_lyrics::LyricsFinder>>,
    /// Where the lyrics showing came from, when not the server: the track
    /// and the provider's name.
    pub lyrics_source: Option<(String, String)>,
    /// The lyrics' language and their translation.
    pub lyric_extras: crate::lyrics_extras::State,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Opens a popover, or closes whichever is open.
    Menu(Option<BarMenu>),
    OpenEqualizer,
    /// List the output devices again (a picker is opening).
    LoadDevices,
    Devices(Result<Vec<OutputDevice>, String>),
    ChooseDevice(DeviceChoice),
    Transcode(crate::sound::Transcode),
    Private(bool),
    /// What the listener is doing, for the next plays; `None` untags.
    Activity(Option<aoide_store::Activity>),
    Interface(crate::appearance::InterfaceSettings),
    LyricsFound(
        String,
        Result<Option<crate::lyrics_online::Found>, crate::failure::Failure>,
    ),
    /// New lyrics are in the panel.
    LyricsShown,
    Translate(bool),
    Translated(String, Result<Vec<String>, String>),
    /// Settings → Lyrics → Translation.
    Server(crate::lyrics_extras::ServerMsg),
    Sleep(Choice),
    SleepMinutes(String),
    SleepCustom,
    SleepCancel,
    SleepTick,
    /// Paused at a boundary: hold the new track at its start.
    SleepRewind(EntryId),
    OpenFull,
    CloseFull,
    /// The full player took the window full screen.
    MadeFullscreen,
    FullShow(crate::full_player::Show),
    VisualFrame,
    WindowFocus(bool),
    CheckHidden,
    WindowHidden(bool),
}

/// Messages that come many times a second and change no artwork: the app
/// skips its after-every-message work for them.
pub fn is_quiet(message: &Message) -> bool {
    matches!(
        message,
        Message::Listening(Msg::VisualFrame | Msg::SleepTick | Msg::CheckHidden)
    )
}

/// What runs at launch.
pub fn boot() -> Task<Message> {
    list_devices()
}

impl App {
    /// What the player plays by: the settings, with Crossfade held off while
    /// the sleep timer waits for the end of a song or album.
    pub(crate) fn player_settings(&self) -> PlayerSettings {
        let mut settings = self.config.settings.player();
        if self
            .listening
            .sleep
            .as_ref()
            .is_some_and(SleepTimer::waits_for_boundary)
        {
            settings.crossfade = false;
        }
        settings
    }

    /// What the app listens for past the shared subscriptions: the sleep
    /// timer's clock, while it runs.
    pub(crate) fn listening_subscription(&self) -> Subscription<Message> {
        let sleep = match &self.listening.sleep {
            Some(timer) if timer.is_timed() => iced::time::every(Duration::from_secs(1))
                .map(|_| Message::Listening(Msg::SleepTick)),
            _ => Subscription::none(),
        };
        Subscription::batch([sleep, self.full_player_subscription()])
    }

    /// What a lyrics lookup needs past the server: the finder and its
    /// cache, or `None` while internet lyrics are off.
    pub(crate) fn lyrics_online(&mut self) -> Option<crate::lyrics_online::Online> {
        if !self.config.interface.internet_lyrics {
            return None;
        }
        if self.listening.finder.is_none() {
            self.listening.finder = crate::lyrics_online::finder();
        }
        Some(crate::lyrics_online::Online {
            finder: self.listening.finder.clone()?,
            dir: self
                .paths
                .as_ref()
                .and_then(crate::lyrics_online::cache_dir),
        })
    }

    /// Private listening on or off. On: the song playing has its play event
    /// closed, as if it ended here, and nothing after that is recorded. Off:
    /// recording starts again with the next song, so a song half heard in
    /// private isn't counted from its middle.
    fn set_private(&mut self, on: bool) -> Task<Message> {
        self.listening.menu = None;
        if on == self.listening.private {
            return Task::none();
        }
        if on {
            let at = aoide_store::now_millis();
            self.store.send(move |w| {
                let _ = w.recorder.stopped(&w.store, at);
            });
        }
        self.listening.private = on;
        Task::none()
    }

    pub(crate) fn listening(&mut self, msg: Msg) -> Task<Message> {
        match msg {
            Msg::Sleep(choice) => self.arm_sleep(choice),
            Msg::OpenFull => self.open_full_player(),
            Msg::CloseFull => self.close_full_player(),
            Msg::MadeFullscreen => self.with_full(|full| full.made_fullscreen = true),
            Msg::FullShow(show) => self.with_full(|full| full.show = show),
            Msg::VisualFrame => {
                self.visual_frame();
                Task::none()
            }
            Msg::WindowFocus(focused) => self.window_focus(focused),
            Msg::CheckHidden => self.check_hidden(),
            Msg::WindowHidden(hidden) => {
                self.with_full(|full| full.hidden = hidden && !full.focused)
            }
            Msg::SleepMinutes(typed) => {
                self.listening.sleep_minutes = typed;
                Task::none()
            }
            Msg::SleepCustom => self.sleep_custom(),
            Msg::SleepCancel => {
                self.listening.menu = None;
                self.sleep_set(None)
            }
            Msg::SleepTick => self.sleep_tick(),
            Msg::SleepRewind(entry) => self.sleep_rewind(entry),
            Msg::Menu(menu) => self.toggle_menu(menu),
            Msg::LoadDevices => list_devices(),
            Msg::Private(on) => self.set_private(on),
            Msg::Activity(activity) => {
                self.listening.menu = None;
                self.config.activity = activity.map(|a| a.as_str().to_owned());
                self.save_config();
                Task::none()
            }
            Msg::Interface(interface) => self.interface_changed(interface),
            Msg::LyricsFound(item, result) => self.lyrics_found(item, result),
            Msg::LyricsShown => self.lyrics_shown(),
            Msg::Translate(on) => self.set_translate(on),
            Msg::Translated(item, result) => {
                self.translated(item, result);
                Task::none()
            }
            Msg::Server(msg) => self.translation_server(msg),
            Msg::Transcode(transcode) => {
                self.config.sound.transcode = transcode.sanitised();
                self.save_config();
                Task::none()
            }
            Msg::Devices(result) => {
                self.listening.devices = Some(result);
                Task::none()
            }
            Msg::ChooseDevice(choice) => self.choose_device(choice),
            Msg::OpenEqualizer => self.open_equalizer(),
        }
    }

    /// Changes the full player, if it is open.
    fn with_full(
        &mut self,
        change: impl FnOnce(&mut crate::full_player::FullPlayer),
    ) -> Task<Message> {
        if let Some(full) = self.listening.full.as_mut() {
            change(full);
        }
        Task::none()
    }

    /// The window gained or lost focus while the full player is open: out of
    /// focus, it may have been minimised.
    fn window_focus(&mut self, focused: bool) -> Task<Message> {
        if let Some(full) = self.listening.full.as_mut() {
            full.focused = focused;
            if focused {
                full.hidden = false;
            }
        }
        if focused {
            Task::none()
        } else {
            self.check_hidden()
        }
    }

    /// Opens a popover, or closes the one open; the volume's lists the
    /// output devices again.
    fn toggle_menu(&mut self, menu: Option<BarMenu>) -> Task<Message> {
        self.listening.menu = if menu == self.listening.menu {
            None
        } else {
            menu
        };
        if self.listening.menu == Some(BarMenu::Volume) {
            list_devices()
        } else {
            Task::none()
        }
    }

    /// Lyrics found (on the server or online): shown, with where they came
    /// from when not the server.
    fn lyrics_found(
        &mut self,
        item: String,
        result: Result<Option<crate::lyrics_online::Found>, crate::failure::Failure>,
    ) -> Task<Message> {
        self.listening.lyrics_source = match &result {
            Ok(Some(crate::lyrics_online::Found {
                provider: Some(provider),
                ..
            })) => Some((item.clone(), provider.clone())),
            _ => None,
        };
        Task::done(Message::Lyrics(
            item,
            result.map(|found| found.map(|f| f.lyrics)),
        ))
        .chain(Task::done(Message::Listening(Msg::LyricsShown)))
    }

    fn open_equalizer(&mut self) -> Task<Message> {
        self.listening.menu = None;
        // Already open on top: leave it.
        let open = self
            .session
            .as_ref()
            .and_then(|s| s.nav.as_ref())
            .is_some_and(|nav| matches!(nav.current().page, crate::screens::Page::Equalizer(_)));
        if open {
            Task::none()
        } else {
            Task::done(Message::Open(Route::Equalizer))
        }
    }
}

/// What the Settings page draws past its Round 1 rows.
pub struct SettingsExtras<'a> {
    pub config: &'a crate::config::Config,
    pub listening: &'a State,
    /// The chosen device couldn't be opened; the default plays.
    pub device_fell_back: bool,
}
