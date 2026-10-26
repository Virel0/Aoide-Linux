//! Settings → Lyrics → Translation: the LibreTranslate server's address,
//! checked when saved, and its API key, kept in the keyring and forgotten
//! when the address changes.

use std::sync::Arc;

use aoide_integrations::translate::libre::LibreTranslate;
use aoide_jellyfin::ServerAddress;
use iced::widget::{button, column, row, text, text_input};
use iced::{Center, Element, Fill, Task};

use super::KEY_ENTRY;
use crate::app::{App, Message};
use crate::credentials::CredentialStore;
use crate::listening::{Msg, SettingsExtras};
use crate::smart::Draft;
use crate::{theme, widgets};

/// The translation server's rows in Settings, as they are being edited.
#[derive(Debug, Default)]
pub struct ServerSettings {
    /// The address as typed; `None` until typed in, when the saved one shows.
    pub address: Option<String>,
    pub key: Draft,
    /// Whether the keyring holds a key; `None` before it has been asked.
    pub key_set: Option<bool>,
    /// What the last check of the server found, or why it failed.
    pub check: Option<Result<String, String>>,
    pub busy: bool,
}

/// Settings → Lyrics → Translation.
#[derive(Debug, Clone)]
pub enum ServerMsg {
    AddressTyped(String),
    SaveAddress,
    Checked(Result<usize, String>),
    KeyTyped(Draft),
    SaveKey,
    ClearKey,
    /// The keyring answered: whether a key is there now.
    KeyStored(Result<bool, String>),
}

/// What the address field holds, as the server's URL; `Ok(None)` for an
/// empty field, which turns translation off.
pub fn parse_address(typed: &str) -> Result<Option<String>, String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return Ok(None);
    }
    ServerAddress::parse(typed)
        .map(|address| Some(address.as_str().to_owned()))
        .map_err(|_| "That isn't an address Aoide can use.".to_owned())
}

/// The key from the keyring, off the async threads. A keyring that can't
/// be read is treated as no key; a server that wants one will say so.
pub(super) async fn read_key(keyring: Arc<dyn CredentialStore>) -> Option<String> {
    tokio::task::spawn_blocking(move || keyring.get(KEY_ENTRY).ok().flatten())
        .await
        .ok()
        .flatten()
}

/// Whether saving `new` over `old` forgets the key: any change of a set
/// address, to another or to none. Setting the first one keeps a key saved
/// before it.
pub fn forgets_key(old: Option<&str>, new: Option<&str>) -> bool {
    old.is_some() && old != new
}

/// Removes the key from the keyring, off the async threads.
async fn forget(keyring: Arc<dyn CredentialStore>) -> Result<(), String> {
    tokio::task::spawn_blocking(move || keyring.delete(KEY_ENTRY))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r.map_err(|e| e.to_string()))
}

/// Forgets the key, and says so to Settings.
fn forget_key(keyring: Arc<dyn CredentialStore>) -> Task<Message> {
    Task::perform(forget(keyring), |result| {
        Message::Listening(Msg::Server(ServerMsg::KeyStored(result.map(|()| false))))
    })
}

/// Whether the keyring holds a key, for Settings.
pub fn key_state(keyring: Arc<dyn CredentialStore>) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || keyring.get(KEY_ENTRY).map(|k| k.is_some()))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()))
        },
        |result| Message::Listening(Msg::Server(ServerMsg::KeyStored(result))),
    )
}

impl App {
    /// Settings → Lyrics → Translation.
    pub(crate) fn translation_server(&mut self, msg: ServerMsg) -> Task<Message> {
        let state = &mut self.listening.lyric_extras.settings;
        match msg {
            ServerMsg::AddressTyped(typed) => {
                state.address = Some(typed);
                Task::none()
            }
            ServerMsg::SaveAddress => self.save_server_address(),
            ServerMsg::Checked(result) => {
                state.busy = false;
                state.check = Some(match result {
                    Ok(0) => Err("It answered, but has no languages installed.".to_owned()),
                    Ok(n) => Ok(format!("Connected. It translates {n} languages.")),
                    Err(error) => Err(format!("Not connected: {error}.")),
                });
                Task::none()
            }
            ServerMsg::KeyTyped(draft) => {
                state.key = draft;
                Task::none()
            }
            ServerMsg::SaveKey => {
                let key = state.key.0.trim().to_owned();
                if key.is_empty() {
                    return Task::none();
                }
                state.key = Draft::default();
                state.busy = true;
                let keyring = self.keyring.clone();
                keyring_task(move || keyring.set(KEY_ENTRY, &key).map(|()| true))
            }
            ServerMsg::ClearKey => {
                state.busy = true;
                let keyring = self.keyring.clone();
                keyring_task(move || keyring.delete(KEY_ENTRY).map(|()| false))
            }
            ServerMsg::KeyStored(result) => {
                state.busy = false;
                match result {
                    Ok(set) => state.key_set = Some(set),
                    Err(error) => state.check = Some(Err(format!("The keyring said: {error}."))),
                }
                Task::none()
            }
        }
    }

    /// Saves the address typed, then checks the server answers; a changed
    /// address forgets the key first.
    fn save_server_address(&mut self) -> Task<Message> {
        let state = &mut self.listening.lyric_extras.settings;
        let typed = state
            .address
            .clone()
            .unwrap_or_else(|| self.config.translation_server.clone().unwrap_or_default());
        let server = match parse_address(&typed) {
            Err(why) => {
                state.check = Some(Err(why));
                return Task::none();
            }
            Ok(server) => server,
        };
        state.address = None;
        state.check = None;
        // A key is the server's: a new address forgets it, so the old key
        // is never sent somewhere else.
        let moved = forgets_key(self.config.translation_server.as_deref(), server.as_deref());
        if moved {
            state.key_set = Some(false);
        }
        self.config.translation_server = server.clone();
        self.save_config();
        // Whatever was shown came from the old server, or none.
        self.listening.lyric_extras.translation = None;
        let keyring = self.keyring.clone();
        let Some(server) = server else {
            return if moved {
                forget_key(keyring)
            } else {
                Task::none()
            };
        };
        self.listening.lyric_extras.settings.busy = true;
        Task::perform(
            async move {
                if moved {
                    forget(keyring.clone())
                        .await
                        .map_err(|e| format!("the old key couldn't be forgotten ({e})"))?;
                }
                let libre = LibreTranslate::new(&server, read_key(keyring).await)
                    .map_err(|e| e.to_string())?;
                libre
                    .languages()
                    .await
                    .map(|l| l.len())
                    .map_err(|e| e.to_string())
            },
            |result| Message::Listening(Msg::Server(ServerMsg::Checked(result))),
        )
    }
}

/// A keyring write, off the async threads; it answers whether a key is
/// there now.
fn keyring_task(
    write: impl FnOnce() -> Result<bool, crate::credentials::CredentialError> + Send + 'static,
) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(write)
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()))
        },
        |result| Message::Listening(Msg::Server(ServerMsg::KeyStored(result))),
    )
}

/// Settings → Lyrics → Translation: the server's address and its key.
pub fn settings_row<'a>(extras: &SettingsExtras<'a>) -> Element<'a, Message> {
    let state = &extras.listening.lyric_extras.settings;
    let send = |m: ServerMsg| Message::Listening(Msg::Server(m));
    let saved = extras
        .config
        .translation_server
        .as_deref()
        .unwrap_or_default();
    let address = state.address.as_deref().unwrap_or(saved);
    let mut rows = column![
        text("Translation server"),
        widgets::secondary(
            "Your own LibreTranslate server translates lyrics that aren't in your language. \
             The lyrics go only to it. Leave the address empty to turn translation off; a new \
             address forgets the saved key."
        )
        .size(13),
        row![
            text_input("translate.example.net", address)
                .on_input(move |s| send(ServerMsg::AddressTyped(s)))
                .on_submit(send(ServerMsg::SaveAddress))
                .padding(8)
                .style(theme::field),
            button(text("Save").size(13))
                .style(theme::tonal)
                .padding([6, 12])
                .on_press_maybe((!state.busy).then(|| send(ServerMsg::SaveAddress))),
        ]
        .spacing(8)
        .align_y(Center),
    ]
    .spacing(6);
    match &state.check {
        _ if state.busy => rows = rows.push(widgets::secondary("Checking\u{2026}").size(13)),
        Some(Ok(line)) => rows = rows.push(widgets::secondary(line.as_str()).size(13)),
        Some(Err(line)) => rows = rows.push(text(line.as_str()).size(13)),
        None => {}
    }
    if extras.config.translation_server.is_none() {
        return rows.into();
    }
    key_rows(rows, state).into()
}

/// The key: saved, cleared, or said to be set. Never shown.
fn key_rows<'a>(
    rows: iced::widget::Column<'a, Message>,
    state: &'a ServerSettings,
) -> iced::widget::Column<'a, Message> {
    let send = |m: ServerMsg| Message::Listening(Msg::Server(m));
    let key_line = match state.key_set {
        None => "Checking the keyring\u{2026}",
        Some(true) => "A key is set.",
        Some(false) => "No key is set. Only needed if your server asks for one.",
    };
    let mut key_row = row![
        column![text("API key"), widgets::secondary(key_line).size(13)]
            .spacing(2)
            .width(Fill)
    ]
    .spacing(8)
    .align_y(Center);
    if state.key_set == Some(true) {
        key_row = key_row.push(
            button(text("Clear").size(13))
                .style(theme::destructive)
                .padding([4, 10])
                .on_press_maybe((!state.busy).then(|| send(ServerMsg::ClearKey))),
        );
    }
    let can_save = !state.busy && !state.key.0.trim().is_empty();
    rows.push(key_row).push(
        row![
            text_input("Paste the server's API key", &state.key.0)
                .secure(true)
                .on_input(move |s| send(ServerMsg::KeyTyped(Draft(s))))
                .on_submit_maybe(can_save.then(|| send(ServerMsg::SaveKey)))
                .padding(8)
                .style(theme::field),
            button(text("Save").size(13))
                .style(theme::tonal)
                .padding([6, 12])
                .on_press_maybe(can_save.then(|| send(ServerMsg::SaveKey))),
        ]
        .spacing(8)
        .align_y(Center),
    )
}
