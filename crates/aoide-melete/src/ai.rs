//! Music by description, through the server's AI: a phrase as filters for a
//! songs query, and a mood as smart-playlist rules. Melete sends the words and
//! the library's genre names to OpenRouter with its own key; the model only
//! translates, and the app runs what comes back against the library.
//!
//! An administrator turns these on. While they're off, both calls answer
//! HTTP 503; [`MeleteClient::ai_enabled`] says so beforehand.

use reqwest::Method;
use serde::Deserialize;
use serde_json::json;

use crate::client::MeleteClient;
use crate::{Error, Result};

/// The order a description asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MusicOrdering {
    /// The most played first.
    MostPlayed,
    /// The newest release first.
    NewestFirst,
    /// Shuffled.
    Random,
    /// The most recently played first.
    RecentlyPlayed,
    /// A to Z by name: the default, and any order this client doesn't know.
    #[default]
    #[serde(other)]
    ByName,
}

/// What a description became: filters over the library. Every genre is one
/// the library has, in its spelling; a reversed year range comes back put
/// right.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct MusicFilters {
    /// A song, album or artist name the description named.
    pub search_term: Option<String>,
    /// The library's own genres.
    #[serde(default)]
    pub genres: Vec<String>,
    /// Released in or after.
    pub from_year: Option<i32>,
    /// Released in or before.
    pub to_year: Option<i32>,
    /// Only what the user likes.
    #[serde(default)]
    pub liked_only: bool,
    /// The order to list the songs in.
    #[serde(default)]
    pub ordering: MusicOrdering,
}

impl MusicFilters {
    /// True when the filters would narrow nothing, and a plain search is the
    /// better answer: running them would list the library by name.
    pub fn is_empty(&self) -> bool {
        self.genres.is_empty()
            && self.from_year.is_none()
            && self.to_year.is_none()
            && self.search_term.is_none()
            && !self.liked_only
    }
}

/// What a mood became.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct MusicMix {
    /// Artists, albums, bands, games, films or shows the description named,
    /// as the model spelled them, to search the library for.
    #[serde(default)]
    pub names: Vec<String>,
    /// The rule set in Aoide's smart-playlist format, `{match, limit, rules}`
    /// with `limit` null for no cap; `None` when no rule was usable. Left as
    /// JSON for the app to check rule by rule. Genre values are as the model
    /// wrote them.
    pub rules: Option<serde_json::Value>,
    /// Each rule the server left out, and why.
    #[serde(default)]
    pub rejected: Vec<String>,
}

#[derive(Deserialize)]
struct About {
    enabled: bool,
}

impl MeleteClient {
    /// Whether the server's AI features are on. A way in that doesn't reach
    /// `/api/v1/ai` (HTTP 403 or 404, as on a music-only one) reads as off.
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors), but for a 403 or a 404,
    /// which are `Ok(false)`.
    pub async fn ai_enabled(&self) -> Result<bool> {
        match self.get::<About>(&["api", "v1", "ai"], &[]).await {
            Ok(about) => Ok(about.enabled),
            Err(Error::NotFound { .. } | Error::Http { status: 403, .. }) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// `description` as filters over the library. Counts as one of the
    /// user's 30 AI searches an hour.
    ///
    /// # Errors
    ///
    /// [`Error::Http`] with status 503 while the server's AI is off, 400 for
    /// a description that's empty or over 300 characters, 429 past the hour's
    /// searches or while OpenRouter is busy, and 502 when OpenRouter failed,
    /// each with the server's words in `detail`; otherwise the
    /// [request errors](MeleteClient#errors).
    pub async fn music_filters(&self, description: &str) -> Result<MusicFilters> {
        let body = json!({ "query": description });
        self.send_decoded(
            Method::POST,
            &["api", "v1", "ai", "music", "search"],
            &[],
            Some(&body),
        )
        .await
    }

    /// A mood as smart-playlist rules, with the names it mentioned. Counts as
    /// one of the user's 30 AI searches an hour.
    ///
    /// # Errors
    ///
    /// As [`music_filters`](Self::music_filters).
    pub async fn music_mix(&self, description: &str) -> Result<MusicMix> {
        let body = json!({ "description": description });
        self.send_decoded(
            Method::POST,
            &["api", "v1", "ai", "music", "mix"],
            &[],
            Some(&body),
        )
        .await
    }
}
