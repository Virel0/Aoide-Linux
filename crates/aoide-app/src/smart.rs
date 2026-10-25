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
//!
//! Search by description ([`search_by_description`]) sends the phrase and
//! the library's genre names, only when the listener presses the button,
//! never on a keystroke; the model's filters then go to Jellyfin as an
//! ordinary query, so the songs are always the server's.

use std::fmt;
use std::sync::Arc;

use aoide_integrations::openrouter::smart_search::{MusicQuery, Ordering};
use aoide_integrations::openrouter::{
    ApiKey, ApiKeyStore, DEFAULT_MODEL, KEYRING_ENTRY, KeyStoreError, Model, OpenRouter,
};
use aoide_jellyfin::{ItemKind, JellyfinClient, ListQuery, NameId, Sort, SortOrder};
use iced::widget::{Column, button, column, row, text, text_input};
use iced::{Center, Element, Fill, Task};
use serde::{Deserialize, Serialize};

use crate::app::{App, Message as AppMessage};
use crate::credentials::CredentialStore;
use crate::playback::tracks::TrackMeta;
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
pub struct Draft(pub(crate) String);

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
    /// Search's "Search by description", for the page with this key.
    Describe {
        page: u64,
        phrase: String,
    },
}

/// Asks the keyring whether a key is set, at launch.
pub fn load_key(keyring: Arc<dyn CredentialStore>) -> Task<AppMessage> {
    Task::perform(
        blocking(move || KeyringApiKeys(keyring).load().map(|k| k.is_some())),
        |r| AppMessage::Smart(Message::KeyLoaded(r)),
    )
}

/// The key, from the keyring, for one request.
pub async fn read_key(keyring: Arc<dyn CredentialStore>) -> Result<ApiKey, String> {
    blocking(move || KeyringApiKeys(keyring).load())
        .await?
        .ok_or_else(|| "No OpenRouter key is set. Add one in Settings → Smart search.".to_owned())
}

/// How many songs a description finds at most.
pub const DESCRIBED_SONGS: u32 = 100;

/// Most years a range is spelled out as, where the server lists none.
const MAX_YEARS: i32 = 150;

/// What a description found.
#[derive(Debug, Clone, PartialEq)]
pub struct Described {
    /// The filters in words, or `None` when the description gave nothing to
    /// filter by (the plain search stands).
    pub understood: Option<String>,
    pub songs: Vec<TrackMeta>,
    /// Why there are no filters, or why nothing could match.
    pub note: Option<String>,
}

/// Why a description could not be searched.
#[derive(Debug)]
pub enum Failure {
    Server(aoide_jellyfin::Error),
    Other(String),
}

/// The filters in words, for "Understood as …".
pub fn understood(query: &MusicQuery) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(term) = &query.search_term {
        parts.push(format!("“{term}”"));
    }
    if !query.genres.is_empty() {
        parts.push(query.genres.join(", "));
    }
    match (query.from_year, query.to_year) {
        (Some(a), Some(b)) if a == b => parts.push(a.to_string()),
        (Some(a), Some(b)) => parts.push(format!("{a}–{b}")),
        (Some(a), None) => parts.push(format!("from {a}")),
        (None, Some(b)) => parts.push(format!("up to {b}")),
        (None, None) => {}
    }
    if query.favorites_only {
        parts.push("favourites".to_owned());
    }
    let order = match query.ordering {
        Ordering::ByName => None,
        Ordering::MostPlayed => Some("most played first"),
        Ordering::NewestFirst => Some("newest first"),
        Ordering::Random => Some("in random order"),
        Ordering::RecentlyPlayed => Some("recently played first"),
    };
    parts.extend(order.map(str::to_owned));
    parts.join(" · ")
}

/// The years of the library inside the query's range: the server's own list
/// where it gave one, else the range spelled out. `None` for no year filter;
/// an empty list means no year of the library is in range, and must not be
/// sent (Jellyfin reads no years as every year).
pub fn years_in_range(
    query: &MusicQuery,
    library_years: &[i32],
    this_year: i32,
) -> Option<Vec<i32>> {
    if query.from_year.is_none() && query.to_year.is_none() {
        return None;
    }
    let from = query.from_year.unwrap_or(i32::MIN);
    let to = query.to_year.unwrap_or(i32::MAX);
    let mut years: Vec<i32> = if library_years.is_empty() {
        let to = to.min(this_year);
        let from = from.max(to - MAX_YEARS + 1);
        (from..=to).collect()
    } else {
        library_years
            .iter()
            .copied()
            .filter(|y| (from..=to).contains(y))
            .collect()
    };
    years.sort_unstable();
    years.dedup();
    Some(years)
}

/// The Jellyfin query for a model's filters: songs in the library, the
/// named genres by id, the years, favourites, the term, the order. `None`
/// when the years rule every song out.
pub fn song_query(
    query: &MusicQuery,
    genres: &[NameId],
    library_years: &[i32],
    library: Option<&str>,
    this_year: i32,
) -> Option<ListQuery> {
    let years = match years_in_range(query, library_years, this_year) {
        Some(years) if years.is_empty() => return None,
        Some(years) => years,
        None => Vec::new(),
    };
    let genre_ids = query
        .genres
        .iter()
        .filter_map(|name| {
            genres
                .iter()
                .find(|g| g.name.eq_ignore_ascii_case(name))
                .map(|g| g.id.clone())
        })
        .collect();
    let (sort, order) = match query.ordering {
        Ordering::ByName => (Sort::Name, SortOrder::Ascending),
        Ordering::MostPlayed => (Sort::PlayCount, SortOrder::Descending),
        Ordering::NewestFirst => (Sort::ReleaseDate, SortOrder::Descending),
        Ordering::Random => (Sort::Random, SortOrder::Ascending),
        Ordering::RecentlyPlayed => (Sort::DatePlayed, SortOrder::Descending),
    };
    Some(ListQuery {
        parent_id: library.map(str::to_owned),
        search_term: query.search_term.clone(),
        genre_ids,
        years,
        favorites_only: query.favorites_only,
        sort,
        order,
        limit: Some(DESCRIBED_SONGS),
        ..ListQuery::default()
    })
}

fn this_year() -> i32 {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400);
    // Close enough for a bound: 1970 plus whole Julian years.
    1970 + i32::try_from(days * 4 / 1461).unwrap_or(0)
}

/// A phrase, through the model, to Jellyfin's songs.
pub async fn search_by_description(
    openrouter: OpenRouter,
    keyring: Arc<dyn CredentialStore>,
    model: String,
    client: JellyfinClient,
    library: Option<String>,
    phrase: String,
) -> Result<Described, Failure> {
    let key = read_key(keyring).await.map_err(Failure::Other)?;
    let filters = client
        .query_filters(library.as_deref(), &[ItemKind::Audio])
        .await
        .map_err(Failure::Server)?;
    let names: Vec<String> = filters.genres.iter().map(|g| g.name.clone()).collect();
    let outcome = openrouter
        .translate_search(&key, &model, &phrase, &names)
        .await;
    let Some(query) = outcome.query.filter(|q| !q.is_empty()) else {
        return Ok(Described {
            understood: None,
            songs: Vec::new(),
            note: Some(outcome.reason.unwrap_or_else(|| {
                "That description gave nothing to filter by, so the plain search stands.".to_owned()
            })),
        });
    };
    let words = understood(&query);
    let Some(list) = song_query(
        &query,
        &filters.genres,
        &filters.years,
        library.as_deref(),
        this_year(),
    ) else {
        return Ok(Described {
            understood: Some(words),
            songs: Vec::new(),
            note: Some("Nothing in your library is from those years.".to_owned()),
        });
    };
    let page = client.songs(&list).await.map_err(Failure::Server)?;
    Ok(Described {
        understood: Some(words),
        songs: page.items.iter().map(TrackMeta::from_item).collect(),
        note: None,
    })
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
            Message::Describe { page, phrase } => {
                let Some(signed) = &self.session else {
                    return Task::none();
                };
                let asked = phrase.clone();
                let openrouter = match client() {
                    Ok(openrouter) => openrouter,
                    Err(error) => {
                        return Task::done(AppMessage::Page(
                            page,
                            crate::screens::PageMsg::Search(
                                crate::screens::search::Message::Described(asked, Err(error)),
                            ),
                        ));
                    }
                };
                Task::perform(
                    search_by_description(
                        openrouter,
                        Arc::clone(&self.keyring),
                        self.config.system.smart_search.model().to_owned(),
                        signed.connection.client.clone(),
                        signed.library.clone(),
                        phrase,
                    ),
                    move |result| match result {
                        Err(Failure::Server(aoide_jellyfin::Error::Unauthorized)) => {
                            AppMessage::TokenRejected
                        }
                        result => AppMessage::Page(
                            page,
                            crate::screens::PageMsg::Search(
                                crate::screens::search::Message::Described(
                                    asked.clone(),
                                    result.map_err(|failure| match failure {
                                        Failure::Server(e) => crate::session::describe(&e),
                                        Failure::Other(e) => e,
                                    }),
                                ),
                            ),
                        ),
                    },
                )
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
            text("OpenRouter key"),
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
            button(text("Show models").size(13))
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
            button(text("Use this ID").size(13))
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
        "Smart search",
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

    fn query() -> MusicQuery {
        MusicQuery::default()
    }

    fn genres() -> Vec<NameId> {
        ["Folk", "Indie", "Ambient"]
            .iter()
            .map(|g| NameId {
                id: format!("g-{g}"),
                name: (*g).to_owned(),
            })
            .collect()
    }

    #[test]
    fn filters_read_as_words() {
        let q = MusicQuery {
            genres: vec!["Folk".into(), "Indie".into()],
            from_year: Some(2010),
            to_year: Some(2019),
            favorites_only: true,
            ordering: Ordering::NewestFirst,
            search_term: Some("harbour".into()),
        };
        assert_eq!(
            understood(&q),
            "“harbour” · Folk, Indie · 2010–2019 · favourites · newest first"
        );
        let one_year = MusicQuery {
            from_year: Some(1999),
            to_year: Some(1999),
            ..query()
        };
        assert_eq!(understood(&one_year), "1999");
        let open = MusicQuery {
            from_year: Some(2020),
            ordering: Ordering::Random,
            ..query()
        };
        assert_eq!(understood(&open), "from 2020 · in random order");
    }

    #[test]
    fn years_come_from_the_library_inside_the_range() {
        let library = [2023, 2022, 2021, 2019, 2018, 2015];
        let q = MusicQuery {
            from_year: Some(2016),
            to_year: Some(2021),
            ..query()
        };
        assert_eq!(
            years_in_range(&q, &library, 2026),
            Some(vec![2018, 2019, 2021])
        );
        assert_eq!(years_in_range(&query(), &library, 2026), None);
        // Nothing in range: an empty list, which the query must not send.
        let none = MusicQuery {
            from_year: Some(1960),
            to_year: Some(1969),
            ..query()
        };
        assert_eq!(years_in_range(&none, &library, 2026), Some(vec![]));
        // A server that lists no years: the range spelled out, up to now.
        let open = MusicQuery {
            from_year: Some(2024),
            ..query()
        };
        assert_eq!(
            years_in_range(&open, &[], 2026),
            Some(vec![2024, 2025, 2026])
        );
        let up_to = MusicQuery {
            to_year: Some(1999),
            ..query()
        };
        let spelled = years_in_range(&up_to, &[], 2026).unwrap();
        assert_eq!(spelled.len(), MAX_YEARS as usize);
        assert_eq!(spelled.last(), Some(&1999));
    }

    #[test]
    fn the_jellyfin_query_carries_every_filter() {
        let q = MusicQuery {
            genres: vec!["folk".into(), "Jazz".into()],
            from_year: Some(2018),
            to_year: Some(2019),
            favorites_only: true,
            ordering: Ordering::MostPlayed,
            search_term: Some("lights".into()),
        };
        let list = song_query(&q, &genres(), &[2019, 2015], Some("lib1"), 2026).unwrap();
        assert_eq!(list.genre_ids, ["g-Folk"]);
        assert_eq!(list.years, [2019]);
        assert!(list.favorites_only);
        assert_eq!(list.search_term.as_deref(), Some("lights"));
        assert_eq!(list.parent_id.as_deref(), Some("lib1"));
        assert_eq!(
            (list.sort, list.order),
            (Sort::PlayCount, SortOrder::Descending)
        );
        assert_eq!(list.limit, Some(DESCRIBED_SONGS));
        let out_of_range = MusicQuery {
            from_year: Some(1950),
            to_year: Some(1959),
            ..q
        };
        assert_eq!(
            song_query(&out_of_range, &genres(), &[2019], None, 2026),
            None
        );
    }

    #[test]
    fn orderings_sort_the_way_the_phone_does() {
        let sorted = |ordering| {
            let list = song_query(
                &MusicQuery {
                    ordering,
                    ..query()
                },
                &[],
                &[],
                None,
                2026,
            )
            .unwrap();
            (list.sort, list.order)
        };
        assert_eq!(sorted(Ordering::ByName), (Sort::Name, SortOrder::Ascending));
        assert_eq!(
            sorted(Ordering::NewestFirst),
            (Sort::ReleaseDate, SortOrder::Descending)
        );
        assert_eq!(
            sorted(Ordering::Random),
            (Sort::Random, SortOrder::Ascending)
        );
        assert_eq!(
            sorted(Ordering::RecentlyPlayed),
            (Sort::DatePlayed, SortOrder::Descending)
        );
    }

    fn jellyfin(server: &wiremock::MockServer) -> JellyfinClient {
        JellyfinClient::new(
            aoide_jellyfin::ServerAddress::parse(&server.uri()).unwrap(),
            aoide_jellyfin::Device::new("Test", "device-1"),
        )
        .unwrap()
        .with_credentials(aoide_jellyfin::Credentials {
            user_id: "u1".into(),
            access_token: "tok".into(),
            username: "tester".into(),
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_description_becomes_a_jellyfin_query_and_only_words_leave() {
        use wiremock::matchers::{header, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/Items/Filters2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Genres": [{"Id": "g-folk", "Name": "Folk"}, {"Id": "g-indie", "Name": "Indie"}],
                "Tags": []
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/Items/Filters"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"Years": [2015, 2019, 2022]})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/Items"))
            .and(query_param("genreIds", "g-folk"))
            .and(query_param("years", "2015,2019"))
            .and(query_param("includeItemTypes", "Audio"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Items": [{"Id": "t1", "Name": "Harbour Lights", "Type": "Audio"}],
                "TotalRecordCount": 1
            })))
            .expect(1)
            .mount(&server)
            .await;
        let reply = serde_json::json!({"genres": ["folk"], "fromYear": 2010, "toYear": 2019,
            "favoritesOnly": false, "searchTerm": null, "ordering": "byName"});
        Mock::given(method("POST"))
            .and(path("/api/v1/chat/completions"))
            .and(header("authorization", "Bearer sk-or-test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": reply.to_string()}}]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let keyring: Arc<dyn CredentialStore> = Arc::new(MemoryStore::default());
        keyring.set(KEYRING_ENTRY, "sk-or-test").unwrap();
        let openrouter = OpenRouter::with_base_url(&format!("{}/api/v1/", server.uri())).unwrap();
        let found = search_by_description(
            openrouter,
            keyring,
            DEFAULT_MODEL.into(),
            jellyfin(&server),
            Some("lib1".into()),
            "calm folk from the twenty tens".into(),
        )
        .await
        .unwrap();
        assert_eq!(found.understood.as_deref(), Some("Folk · 2010–2019"));
        assert_eq!(found.songs.len(), 1);
        assert_eq!(found.songs[0].title, "Harbour Lights");
        assert_eq!(found.note, None);
        // What went to OpenRouter: the phrase and the genre names, nothing else
        // of the library's.
        let requests = server.received_requests().await.unwrap();
        let sent: Vec<_> = requests
            .iter()
            .filter(|r| r.url.path() == "/api/v1/chat/completions")
            .collect();
        assert_eq!(sent.len(), 1);
        let body = String::from_utf8_lossy(&sent[0].body);
        assert!(body.contains("calm folk from the twenty tens"));
        assert!(body.contains("Folk, Indie"));
        assert!(!body.contains("Harbour Lights"));
        assert!(!body.contains("lib1"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn no_key_asks_nothing_of_anyone() {
        let server = wiremock::MockServer::start().await;
        let openrouter = OpenRouter::with_base_url(&format!("{}/api/v1/", server.uri())).unwrap();
        let result = search_by_description(
            openrouter,
            Arc::new(MemoryStore::default()),
            DEFAULT_MODEL.into(),
            jellyfin(&server),
            None,
            "calm folk from the twenty tens".into(),
        )
        .await;
        let Err(Failure::Other(reason)) = result else {
            panic!("no key is a reason, not a request");
        };
        assert!(reason.contains("No OpenRouter key"), "{reason}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[test]
    fn this_year_is_plausible() {
        let year = this_year();
        assert!((2025..2200).contains(&year), "{year}");
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
