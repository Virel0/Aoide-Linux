//! How the listening goes, past what plays: the player bar's menus, the
//! equaliser's way in, and (as they arrive) the output device, the sleep
//! timer, private listening and the full-screen player.
//!
//! One state and one message type for the app to hold and route, so these
//! features touch the shared files in one place each.

use iced::Task;

use crate::app::{App, Message};
use crate::screens::Route;

/// A popover over the player bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarMenu {
    /// The `…` at the end of the bar.
    More,
}

#[derive(Debug, Default)]
pub struct State {
    pub menu: Option<BarMenu>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Opens a popover, or closes whichever is open.
    Menu(Option<BarMenu>),
    OpenEqualizer,
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
}
