//! Settings → System, and the desktop services behind it: what is kept in
//! the config file ([`SystemSettings`]) and what runs while the app does
//! ([`State`]).

use iced::Element;
use iced::widget::{center, column, container, mouse_area, opaque};
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
    /// Settings → Smart Search's model (the key is in the keyring).
    pub smart_search: crate::smart::SmartSettings,
    /// The update notice: whether to check, and what the last check found.
    pub updates: crate::updates::UpdateSettings,
    /// How much Aoide writes to standard error.
    pub log_level: crate::logging::LogLevel,
}

/// The desktop services, running beside MPRIS on the integration's runtime.
pub struct State {
    pub tray: Tray,
    /// `None` if the integration's runtime could not be built.
    pub discord: Option<aoide_integrations::discord::Presence>,
    pub discord_status: aoide_integrations::discord::PresenceStatus,
    pub updates: crate::updates::State,
    /// The command palette, while it is open.
    pub palette: Option<crate::palette::Palette>,
    pub maintenance: crate::maintenance::State,
}

impl State {
    /// Starts what the settings turn on.
    pub fn new(desktop: &Link, config: &Config) -> Self {
        let runtime = desktop.runtime().cloned();
        Self {
            discord: crate::discord::start(runtime.as_ref(), config.system.discord.clone()),
            discord_status: aoide_integrations::discord::PresenceStatus::Off,
            tray: Tray::new(runtime, &config.system.tray),
            updates: crate::updates::State::default(),
            palette: None,
            maintenance: crate::maintenance::State::default(),
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

/// Settings → System: the tray, Discord, updates, the settings file, logging
/// and the caches.
pub fn settings_sections(app: &App) -> Element<'_, Message> {
    column![
        crate::tray::settings_section(&app.config.system.tray, &app.system.tray.status),
        crate::discord::settings_section(&app.config.system.discord, app.system.discord_status),
        crate::updates::settings_section(&app.config.system.updates, &app.system.updates),
        crate::maintenance::settings_sections(app),
    ]
    .spacing(24)
    .into()
}

/// What is open over the whole window: the command palette, else the
/// release notes.
pub fn overlay(app: &App) -> Option<Element<'_, Message>> {
    crate::palette::view(app).or_else(|| crate::updates::notes(app))
}

/// A card over a dimmed window; a click outside it sends `close`.
pub fn sheet<'a>(card: Element<'a, Message>, close: Message) -> Element<'a, Message> {
    let card = container(card)
        .padding(20)
        .max_width(560)
        .style(crate::theme::menu);
    opaque(mouse_area(center(opaque(card)).padding(40).style(crate::theme::scrim)).on_press(close))
}
