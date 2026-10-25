//! How the listening goes, past what plays: the player bar's menus, the
//! equaliser's way in, and (as they arrive) the output device, the sleep
//! timer, private listening and the full-screen player.
//!
//! One state and one message type for the app to hold and route, so these
//! features touch the shared files in one place each.

use std::time::Duration;

use aoide_audio::{DeviceChoice, EngineEvent, EntryId, OutputDevice, PlayerSettings};
use iced::{Subscription, Task};

use crate::app::{App, Message};
use crate::playback::sleep_timer::{Choice, SleepTimer, Verdict};
use crate::screens::Route;

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
    /// Private listening: no playback reports to Jellyfin and no play
    /// events in the history while on. Lasts until switched off or quit.
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
    Interface(crate::appearance::InterfaceSettings),
    LyricsFound(String, Result<Option<crate::lyrics_online::Found>, String>),
    /// New lyrics are in the panel.
    LyricsShown,
    Translate(bool),
    /// Only made with the `local-translation` feature.
    #[cfg_attr(not(feature = "local-translation"), allow(dead_code))]
    Translated(String, Result<Vec<String>, String>),
    ModelProgress,
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
        Message::Listening(
            Msg::VisualFrame | Msg::SleepTick | Msg::CheckHidden | Msg::ModelProgress
        )
    )
}

/// What runs at launch.
pub fn boot() -> Task<Message> {
    list_devices()
}

/// cpal's listing blocks (ALSA can take a moment), so off the UI thread.
fn list_devices() -> Task<Message> {
    Task::perform(
        async {
            tokio::task::spawn_blocking(aoide_audio::output_devices)
                .await
                .map_err(|e| e.to_string())
                .and_then(|listed| listed.map_err(|e| e.to_string()))
        },
        |result| Message::Listening(Msg::Devices(result)),
    )
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
        Subscription::batch([
            sleep,
            self.full_player_subscription(),
            self.lyric_extras_subscription(),
        ])
    }

    /// An engine event, for the sleep timer: a boundary to pause on, or the
    /// end of the queue.
    pub(crate) fn sleep_on_event(&mut self, event: &EngineEvent) -> Task<Message> {
        let Some(timer) = &self.listening.sleep else {
            return Task::none();
        };
        let verdict = match event {
            EngineEvent::TrackStarted { id } => {
                let album = self
                    .playback
                    .queued(*id)
                    .and_then(|q| q.meta.album_id.clone());
                timer.track_started(*id, album.as_deref())
            }
            EngineEvent::Finished => timer.finished(),
            _ => Verdict::Keep,
        };
        match (verdict, event) {
            (Verdict::Pause, EngineEvent::TrackStarted { id }) => {
                let entry = *id;
                let paused = self.sleep_pause();
                // Once the pause's fade is over, back to the track's start, so
                // Play begins it rather than a fraction in.
                let rewind =
                    Task::perform(tokio::time::sleep(Duration::from_millis(400)), move |()| {
                        Message::Listening(Msg::SleepRewind(entry))
                    });
                Task::batch([paused, rewind])
            }
            (Verdict::Pause, _) => self.sleep_pause(),
            (Verdict::Drop, _) => self.sleep_set(None),
            (Verdict::Keep, _) => Task::none(),
        }
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

    /// Private listening on or off. On: the song playing is reported
    /// stopped and its play event closed, as if it ended here, and nothing
    /// after that is reported or recorded. Off: reporting and recording
    /// start again with the next song, so a song half heard in private
    /// isn't counted from its middle.
    fn set_private(&mut self, on: bool) -> Task<Message> {
        self.listening.menu = None;
        if on == self.listening.private {
            return Task::none();
        }
        let task = if on {
            let report = self.final_report();
            let task = self.send_reports(report.into_iter().collect());
            let at = aoide_store::now_millis();
            self.store.send(move |w| {
                let _ = w.recorder.stopped(&w.store, at);
            });
            task
        } else {
            if let Some(signed) = self.session.as_mut() {
                signed.reporter = crate::playback::report::Reporter::default();
            }
            Task::none()
        };
        self.listening.private = on;
        task
    }

    /// Pauses for the sleep timer and forgets it.
    fn sleep_pause(&mut self) -> Task<Message> {
        if let Err(error) = self.playback.pause() {
            self.notice = Some(error);
        }
        let report = self.report_pause_state();
        Task::batch([self.sleep_set(None), report])
    }

    /// Sets or clears the timer; the player's settings follow it.
    fn sleep_set(&mut self, timer: Option<SleepTimer>) -> Task<Message> {
        self.listening.sleep = timer;
        let settings = self.player_settings();
        if let Err(error) = self.playback.set_settings(settings) {
            self.notice = Some(error);
        }
        Task::none()
    }

    pub(crate) fn listening(&mut self, msg: Msg) -> Task<Message> {
        match msg {
            Msg::Sleep(choice) => {
                self.listening.menu = None;
                let current = self
                    .playback
                    .current()
                    .map(|(entry, q)| (entry, q.meta.album_id.clone()));
                let timer = SleepTimer::arm(
                    choice,
                    current.as_ref().map(|(e, a)| (*e, a.as_deref())),
                    self.now(),
                );
                if timer.is_none() {
                    self.notice =
                        Some("Nothing is playing, so there is no end to wait for.".into());
                }
                self.sleep_set(timer)
            }
            Msg::OpenFull => self.open_full_player(),
            Msg::CloseFull => self.close_full_player(),
            Msg::MadeFullscreen => {
                if let Some(full) = self.listening.full.as_mut() {
                    full.made_fullscreen = true;
                }
                Task::none()
            }
            Msg::FullShow(show) => {
                if let Some(full) = self.listening.full.as_mut() {
                    full.show = show;
                }
                Task::none()
            }
            Msg::VisualFrame => {
                self.visual_frame();
                Task::none()
            }
            Msg::WindowFocus(focused) => {
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
            Msg::CheckHidden => self.check_hidden(),
            Msg::WindowHidden(hidden) => {
                if let Some(full) = self.listening.full.as_mut() {
                    full.hidden = hidden && !full.focused;
                }
                Task::none()
            }
            Msg::SleepMinutes(typed) => {
                self.listening.sleep_minutes = typed;
                Task::none()
            }
            Msg::SleepCustom => {
                match crate::playback::sleep_timer::parse_minutes(&self.listening.sleep_minutes) {
                    Some(minutes) => {
                        self.listening.sleep_minutes.clear();
                        self.listening(Msg::Sleep(Choice::Minutes(minutes)))
                    }
                    None => Task::none(),
                }
            }
            Msg::SleepCancel => {
                self.listening.menu = None;
                self.sleep_set(None)
            }
            Msg::SleepTick => match &self.listening.sleep {
                Some(timer) if timer.tick(self.now()) == Verdict::Pause => self.sleep_pause(),
                _ => Task::none(),
            },
            Msg::SleepRewind(entry) => {
                let held = self.playback.current().is_some_and(|(e, _)| e == entry)
                    && !self.playback.is_playing();
                if held && let Err(error) = self.playback.seek(0) {
                    self.notice = Some(error);
                }
                Task::none()
            }
            Msg::Menu(menu) => {
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
            Msg::LoadDevices => list_devices(),
            Msg::Private(on) => self.set_private(on),
            Msg::Interface(interface) => self.interface_changed(interface),
            Msg::LyricsFound(item, result) => {
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
            Msg::LyricsShown => self.lyrics_shown(),
            Msg::Translate(on) => self.set_translate(on),
            Msg::Translated(item, result) => {
                self.translated(item, result);
                Task::none()
            }
            Msg::ModelProgress => {
                self.model_progress();
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
            Msg::ChooseDevice(choice) => {
                self.listening.menu = None;
                if choice == self.config.sound.device {
                    return Task::none();
                }
                self.config.sound.device = choice.clone();
                self.save_config();
                if let Err(error) = self.playback.set_device(choice) {
                    self.notice = Some(error);
                }
                Task::none()
            }
            Msg::OpenEqualizer => {
                self.listening.menu = None;
                // Already open on top: leave it.
                let open = self
                    .session
                    .as_ref()
                    .and_then(|s| s.nav.as_ref())
                    .is_some_and(|nav| {
                        matches!(nav.current().page, crate::screens::Page::Equalizer(_))
                    });
                if open {
                    Task::none()
                } else {
                    Task::done(Message::Open(Route::Equalizer))
                }
            }
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
