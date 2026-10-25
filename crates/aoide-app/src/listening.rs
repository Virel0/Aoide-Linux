//! How the listening goes, past what plays: the player bar's menus, the
//! equaliser's way in, and (as they arrive) the output device, the sleep
//! timer, private listening and the full-screen player.
//!
//! One state and one message type for the app to hold and route, so these
//! features touch the shared files in one place each.

use aoide_audio::{DeviceChoice, OutputDevice};
use iced::Task;

use crate::app::{App, Message};
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
    pub(crate) fn listening(&mut self, msg: Msg) -> Task<Message> {
        match msg {
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
