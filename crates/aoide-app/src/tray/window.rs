//! What the window does around the tray: hidden to it on close or minimise
//! (only while a panel shows the icon), shown again from it, and the tray
//! menu's clicks.

use aoide_mpris::TrayCommand;
use iced::{Task, window};

use super::{Message, TraySettings, transport_for};
use crate::app::{App, Message as AppMessage};

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

/// The first (only) window, for the hide and show calls.
fn with_window(task: fn(window::Id) -> Task<AppMessage>) -> Task<AppMessage> {
    window::oldest().then(move |id| id.map_or_else(Task::none, task))
}

impl App {
    /// A click in the tray's menu.
    pub(super) fn tray_command(&mut self, command: TrayCommand) -> Task<AppMessage> {
        match command {
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
        }
    }

    /// Whether the window was minimised, asked when focus left it.
    pub(super) fn minimised(&mut self, minimised: Option<bool>) -> Task<AppMessage> {
        if minimised == Some(true)
            && on_minimise(&self.config.system.tray, self.system.tray.status.is_shown())
                == WindowAction::HideToTray
        {
            self.hide_window()
        } else {
            Task::none()
        }
    }

    /// A hidden window with no tray on screen to bring it back: show it.
    pub(super) fn unstrand(&mut self) -> Task<AppMessage> {
        if self.system.tray.visible || self.system.tray.status.is_shown() {
            Task::none()
        } else {
            self.show_window()
        }
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
