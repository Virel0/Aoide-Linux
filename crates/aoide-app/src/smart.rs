//! Smart search's settings: the OpenRouter key and model behind search by
//! description and mixes.
//!
//! The key lives in the system keyring, through [`KeyringApiKeys`], an
//! [`ApiKeyStore`] over the app's own [`CredentialStore`]. The screen only
//! ever writes it: it can save a key, clear it, and say that one is set, but
//! never reads it back, shows it, or logs it. It is read from the keyring at
//! the moment a request needs it, off the UI thread.
//!
//! The model list is OpenRouter's public catalogue, fetched (without the
//! key) only when the listener asks to see it.

use std::fmt;
use std::sync::Arc;

use aoide_integrations::openrouter::{
    ApiKey, ApiKeyStore, DEFAULT_MODEL, KEYRING_ENTRY, KeyStoreError, Model, OpenRouter,
};
use iced::widget::{Column, button, column, row, text, text_input};
use iced::{Center, Element, Fill, Task};
use serde::{Deserialize, Serialize};

use crate::app::{App, Message as AppMessage};
use crate::credentials::CredentialStore;
use crate::screens::Load;
use crate::theme;
use crate::widgets;

/// A development build can talk to a stand-in instead of OpenRouter
/// (`AOIDE_DEV_OPENROUTER_URL=http://127.0.0.1:…/api/v1/`).
const DEV_URL: &str = "AOIDE_DEV_OPENROUTER_URL";

/// How many models the picker lists at once.
const LISTED: usize = 8;

pub const KEY_FIELD: &str = "smart-search-key";

/// The OpenRouter key, kept in the system keyring under the app's service.
pub struct KeyringApiKeys(pub Arc<dyn CredentialStore>);

impl ApiKeyStore for KeyringApiKeys {
    fn load(&self) -> Result<Option<ApiKey>, KeyStoreError> {
        self.0
            .get(KEYRING_ENTRY)
            .map(|key| key.as_deref().and_then(ApiKey::new))
            .map_err(|e| KeyStoreError(e.to_string()))
    }

    fn save(&self, key: &ApiKey) -> Result<(), KeyStoreError> {
        self.0
            .set(KEYRING_ENTRY, key.expose())
            .map_err(|e| KeyStoreError(e.to_string()))
    }

    fn clear(&self) -> Result<(), KeyStoreError> {
        self.0
            .delete(KEYRING_ENTRY)
            .map_err(|e| KeyStoreError(e.to_string()))
    }
}

/// Runs keyring work off the async threads: D-Bus calls and Keychain
/// prompts block.
pub async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, KeyStoreError> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(job)
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r.map_err(|e| e.to_string()))
}

/// The settings kept in the config file. The key is not one of them.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SmartSettings {
    /// An OpenRouter model id; empty is [`DEFAULT_MODEL`].
    pub model: String,
}

impl SmartSettings {
    pub fn model(&self) -> &str {
        match self.model.trim() {
            "" => DEFAULT_MODEL,
            model => model,
        }
    }
}

/// A key being typed. Its `Debug` never shows it.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Draft(String);

impl fmt::Debug for Draft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Draft(…)")
    }
}

/// The client: OpenRouter, or the development stand-in.
pub fn client() -> Result<OpenRouter, String> {
    match std::env::var(DEV_URL) {
        Ok(url) if cfg!(debug_assertions) => OpenRouter::with_base_url(&url),
        _ => OpenRouter::new(),
    }
    .map_err(|e| e.to_string())
}

/// What the app holds for smart search while it runs.
#[derive(Debug, Default)]
pub struct State {
    /// Whether the keyring has a key; `None` until it has been asked.
    pub key_set: Option<bool>,
    pub draft: Draft,
    pub key_error: Option<String>,
    pub saving: bool,
    /// What the model field says.
    pub model_filter: String,
    pub models: Option<Load<Vec<Model>>>,
}

impl State {
    pub fn has_key(&self) -> bool {
        self.key_set == Some(true)
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    KeyLoaded(Result<bool, String>),
    KeyTyped(Draft),
    SaveKey,
    ClearKey,
    /// The keyring answered a save (`true`) or a clear (`false`).
    KeyStored(bool, Result<(), String>),
    ModelTyped(String),
    UseModel(String),
    ShowModels,
    Models(Result<Vec<Model>, String>),
}

/// Asks the keyring whether a key is set, at launch.
pub fn load_key(keyring: Arc<dyn CredentialStore>) -> Task<AppMessage> {
    Task::perform(
        blocking(move || KeyringApiKeys(keyring).load().map(|k| k.is_some())),
        |r| AppMessage::Smart(Message::KeyLoaded(r)),
    )
}

/// US dollars per token as dollars per million, the way OpenRouter's site
/// prices them.
fn per_million(price: f64) -> String {
    let per = price * 1_000_000.0;
    if per >= 10.0 {
        format!("${per:.0}")
    } else if per >= 0.1 {
        format!("${per:.2}")
    } else {
        format!("${per:.3}")
    }
}

/// A model's price in a few words.
pub fn price_label(model: &Model) -> String {
    match (model.prompt_price, model.completion_price) {
        (Some(p), Some(c)) if p == 0.0 && c == 0.0 => "free".to_owned(),
        (Some(p), Some(c)) => format!(
            "{} in · {} out per million tokens",
            per_million(p),
            per_million(c)
        ),
        (Some(p), None) => format!("{} in per million tokens", per_million(p)),
        _ => "price varies".to_owned(),
    }
}

/// The models whose id or name holds every word of `filter`, cheapest
/// first (the order `models()` gives), at most `limit`.
pub fn matching<'a>(models: &'a [Model], filter: &str, limit: usize) -> Vec<&'a Model> {
    let words: Vec<String> = filter.split_whitespace().map(str::to_lowercase).collect();
    models
        .iter()
        .filter(|m| {
            let haystack = format!("{} {}", m.id, m.name).to_lowercase();
            words.iter().all(|w| haystack.contains(w))
        })
        .take(limit)
        .collect()
}

impl App {
    pub(crate) fn smart_message(&mut self, message: Message) -> Task<AppMessage> {
        let state = &mut self.smart;
        match message {
            Message::KeyLoaded(result) => {
                match result {
                    Ok(set) => state.key_set = Some(set),
                    Err(error) => {
                        state.key_set = Some(false);
                        state.key_error = Some(format!("The keyring couldn't be read: {error}"));
                    }
                }
                Task::none()
            }
            Message::KeyTyped(draft) => {
                state.draft = draft;
                state.key_error = None;
                Task::none()
            }
            Message::SaveKey => {
                let Some(key) = ApiKey::new(&state.draft.0) else {
                    return Task::none();
                };
                state.draft = Draft::default();
                state.saving = true;
                let store = KeyringApiKeys(Arc::clone(&self.keyring));
                Task::perform(blocking(move || store.save(&key)), |r| {
                    AppMessage::Smart(Message::KeyStored(true, r))
                })
            }
            Message::ClearKey => {
                state.saving = true;
                let store = KeyringApiKeys(Arc::clone(&self.keyring));
                Task::perform(blocking(move || store.clear()), |r| {
                    AppMessage::Smart(Message::KeyStored(false, r))
                })
            }
            Message::KeyStored(saved, result) => {
                state.saving = false;
                match result {
                    Ok(()) => {
                        state.key_set = Some(saved);
                        state.key_error = None;
                    }
                    Err(error) => {
                        state.key_error = Some(if saved {
                            format!("The key couldn't be saved: {error}")
                        } else {
                            format!("The key couldn't be cleared: {error}")
                        });
                    }
                }
                Task::none()
            }
            Message::ModelTyped(filter) => {
                state.model_filter = filter;
                Task::none()
            }
            Message::UseModel(id) => {
                let id = id.trim().to_owned();
                if id.is_empty() {
                    return Task::none();
                }
                state.model_filter.clear();
                self.config.system.smart_search.model = id;
                self.save_config();
                Task::none()
            }
            Message::ShowModels => {
                state.models = Some(Load::Loading);
                Task::perform(
                    async move {
                        let client = client()?;
                        client.models().await.map_err(|e| e.to_string())
                    },
                    |r| AppMessage::Smart(Message::Models(r)),
                )
            }
            Message::Models(result) => {
                state.models = Some(Load::from(result));
                Task::none()
            }
        }
    }
}

/// Settings → Smart Search: the key (write-only) and the model.
pub fn settings_section<'a>(
    state: &'a State,
    settings: &'a SmartSettings,
) -> Element<'a, AppMessage> {
    let send = |m: Message| AppMessage::Smart(m);
    let mut rows: Column<'a, AppMessage> = column![];

    // The key: saved, cleared, or said to be set. Never shown.
    let key_line = match state.key_set {
        None => "Checking the keyring…",
        Some(true) => "A key is set.",
        Some(false) => "No key is set.",
    };
    let mut key_row = row![
        column![
            text("OpenRouter Key"),
            widgets::secondary(key_line).size(13)
        ]
        .spacing(2)
        .width(Fill)
    ]
    .spacing(8)
    .align_y(Center);
    if state.has_key() {
        key_row = key_row.push(
            button(text("Clear").size(13))
                .style(theme::destructive)
                .padding([4, 10])
                .on_press_maybe((!state.saving).then(|| send(Message::ClearKey))),
        );
    }
    rows = rows.push(key_row);
    let can_save = !state.saving && ApiKey::new(&state.draft.0).is_some();
    rows = rows.push(
        row![
            text_input(
                if state.has_key() {
                    "Paste a new key to replace it"
                } else {
                    "Paste your OpenRouter key"
                },
                &state.draft.0,
            )
            .id(KEY_FIELD)
            .secure(true)
            .on_input(|s| AppMessage::Smart(Message::KeyTyped(Draft(s))))
            .on_submit_maybe(can_save.then(|| send(Message::SaveKey)))
            .padding(8)
            .style(theme::field),
            button(text("Save").size(13))
                .style(theme::tonal)
                .padding([6, 12])
                .on_press_maybe(can_save.then(|| send(Message::SaveKey))),
        ]
        .spacing(8)
        .align_y(Center),
    );
    if let Some(error) = &state.key_error {
        rows = rows.push(text(error.as_str()).size(13));
    }

    // The model: picked from OpenRouter's list, or any id typed.
    rows = rows.push(
        row![
            column![text("Model"), widgets::secondary(settings.model()).size(13)]
                .spacing(2)
                .width(Fill),
            button(text("Show Models").size(13))
                .style(theme::tonal)
                .padding([4, 10])
                .on_press_maybe(
                    (!matches!(state.models, Some(Load::Loading)))
                        .then(|| send(Message::ShowModels))
                ),
        ]
        .spacing(8)
        .align_y(Center),
    );
    let typed = state.model_filter.trim();
    rows = rows.push(
        row![
            text_input("Search the models, or type a model id", &state.model_filter)
                .on_input(|s| AppMessage::Smart(Message::ModelTyped(s)))
                .on_submit_maybe(
                    (!typed.is_empty()).then(|| send(Message::UseModel(typed.to_owned())))
                )
                .padding(8)
                .style(theme::field),
            button(text("Use This Id").size(13))
                .style(theme::tonal)
                .padding([6, 12])
                .on_press_maybe(
                    (!typed.is_empty()).then(|| send(Message::UseModel(typed.to_owned())))
                ),
        ]
        .spacing(8)
        .align_y(Center),
    );
    match &state.models {
        None => {}
        Some(Load::Loading) => rows = rows.push(widgets::secondary("Loading OpenRouter's models…")),
        Some(Load::Failed(error)) => {
            rows = rows.push(text(format!("The models couldn't be loaded: {error}")).size(13));
        }
        Some(Load::Ready(models)) => {
            let found = matching(models, typed, LISTED);
            if found.is_empty() {
                rows = rows.push(widgets::secondary("No model matches that.").size(13));
            }
            for model in found {
                let chosen = model.id == settings.model();
                rows = rows.push(
                    button(
                        row![
                            column![
                                text(model.name.as_str()).size(14),
                                widgets::secondary(model.id.as_str()).size(12),
                            ]
                            .spacing(1)
                            .width(Fill),
                            widgets::secondary(price_label(model)).size(12),
                        ]
                        .spacing(10)
                        .align_y(Center),
                    )
                    .width(Fill)
                    .padding([4, 8])
                    .style(if chosen {
                        theme::row_selected
                    } else {
                        theme::row
                    })
                    .on_press(send(Message::UseModel(model.id.clone()))),
                );
            }
        }
    }

    crate::screens::settings::section(
        "Smart Search",
        rows,
        "Search by description and Mixes ask a language model on OpenRouter to turn your words \
         into filters; your server then finds the songs. Only the words you typed and your \
         library's genre names are sent, with the key, to OpenRouter, and only when you ask. \
         The key stays in your system keyring. Each request is billed to your OpenRouter account \
         at the model's price; small models cost a fraction of a cent.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::MemoryStore;

    fn model(id: &str, name: &str, prompt: Option<f64>, completion: Option<f64>) -> Model {
        Model {
            id: id.into(),
            name: name.into(),
            prompt_price: prompt,
            completion_price: completion,
            context_length: None,
        }
    }

    #[test]
    fn the_key_goes_to_the_keyring_under_its_entry_and_comes_back() {
        let keyring: Arc<dyn CredentialStore> = Arc::new(MemoryStore::default());
        let store = KeyringApiKeys(Arc::clone(&keyring));
        assert_eq!(store.load().unwrap(), None);
        store.save(&ApiKey::new(" sk-or-test ").unwrap()).unwrap();
        assert_eq!(
            keyring.get(KEYRING_ENTRY).unwrap().as_deref(),
            Some("sk-or-test")
        );
        assert_eq!(store.load().unwrap(), ApiKey::new("sk-or-test"));
        store.clear().unwrap();
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
    }

    #[test]
    fn a_keyring_that_refuses_is_reported() {
        let store = KeyringApiKeys(Arc::new(MemoryStore::failing()));
        assert!(store.save(&ApiKey::new("k").unwrap()).is_err());
    }

    #[test]
    fn a_draft_never_shows_in_debug_output() {
        let draft = Draft("sk-or-v1-secret".into());
        assert!(!format!("{draft:?}").contains("secret"));
        let message = Message::KeyTyped(draft);
        assert!(!format!("{message:?}").contains("secret"));
    }

    #[test]
    fn an_empty_model_is_the_default() {
        assert_eq!(SmartSettings::default().model(), DEFAULT_MODEL);
        let chosen = SmartSettings {
            model: " anthropic/claude-haiku ".into(),
        };
        assert_eq!(chosen.model(), "anthropic/claude-haiku");
    }

    #[test]
    fn prices_read_per_million_tokens() {
        let cheap = model("a/mini", "Mini", Some(0.000_000_15), Some(0.000_000_6));
        assert_eq!(
            price_label(&cheap),
            "$0.15 in · $0.60 out per million tokens"
        );
        let dear = model("a/big", "Big", Some(0.000_015), Some(0.000_075));
        assert_eq!(price_label(&dear), "$15 in · $75 out per million tokens");
        assert_eq!(
            price_label(&model("a/free", "Free", Some(0.0), Some(0.0))),
            "free"
        );
        assert_eq!(
            price_label(&model("a/auto", "Auto", None, None)),
            "price varies"
        );
        let tiny = model("a/t", "T", Some(0.000_000_02), None);
        assert_eq!(price_label(&tiny), "$0.020 in per million tokens");
    }

    #[test]
    fn the_picker_matches_every_word_in_id_or_name() {
        let models = vec![
            model("openai/gpt-4o-mini", "OpenAI: GPT-4o-mini", Some(0.1), None),
            model(
                "anthropic/claude-haiku",
                "Anthropic: Claude Haiku",
                Some(0.2),
                None,
            ),
            model("openai/gpt-4o", "OpenAI: GPT-4o", Some(0.3), None),
        ];
        let ids = |found: Vec<&Model>| found.iter().map(|m| m.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(matching(&models, "", 8)).len(), 3);
        assert_eq!(
            ids(matching(&models, "GPT mini", 8)),
            ["openai/gpt-4o-mini"]
        );
        assert_eq!(
            ids(matching(&models, "haiku", 8)),
            ["anthropic/claude-haiku"]
        );
        assert_eq!(ids(matching(&models, "openai", 1)), ["openai/gpt-4o-mini"]);
        assert!(matching(&models, "llama", 8).is_empty());
    }
}
