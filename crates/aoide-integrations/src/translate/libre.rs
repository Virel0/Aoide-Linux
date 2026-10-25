//! Lyric translation by the listener's own LibreTranslate server.
//!
//! [LibreTranslate](https://github.com/LibreTranslate/LibreTranslate) is
//! self-hosted (AGPL-3.0, Argos Translate's models): the lyrics go to the
//! server the listener names and nowhere else. Two calls:
//!
//! - `GET /languages`: what the server can translate, for the settings
//!   check and to spell the target as the server does ("zh-Hans", not "zh").
//! - `POST /translate` with `q` as a list of lines, `source: "auto"`, and
//!   the API key in the body when the server wants one. The answer's
//!   `translatedText` is a list in the same order.
//!
//! Sent: the lyric lines, the target language and, if set, the key. Nothing
//! about the listener, the track or the machine.

use std::time::Duration;

use reqwest::{StatusCode, Url};
use serde::{Deserialize, Serialize};

/// CPU-only servers take a while over a whole song.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Lines per request: well under the batch limit an administrator might set.
const LINES_PER_REQUEST: usize = 32;

/// Most an answer may be: a song's lines, or the language list.
const MAX_BODY: usize = 4 << 20;

/// What a server answered in words, cut to this many characters.
const MESSAGE_CHARS: usize = 140;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LibreError {
    #[error("couldn't reach the translation server ({0})")]
    Unreachable(String),
    #[error("the translation server wants an API key, or didn't accept this one")]
    Unauthorised,
    #[error("the translation server is busy; try again in a minute")]
    Busy,
    #[error("the translation server can't translate into {0}")]
    Unsupported(String),
    #[error("the translation server said: {0}")]
    Refused(String),
    #[error("the translation server's answer wasn't understood ({0})")]
    Unreadable(String),
}

/// A language the server has.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Language {
    pub code: String,
    pub name: String,
}

#[derive(Serialize)]
struct Request<'a> {
    q: &'a [&'a str],
    source: &'a str,
    target: &'a str,
    format: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    api_key: Option<&'a str>,
}

#[derive(Deserialize)]
struct Translated {
    #[serde(rename = "translatedText")]
    translated_text: Vec<String>,
}

#[derive(Deserialize)]
struct Refusal {
    error: String,
}

/// A LibreTranslate server.
#[derive(Debug, Clone)]
pub struct LibreTranslate {
    http: reqwest::Client,
    base: Url,
    key: Option<String>,
}

impl LibreTranslate {
    /// `base` is the server's address (`https://translate.example.net/`, or
    /// one under a path); `key` its API key when it wants one.
    pub fn new(base: &str, key: Option<String>) -> Result<Self, LibreError> {
        crate::install_crypto();
        let mut base = Url::parse(base).map_err(|e| LibreError::Unreachable(e.to_string()))?;
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(same_origin_redirects())
            .build()
            .map_err(|e| LibreError::Unreachable(e.to_string()))?;
        let key = key.filter(|k| !k.trim().is_empty());
        Ok(Self { http, base, key })
    }

    fn url(&self, path: &str) -> Url {
        // `path` is one of this module's literals, always a valid relative URL.
        self.base.join(path).unwrap_or_else(|_| self.base.clone())
    }

    /// The languages the server has.
    pub async fn languages(&self) -> Result<Vec<Language>, LibreError> {
        let response = self
            .http
            .get(self.url("languages"))
            .send()
            .await
            .map_err(|e| LibreError::Unreachable(e.without_url().to_string()))?;
        let response = checked(response, true).await?;
        json(response).await
    }

    /// `lines` in `target` (ISO 639-1, as the locale gives it), in order;
    /// blank lines stay blank and aren't sent.
    pub async fn translate_lines(
        &self,
        lines: &[String],
        target: &str,
    ) -> Result<Vec<String>, LibreError> {
        let languages = self.languages().await?;
        let target = server_code(target, &languages)
            .ok_or_else(|| LibreError::Unsupported(target.to_owned()))?;
        let worded: Vec<(usize, &str)> = lines
            .iter()
            .enumerate()
            .map(|(i, line)| (i, line.trim()))
            .filter(|(_, line)| !line.is_empty())
            .collect();
        let mut out = vec![String::new(); lines.len()];
        for chunk in worded.chunks(LINES_PER_REQUEST) {
            let q: Vec<&str> = chunk.iter().map(|(_, line)| *line).collect();
            let translated = self.translate_batch(&q, &target).await?;
            if translated.len() != q.len() {
                return Err(LibreError::Unreadable(format!(
                    "{} lines back for {} sent",
                    translated.len(),
                    q.len()
                )));
            }
            for ((index, _), line) in chunk.iter().zip(translated) {
                out[*index] = line;
            }
        }
        Ok(out)
    }

    async fn translate_batch(&self, q: &[&str], target: &str) -> Result<Vec<String>, LibreError> {
        let request = Request {
            q,
            source: "auto",
            target,
            format: "text",
            api_key: self.key.as_deref(),
        };
        let response = self
            .http
            .post(self.url("translate"))
            .json(&request)
            .send()
            .await
            .map_err(|e| LibreError::Unreachable(e.without_url().to_string()))?;
        let response = checked(response, self.key.is_some()).await?;
        let body: Translated = json(response).await?;
        Ok(body.translated_text)
    }
}

/// The server's spelling of `code`: itself, else its first regional form
/// ("zh" → "zh-Hans"); `None` when the server hasn't the language.
pub fn server_code(code: &str, languages: &[Language]) -> Option<String> {
    let code = code.to_ascii_lowercase();
    languages
        .iter()
        .find(|l| l.code.eq_ignore_ascii_case(&code))
        .or_else(|| {
            languages.iter().find(|l| {
                l.code
                    .to_ascii_lowercase()
                    .strip_prefix(code.as_str())
                    .is_some_and(|rest| rest.starts_with('-'))
            })
        })
        .map(|l| l.code.clone())
}

/// The answer as JSON, read no further than [`MAX_BODY`].
async fn json<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, LibreError> {
    let unreadable = |e: reqwest::Error| LibreError::Unreadable(e.without_url().to_string());
    let too_long = || LibreError::Unreadable("the answer is too long".to_owned());
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BODY as u64)
    {
        return Err(too_long());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(unreadable)? {
        if body.len() + chunk.len() > MAX_BODY {
            return Err(too_long());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|e| LibreError::Unreadable(e.to_string()))
}

/// Redirects only within the server's own origin (scheme, host and port):
/// on a 307 or 308 reqwest sends the body again, API key and lyrics with
/// it, wherever `Location` points.
fn same_origin_redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let same = attempt
            .previous()
            .first()
            .is_some_and(|first| first.origin() == attempt.url().origin());
        if !same {
            attempt.stop()
        } else if attempt.previous().len() > 10 {
            attempt.error("too many redirects")
        } else {
            attempt.follow()
        }
    })
}

/// The response, or what its status means. The server's own words are
/// untrusted text, so only a short, single-line piece of them is kept.
/// `keyed`: a key was sent. A server set up to need keys refuses keyless
/// requests with 429 (a request limit of none), which means "no key" then.
async fn checked(
    response: reqwest::Response,
    keyed: bool,
) -> Result<reqwest::Response, LibreError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(LibreError::Unauthorised),
        StatusCode::TOO_MANY_REQUESTS if !keyed => Err(LibreError::Unauthorised),
        StatusCode::TOO_MANY_REQUESTS => Err(LibreError::Busy),
        _ => {
            let said = response
                .json::<Refusal>()
                .await
                .map(|r| r.error)
                .unwrap_or_else(|_| format!("HTTP {status}"));
            Err(LibreError::Refused(short(&said)))
        }
    }
}

fn short(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    let mut cut: String = line.chars().take(MESSAGE_CHARS).collect();
    if line.chars().count() > MESSAGE_CHARS {
        cut.push('…');
    }
    cut
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn languages() -> serde_json::Value {
        json!([
            {"code": "en", "name": "English", "targets": ["fr", "ja"]},
            {"code": "fr", "name": "French", "targets": ["en"]},
            {"code": "zh-Hans", "name": "Chinese (Simplified)", "targets": ["en"]}
        ])
    }

    async fn server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/languages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(languages()))
            .mount(&server)
            .await;
        server
    }

    #[test]
    fn the_target_is_spelled_as_the_server_spells_it() {
        let langs: Vec<Language> = serde_json::from_value(languages()).unwrap();
        assert_eq!(server_code("en", &langs).as_deref(), Some("en"));
        assert_eq!(server_code("zh", &langs).as_deref(), Some("zh-Hans"));
        assert_eq!(server_code("ZH", &langs).as_deref(), Some("zh-Hans"));
        assert_eq!(server_code("z", &langs), None);
        assert_eq!(server_code("de", &langs), None);
    }

    #[tokio::test]
    async fn lines_go_as_one_list_and_blank_lines_stay_blank() {
        let server = server().await;
        Mock::given(method("POST"))
            .and(path("/translate"))
            .and(body_partial_json(json!({
                "q": ["Sous le ciel", "de Paris"],
                "source": "auto",
                "target": "en",
                "format": "text",
                "api_key": "test-key"
            })))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"translatedText": ["Under the sky", "of Paris"]})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let libre = LibreTranslate::new(&server.uri(), Some("test-key".into())).unwrap();
        let lines = ["Sous le ciel", "", "de Paris"].map(str::to_owned);
        let out = libre.translate_lines(&lines, "en").await.unwrap();
        assert_eq!(out, ["Under the sky", "", "of Paris"]);
    }

    #[tokio::test]
    async fn no_key_is_sent_when_none_is_set() {
        let server = server().await;
        Mock::given(method("POST"))
            .and(path("/translate"))
            .respond_with(move |request: &wiremock::Request| {
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                assert!(body.get("api_key").is_none(), "{body}");
                ResponseTemplate::new(200).set_body_json(json!({"translatedText": ["x"]}))
            })
            .mount(&server)
            .await;
        let libre = LibreTranslate::new(&server.uri(), Some("  ".into())).unwrap();
        assert_eq!(
            libre.translate_lines(&["y".into()], "en").await.unwrap(),
            ["x"]
        );
    }

    #[tokio::test]
    async fn long_lyrics_go_in_several_requests_in_order() {
        let server = server().await;
        Mock::given(method("POST"))
            .and(path("/translate"))
            .respond_with(|request: &wiremock::Request| {
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                let out: Vec<String> = body["q"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|l| format!("<{}>", l.as_str().unwrap()))
                    .collect();
                ResponseTemplate::new(200).set_body_json(json!({"translatedText": out}))
            })
            .expect(3)
            .mount(&server)
            .await;
        let libre = LibreTranslate::new(&server.uri(), None).unwrap();
        let lines: Vec<String> = (0..70).map(|n| format!("line {n}")).collect();
        let out = libre.translate_lines(&lines, "en").await.unwrap();
        assert_eq!(out.len(), 70);
        assert_eq!(out[0], "<line 0>");
        assert_eq!(out[69], "<line 69>");
    }

    #[tokio::test]
    async fn refusals_are_said_in_words() {
        let server = server().await;
        Mock::given(method("POST"))
            .and(path("/translate"))
            .respond_with(
                ResponseTemplate::new(403).set_body_json(json!({"error": "Invalid API key"})),
            )
            .mount(&server)
            .await;
        let libre = LibreTranslate::new(&server.uri(), None).unwrap();
        let lines = ["y".to_owned()];
        assert_eq!(
            libre.translate_lines(&lines, "en").await,
            Err(LibreError::Unauthorised)
        );
        assert_eq!(
            libre.translate_lines(&lines, "de").await,
            Err(LibreError::Unsupported("de".into()))
        );

        // 429 is "busy" with a key, and "a key is needed" without one.
        let busy = server_with(ResponseTemplate::new(429)).await;
        let libre = LibreTranslate::new(&busy.uri(), Some("k".into())).unwrap();
        assert_eq!(
            libre.translate_lines(&lines, "en").await,
            Err(LibreError::Busy)
        );
        let libre = LibreTranslate::new(&busy.uri(), None).unwrap();
        assert_eq!(
            libre.translate_lines(&lines, "en").await,
            Err(LibreError::Unauthorised)
        );

        let long = "x".repeat(500) + "\nsecond line";
        let refused =
            server_with(ResponseTemplate::new(400).set_body_json(json!({ "error": long }))).await;
        let libre = LibreTranslate::new(&refused.uri(), None).unwrap();
        match libre.translate_lines(&lines, "en").await {
            Err(LibreError::Refused(said)) => {
                assert_eq!(said.chars().count(), MESSAGE_CHARS + 1);
                assert!(!said.contains('\n'));
            }
            other => panic!("{other:?}"),
        }
    }

    async fn server_with(translate: ResponseTemplate) -> MockServer {
        let server = server().await;
        Mock::given(method("POST"))
            .and(path("/translate"))
            .respond_with(translate)
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn a_mismatched_answer_is_not_shown_against_the_wrong_lines() {
        let server = server_with(
            ResponseTemplate::new(200).set_body_json(json!({"translatedText": ["only one"]})),
        )
        .await;
        let libre = LibreTranslate::new(&server.uri(), None).unwrap();
        let lines = ["a".to_owned(), "b".to_owned()];
        assert!(matches!(
            libre.translate_lines(&lines, "en").await,
            Err(LibreError::Unreadable(_))
        ));
    }

    #[tokio::test]
    async fn a_server_under_a_path_is_asked_under_it() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/lt/languages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(languages()))
            .expect(1)
            .mount(&server)
            .await;
        let libre = LibreTranslate::new(&format!("{}/lt", server.uri()), None).unwrap();
        assert_eq!(libre.languages().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn the_key_and_lyrics_never_follow_a_redirect_elsewhere() {
        let elsewhere = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"translatedText": ["x"]})),
            )
            .expect(0)
            .mount(&elsewhere)
            .await;
        let server = server_with(
            ResponseTemplate::new(307)
                .insert_header("Location", format!("{}/collect", elsewhere.uri())),
        )
        .await;
        let libre = LibreTranslate::new(&server.uri(), Some("secret".into())).unwrap();
        assert!(libre.translate_lines(&["y".into()], "en").await.is_err());
    }
}
