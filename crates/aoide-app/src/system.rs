//! Settings → System, and the desktop services behind it: what is kept in
//! the config file ([`SystemSettings`]) and what runs while the app does
//! ([`State`]).

use iced::Element;
use iced::widget::{column, text};
use serde::{Deserialize, Serialize};

use crate::app::{App, Message};
use crate::config::Config;
use crate::integration::Link;
use crate::tray::{Tray, TraySettings};

/// The System settings, one field of the config file.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SystemSettings {
    pub tray: TraySettings,
    /// Discord Rich Presence; off unless turned on.
    pub discord: aoide_integrations::discord::PresenceOptions,
}

/// The desktop services, running beside MPRIS on the integration's runtime.
pub struct State {
    pub tray: Tray,
    /// `None` if the integration's runtime could not be built.
    pub discord: Option<aoide_integrations::discord::Presence>,
    pub discord_status: aoide_integrations::discord::PresenceStatus,
}

impl State {
    /// Starts what the settings turn on.
    pub fn new(desktop: &Link, config: &Config) -> Self {
        let runtime = desktop.runtime().cloned();
        Self {
            discord: crate::discord::start(runtime.as_ref(), config.system.discord.clone()),
            discord_status: aoide_integrations::discord::PresenceStatus::Off,
            tray: Tray::new(runtime, &config.system.tray),
        }
    }
}

impl App {
    /// Hands what plays to the services that show it. Each passes on only a
    /// change, so this is cheap after every message.
    pub(crate) fn publish_system(&self) {
        self.publish_tray();
        self.publish_discord();
    }
}

/// The System sections of Settings, under their heading.
pub fn settings_sections(app: &App) -> Element<'_, Message> {
    column![
        text("System").size(20).font(crate::widgets::BOLD),
        crate::tray::settings_section(&app.config.system.tray, &app.system.tray.status),
        crate::discord::settings_section(&app.config.system.discord, app.system.discord_status),
    ]
    .spacing(24)
    .into()
}
