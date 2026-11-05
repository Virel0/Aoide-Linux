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
    /// Where the lyrics showing came from, when it's a service to credit:
    /// the track and the service's name.
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
    /// A track's lyrics, and whether the server said pronunciation is off.
    LyricsFound(
        String,
        Result<Option<aoide_melete::LyricsAnswer>, crate::failure::Failure>,
        bool,
    ),
    /// New lyrics are in the panel.
    LyricsShown,
    Translate(bool),
    /// A translation, or why not, and whether the server has none set up.
    Translated(String, Result<Vec<String>, String>, bool),
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
    FullLayout(crate::full_player::Layout),
    /// V in the full player: the next layout.
    NextLayout,
    FullGround(crate::full_player::Ground),
    /// B in the full player: the next ground.
    NextGround,
    /// The playing cover made into the full player's ground (`None` when it
    /// couldn't be read).
    Backdrop(crate::artwork::ArtKey, Option<crate::full_player::Made>),
    /// An album's colours from the server (empty when it has none, or
    /// couldn't be asked), for the full player's ground.
    AlbumPalette(String, Vec<[u8; 3]>),
    /// The full player's clock: its ground moves, its controls may hide.
    FullTick(std::time::Instant),
    /// The pointer moved or a key was pressed over the full player.
    FullStirred,
    FullFocus(bool),
}

/// Messages that come many times a second and change no artwork: the app
/// skips its after-every-message work for them.
pub fn is_quiet(message: &Message) -> bool {
    matches!(
        message,
        Message::Listening(Msg::SleepTick | Msg::FullTick(_) | Msg::FullStirred)
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
            Msg::FullLayout(layout) => self.set_full_layout(layout),
            Msg::NextLayout => self.set_full_layout(self.config.interface.full_layout.next()),
            Msg::FullGround(ground) => self.set_full_ground(ground),
            Msg::NextGround => self.set_full_ground(self.config.interface.full_ground.next()),
            Msg::Backdrop(key, made) => self.backdrop_made(key, made),
            Msg::AlbumPalette(album, palette) => self.palette_arrived(album, palette),
            Msg::FullTick(now) => self.full_tick(now),
            Msg::FullStirred => self.full_stirred(),
            Msg::FullFocus(focused) => self.full_focus(focused),
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
            Msg::LyricsFound(item, result, pronunciation_off) => {
                self.lyrics_found(item, result, pronunciation_off)
            }
            Msg::LyricsShown => self.lyrics_shown(),
            Msg::Translate(on) => self.set_translate(on),
            Msg::Translated(item, result, not_set_up) => {
                self.translated(item, result, not_set_up);
                Task::none()
            }
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

    /// Lyrics found: shown, with their pronunciation, and LRCLIB credited
    /// when they came from it.
    fn lyrics_found(
        &mut self,
        item: String,
        result: Result<Option<aoide_melete::LyricsAnswer>, crate::failure::Failure>,
        pronunciation_off: bool,
    ) -> Task<Message> {
        let answer = result.as_ref().ok().and_then(Option::as_ref);
        self.listening.lyrics_source = answer
            .filter(|a| a.source.as_deref() == Some("lrclib"))
            .map(|_| (item.clone(), "LRCLIB".to_owned()));
        let pronunciations = answer.and_then(|a| a.pronunciations.clone());
        self.pronounced(&item, pronunciations, pronunciation_off);
        Task::done(Message::Lyrics(
            item,
            result.map(|found| found.map(|a| a.lyrics)),
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
