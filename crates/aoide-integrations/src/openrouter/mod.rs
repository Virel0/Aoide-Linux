//! OpenRouter, for the two features that need a language model: search by
//! description ([`OpenRouter::translate_search`]) and mixes from a mood
//! ([`OpenRouter::describe_mix`]).
//!
//! **The model only translates.** It turns words into filters or rules;
//! Jellyfin and the local history select the tracks, so a model can never
//! put a song in front of the listener. See [`smart_search`] and
//! [`smart_rules`].
//!
//! **What is sent**, to `https://openrouter.ai/api/v1/chat/completions` and
//! nowhere else: the phrase the listener typed and the names of the
//! library's genres, inside the prompt; the chosen model id; and the API key
//! in the `Authorization` header. No titles, artists, history, server
//! address or identifier of the listener. The request also names the app
//! (`HTTP-Referer` and `X-Title`), which is how OpenRouter attributes
//! requests. The model list ([`OpenRouter::models`]) is fetched without the
//! key.
//!
//! **The key is the caller's.** The app keeps it in the system keyring
//! through an [`ApiKeyStore`] and passes it to each call; this module never
//! stores, logs or prints it ([`ApiKey`]'s `Debug` is redacted).
//!
//! Ported from the Electron app's `src/main/features/aoide/smart-search.ts`.

mod json;
pub mod mix_plan;
pub mod smart_rules;
pub mod smart_search;

use std::fmt;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use reqwest::Url;
use serde::Deserialize;
use serde_json::json;

use smart_rules::{ParsedRules, SmartRules, build_mix_prompt, parse_rules};
use smart_search::{MusicQuery, build_prompt, parse_query};

/// OpenRouter's API, as every request is addressed.
pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1/";

/// A small, cheap, fast model: translating a phrase into six fields from a
/// closed vocabulary is about the easiest thing a language model does.
pub const DEFAULT_MODEL: &str = "openai/gpt-4o-mini";

/// The keyring entry the app should keep the key under.
pub const KEYRING_ENTRY: &str = "openrouter-api-key";

/// Long enough for a slow cold start, short enough that a search still
/// feels like one.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// How much of a refusal's body is kept in the reason.
const REASON_BODY: usize = 200;

/// The app's own name in OpenRouter's attribution headers.
const REFERER: &str = "https://github.com/Virel0/Aoide-Linux";
const TITLE: &str = "Aoide";

/// An OpenRouter API key. Its `Debug` and `Display` never show it.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// The key, trimmed; `None` when it is empty.
    pub fn new(key: &str) -> Option<ApiKey> {
        let key = key.trim();
        (!key.is_empty()).then(|| ApiKey(key.to_owned()))
    }

    /// The key itself, for the keyring.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(…)")
    }
}

/// The keyring failed, in its own words.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct KeyStoreError(pub String);

/// Where the app keeps the key: the system keyring, under
/// [`KEYRING_ENTRY`]. The app implements it over its own credential store;
/// [`MemoryKeyStore`] is for tests.
pub trait ApiKeyStore: Send + Sync {
    /// The key, `None` when no key is set.
    ///
    /// # Errors
    ///
    /// [`KeyStoreError`] when the keyring can't be read (locked, or no
    /// keyring service).
    fn load(&self) -> Result<Option<ApiKey>, KeyStoreError>;
    /// Keeps `key`, replacing any key already kept.
    ///
    /// # Errors
    ///
    /// [`KeyStoreError`] when the keyring can't be written.
    fn save(&self, key: &ApiKey) -> Result<(), KeyStoreError>;
    /// Forgets the key. Clearing a key that is not there is not an error.
    ///
    /// # Errors
    ///
    /// [`KeyStoreError`] when the keyring can't be written.
    fn clear(&self) -> Result<(), KeyStoreError>;
}

/// An [`ApiKeyStore`] in memory, gone with the process.
#[derive(Debug, Default)]
pub struct MemoryKeyStore(Mutex<Option<ApiKey>>);

impl ApiKeyStore for MemoryKeyStore {
    fn load(&self) -> Result<Option<ApiKey>, KeyStoreError> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone())
    }

    fn save(&self, key: &ApiKey) -> Result<(), KeyStoreError> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(key.clone());
        Ok(())
    }

    fn clear(&self) -> Result<(), KeyStoreError> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = None;
        Ok(())
    }
}

/// A model OpenRouter offers.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    /// OpenRouter's id, what a request names: `openai/gpt-4o-mini`.
    pub id: String,
    /// The name to show; the id when OpenRouter gives none.
    pub name: String,
    /// US dollars per prompt token, when OpenRouter says.
    pub prompt_price: Option<f64>,
    /// US dollars per completion token, when OpenRouter says.
    pub completion_price: Option<f64>,
    /// The most tokens the model takes, prompt and reply together, when
    /// OpenRouter says.
    pub context_length: Option<u64>,
}

/// Why a call to OpenRouter gave no answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpenRouterError {
    /// OpenRouter refused, in its own words (out of credit, unknown model,
    /// invalid key), the body cut to 200 characters.
    #[error("OpenRouter returned HTTP {status}: {body}")]
    Refused {
        /// The HTTP status, outside 2xx.
        status: u16,
        /// The start of the answer's body.
        body: String,
    },
    /// No answer within 15 seconds.
    #[error("OpenRouter did not answer in time.")]
    Timeout,
    /// No answer at all (DNS, a refused connection, TLS), or a base URL that
    /// doesn't parse.
    #[error("Could not reach OpenRouter: {0}")]
    Unreachable(String),
    /// A 2xx answer that isn't the JSON expected, or a completion with no
    /// text in its first choice.
    #[error("OpenRouter returned a reply in an unfamiliar shape.")]
    UnfamiliarReply,
}

/// What a search phrase became.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchOutcome {
    /// `None` when there was no usable answer; fall back to a plain search.
    /// May be [`MusicQuery::is_empty`], which also means a plain search.
    pub query: Option<MusicQuery>,
    /// Set only when something went wrong, in the service's own words.
    pub reason: Option<String>,
}

/// What a mood became.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MixOutcome {
    /// Things the description named; see [`ParsedRules::names`].
    pub names: Vec<String>,
    /// Rules the model produced that no app could evaluate, and why.
    pub rejected: Vec<String>,
    /// The rules that survived, or `None` when there was no usable answer.
    pub rules: Option<SmartRules>,
    /// Set only when the call failed, in the service's own words.
    pub reason: Option<String>,
}

/// The OpenRouter client. Cheap to clone.
#[derive(Debug, Clone)]
pub struct OpenRouter {
    http: reqwest::Client,
    base: Url,
}

impl OpenRouter {
    /// A client for OpenRouter itself.
    ///
    /// # Errors
    ///
    /// [`OpenRouterError::Unreachable`] when the HTTP client can't be built. Sends
    /// nothing.
    pub fn new() -> Result<Self, OpenRouterError> {
        Self::with_base_url(DEFAULT_BASE_URL)
    }

    /// A client for another address with OpenRouter's API (tests).
    ///
    /// # Errors
    ///
    /// [`OpenRouterError::Unreachable`] when `base` isn't a URL or the HTTP client
    /// can't be built. Sends nothing.
    pub fn with_base_url(base: &str) -> Result<Self, OpenRouterError> {
        crate::install_crypto();
        let mut base = Url::parse(base).map_err(|e| OpenRouterError::Unreachable(e.to_string()))?;
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| OpenRouterError::Unreachable(e.to_string()))?;
        Ok(Self { http, base })
    }

    fn url(&self, path: &str) -> Url {
        // `path` is one of this module's constants, always a valid relative URL.
        self.base.join(path).unwrap_or_else(|_| self.base.clone())
    }

    /// The models OpenRouter offers now, cheapest prompt first (those without
    /// a price last). Fetched, not hard-coded: the catalogue changes weekly.
    /// Sent without the key, so the picker works before one is entered.
    ///
    /// # Errors
    ///
    /// [`OpenRouterError::Timeout`] or [`OpenRouterError::Unreachable`] without an
    /// answer; [`OpenRouterError::Refused`] for a non-2xx status;
    /// [`OpenRouterError::UnfamiliarReply`] when the list isn't the JSON expected.
    /// A model without an id is skipped, not an error.
    pub async fn models(&self) -> Result<Vec<Model>, OpenRouterError> {
        let response = self
            .http
            .get(self.url("models"))
            .send()
            .await
            .map_err(transport)?;
        let body = read_body(response).await?;
        let list: ModelList =
            serde_json::from_str(&body).map_err(|_| OpenRouterError::UnfamiliarReply)?;
        let mut models: Vec<Model> = list
            .data
            .into_iter()
            .filter_map(|m| {
                let id = m.id?;
                let pricing = m.pricing.unwrap_or_default();
                Some(Model {
                    name: m.name.unwrap_or_else(|| id.clone()),
                    id,
                    prompt_price: price(pricing.prompt.as_ref()),
                    completion_price: price(pricing.completion.as_ref()),
                    context_length: m.context_length,
                })
            })
            .collect();
        models.sort_by(|a, b| {
            let key = |m: &Model| m.prompt_price.unwrap_or(f64::INFINITY);
            key(a).total_cmp(&key(b)).then_with(|| a.id.cmp(&b.id))
        });
        Ok(models)
    }

    /// One prompt, one reply: the text of the first choice. Temperature 0:
    /// the same phrase should give the same filters twice.
    ///
    /// # Errors
    ///
    /// [`OpenRouterError::Timeout`] or [`OpenRouterError::Unreachable`] without an
    /// answer; [`OpenRouterError::Refused`] for a non-2xx status (a bad key, no
    /// credit, an unknown model); [`OpenRouterError::UnfamiliarReply`] when the
    /// answer isn't a completion or its first choice has no text.
    pub async fn complete(
        &self,
        key: &ApiKey,
        model: &str,
        prompt: &str,
    ) -> Result<String, OpenRouterError> {
        let model = match model.trim() {
            "" => DEFAULT_MODEL,
            model => model,
        };
        let body = json!({
            "model": model,
            "messages": [{ "role": "user", "content": prompt }],
            "temperature": 0,
        });
        let response = self
            .http
            .post(self.url("chat/completions"))
            .bearer_auth(key.expose())
            .header("HTTP-Referer", REFERER)
            .header("X-Title", TITLE)
            .json(&body)
            .send()
            .await
            .map_err(transport)?;
        let body = read_body(response).await?;
        let reply: Completion =
            serde_json::from_str(&body).map_err(|_| OpenRouterError::UnfamiliarReply)?;
        reply
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message)
            .and_then(|m| m.content)
            .ok_or(OpenRouterError::UnfamiliarReply)
    }

    /// A search phrase as filters. Never fails at the caller: every failure
    /// comes back as `query: None` with a reason, because the plain search
    /// is always there to fall back on.
    pub async fn translate_search(
        &self,
        key: &ApiKey,
        model: &str,
        phrase: &str,
        library_genres: &[String],
    ) -> SearchOutcome {
        match self
            .complete(key, model, &build_prompt(phrase, library_genres))
            .await
        {
            Ok(reply) => SearchOutcome {
                query: parse_query(&reply, library_genres),
                reason: None,
            },
            Err(e) => SearchOutcome {
                query: None,
                reason: Some(e.to_string()),
            },
        }
    }

    /// A mood as smart playlist rules and the names it mentioned. Rules the
    /// model got wrong are dropped with a reason, not silently trimmed.
    /// Resolve the genres with [`mix_plan::match_genres`] over
    /// [`mix_plan::described_genres`].
    pub async fn describe_mix(
        &self,
        key: &ApiKey,
        model: &str,
        description: &str,
        library_genres: &[String],
    ) -> MixOutcome {
        match self
            .complete(key, model, &build_mix_prompt(description, library_genres))
            .await
        {
            Ok(reply) => {
                let ParsedRules {
                    names,
                    rejected,
                    rules,
                } = parse_rules(&reply);
                MixOutcome {
                    names,
                    rejected,
                    rules,
                    reason: None,
                }
            }
            Err(e) => MixOutcome {
                reason: Some(e.to_string()),
                ..MixOutcome::default()
            },
        }
    }
}

fn transport(error: reqwest::Error) -> OpenRouterError {
    if error.is_timeout() {
        OpenRouterError::Timeout
    } else {
        OpenRouterError::Unreachable(error.to_string())
    }
}

async fn read_body(response: reqwest::Response) -> Result<String, OpenRouterError> {
    let status = response.status();
    let body = response.text().await.map_err(transport)?;
    if !status.is_success() {
        return Err(OpenRouterError::Refused {
            status: status.as_u16(),
            body: body.chars().take(REASON_BODY).collect(),
        });
    }
    Ok(body)
}

/// OpenRouter writes prices as decimal strings ("0.00000015"), and "-1"
/// for a router whose price depends on the model it picks: no price.
fn price(value: Option<&serde_json::Value>) -> Option<f64> {
    let number = match value? {
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok()?,
        serde_json::Value::Number(n) => n.as_f64()?,
        _ => return None,
    };
    (number.is_finite() && number >= 0.0).then_some(number)
}

#[derive(Deserialize)]
struct ModelList {
    #[serde(default)]
    data: Vec<RawModel>,
}

#[derive(Deserialize)]
struct RawModel {
    id: Option<String>,
    name: Option<String>,
    pricing: Option<RawPricing>,
    context_length: Option<u64>,
}

#[derive(Deserialize, Default)]
struct RawPricing {
    prompt: Option<serde_json::Value>,
    completion: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct Completion {
    #[serde(default)]
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    content: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_never_shows_in_debug_output() {
        let key = ApiKey::new("  sk-or-v1-secret  ").unwrap();
        assert_eq!(key.expose(), "sk-or-v1-secret");
        assert!(!format!("{key:?}").contains("secret"));
        assert_eq!(ApiKey::new("   "), None);
    }

    #[test]
    fn the_memory_store_keeps_and_clears() {
        let store = MemoryKeyStore::default();
        assert_eq!(store.load().unwrap(), None);
        let key = ApiKey::new("k").unwrap();
        store.save(&key).unwrap();
        assert_eq!(store.load().unwrap(), Some(key));
        store.clear().unwrap();
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
    }

    #[test]
    fn prices_are_read_from_strings_or_numbers() {
        assert_eq!(price(Some(&json!("0.00000015"))), Some(0.000_000_15));
        assert_eq!(price(Some(&json!(0.5))), Some(0.5));
        assert_eq!(price(Some(&json!("-1"))), None);
        assert_eq!(price(Some(&json!("free"))), None);
        assert_eq!(price(Some(&json!(null))), None);
        assert_eq!(price(None), None);
    }

    #[test]
    fn a_base_url_without_a_trailing_slash_still_joins() {
        let client = OpenRouter::with_base_url("http://127.0.0.1:9/api/v1").unwrap();
        assert_eq!(
            client.url("models").as_str(),
            "http://127.0.0.1:9/api/v1/models"
        );
    }
}
