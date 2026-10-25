//! Translating lyrics on this machine with Firefox Translations' models,
//! through the `fxtranslate` engine (MPL-2.0, pure Rust; its optional SIMD
//! kernel is C++ and falls back to Rust where it cannot be built).
//!
//! Every model translates to or from English; another pair goes through
//! English in two steps. A pair's files (a Marian model and its
//! SentencePiece vocabulary) are fetched on first use, verified against the
//! SHA-256 their catalogue gives, and kept under the cache directory the app
//! names. After that the pair works offline, and nothing is fetched again.
//!
//! **What is fetched, from where:** the model catalogue and the model files,
//! from Mozilla's Remote Settings (`firefox.settings.services.mozilla.com`
//! and its attachment CDN) or from a mirror ([`ModelSource::Mirror`]). Those
//! requests carry nothing about the listener or the lyrics. fxtranslate's
//! author notes that Mozilla provisions that hosting for Firefox, not for
//! other programs, and suggests mirroring the files a program depends on.
//!
//! Everything here blocks: call it from a blocking thread
//! (`tokio::task::spawn_blocking`), never from async code directly.

use std::io::{Read, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use fxtranslate::cache::{Cache, ModelFiles};
use fxtranslate::engine::{Engine, Translation};
use fxtranslate::fetch::{DownloadOutcome, Fetch, FetchError, Sink};
use fxtranslate::loader::load_translation;
use fxtranslate::remote::{CDN_ROOT, PROD_ENDPOINT};
use fxtranslate::route::PREFERRED_HUB;

/// Where model files come from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ModelSource {
    /// Mozilla's Remote Settings and its CDN, as Firefox fetches them.
    #[default]
    Mozilla,
    /// A copy of both under one `https://` address: the catalogue at
    /// `<base>/settings/…` and the files at `<base>/attachments/…`, each with
    /// the same path as on Mozilla's hosts.
    Mirror(String),
}

#[derive(Debug, thiserror::Error)]
pub enum TranslateError {
    #[error("no translation model for {from} to {to}: {reason}")]
    NoModel {
        from: String,
        to: String,
        reason: String,
    },
    #[error("could not set up the translation client: {0}")]
    Client(String),
}

/// How long one request may take. A model is tens of megabytes on a slow
/// line; the download resumes after a failure anyway.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// fxtranslate's [`Fetch`] over reqwest's blocking client, so no second
/// HTTP stack and TLS setup come in. Rewrites Mozilla's addresses to a
/// mirror's when there is one.
struct ReqwestFetch {
    http: reqwest::blocking::Client,
    source: ModelSource,
}

impl ReqwestFetch {
    fn new(source: ModelSource) -> Result<Self, TranslateError> {
        crate::install_crypto();
        let http = reqwest::blocking::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| TranslateError::Client(e.to_string()))?;
        Ok(Self { http, source })
    }

    fn address(&self, url: &str) -> String {
        rewrite(&self.source, url)
    }
}

/// `url` as it is fetched from `source`.
fn rewrite(source: &ModelSource, url: &str) -> String {
    let ModelSource::Mirror(base) = source else {
        return url.to_owned();
    };
    let base = base.trim_end_matches('/');
    if let Some(rest) = url.strip_prefix(PROD_ENDPOINT) {
        format!("{base}/settings{rest}")
    } else if let Some(rest) = url.strip_prefix(CDN_ROOT) {
        format!("{base}/attachments{rest}")
    } else {
        url.to_owned()
    }
}

fn retryable_status(code: u16) -> bool {
    code == 429 || code >= 500
}

impl Fetch for ReqwestFetch {
    fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        let url = self.address(url);
        let response = self
            .http
            .get(&url)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|e| format!("GET {url}: {e}"))?;
        response
            .bytes()
            .map(|b| b.to_vec())
            .map_err(|e| format!("reading {url}: {e}"))
    }

    fn get_to(
        &self,
        url: &str,
        range_from: u64,
        sink: &mut dyn Sink,
        on_progress: &mut dyn FnMut(u64, Option<u64>),
    ) -> Result<DownloadOutcome, FetchError> {
        let url = self.address(url);
        let mut request = self.http.get(&url);
        if range_from > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={range_from}-"));
        }
        let mut response = request.send().map_err(|e| FetchError {
            message: format!("GET {url}: {e}"),
            retryable: true,
        })?;
        let status = response.status();
        if !status.is_success() {
            return Err(FetchError {
                message: format!("GET {url}: HTTP {status}"),
                retryable: retryable_status(status.as_u16()),
            });
        }
        // 206: the server honoured the range, append; 200: the whole body.
        let resumed = status.as_u16() == 206;
        let base = if resumed { range_from } else { 0 };
        let total = response.content_length().map(|n| n + base);
        sink.seek(SeekFrom::Start(base)).map_err(|e| FetchError {
            message: format!("seek: {e}"),
            retryable: false,
        })?;
        let mut done = base;
        on_progress(done, total);
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let n = response.read(&mut buffer).map_err(|e| FetchError {
                message: format!("reading {url}: {e}"),
                retryable: true,
            })?;
            if n == 0 {
                break;
            }
            sink.write_all(&buffer[..n]).map_err(|e| FetchError {
                message: format!("writing: {e}"),
                retryable: false,
            })?;
            done += n as u64;
            on_progress(done, total);
        }
        Ok(DownloadOutcome { resumed, total })
    }
}

/// A request to the translation thread.
enum Job {
    Translate {
        lines: Vec<String>,
        from: String,
        to: String,
        reply: mpsc::Sender<Result<Vec<String>, TranslateError>>,
    },
    Unload,
}

/// Translates lyric lines on this machine.
///
/// The engine lives on a thread of its own: with its SIMD kernel,
/// fxtranslate's model may not move between threads, and the blocking HTTP
/// client must not be made or dropped inside an async runtime. This handle
/// is `Send` and `Sync`; [`LocalTranslator::translate_lines`] blocks until
/// the thread answers. The last pair's model stays loaded for the next song
/// (about 150 MB); [`LocalTranslator::unload`] frees it. Dropping the handle
/// ends the thread.
#[derive(Debug)]
pub struct LocalTranslator {
    jobs: mpsc::Sender<Job>,
    cache_dir: PathBuf,
    source: ModelSource,
}

impl LocalTranslator {
    /// Models are kept under `cache_dir` (the app's cache directory, e.g.
    /// `~/.cache/aoide/translation-models`). Nothing is fetched until a
    /// translation needs a model that is not there.
    pub fn new(cache_dir: impl Into<PathBuf>, source: ModelSource) -> Result<Self, TranslateError> {
        let cache_dir = cache_dir.into();
        let (jobs, inbox) = mpsc::channel();
        let worker_dir = cache_dir.clone();
        let worker_source = source.clone();
        std::thread::Builder::new()
            .name("aoide-translate".into())
            .spawn(move || work(&worker_dir, worker_source, &inbox))
            .map_err(|e| TranslateError::Client(e.to_string()))?;
        Ok(Self {
            jobs,
            cache_dir,
            source,
        })
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    pub fn source(&self) -> &ModelSource {
        &self.source
    }

    /// Whether `from` to `to` can be translated without fetching anything:
    /// a direct model, or both legs through English, already cached.
    pub fn is_downloaded(&self, from: &str, to: &str) -> bool {
        let cache = Cache::with_root(&self.cache_dir);
        let hub = PREFERRED_HUB;
        cache.cached_model(from, to).is_some()
            || (from != hub
                && to != hub
                && cache.cached_model(from, hub).is_some()
                && cache.cached_model(hub, to).is_some())
    }

    /// Frees the loaded model.
    pub fn unload(&self) {
        let _ = self.jobs.send(Job::Unload);
    }

    /// `lines` translated from `from` into `to`, one output line per input
    /// line; blank lines stay blank. The first use of a pair downloads its
    /// model. Blocks; see the module docs.
    pub fn translate_lines(
        &self,
        lines: &[String],
        from: &str,
        to: &str,
    ) -> Result<Vec<String>, TranslateError> {
        if from == to {
            return Ok(lines.to_vec());
        }
        let (reply, answer) = mpsc::channel();
        let job = Job::Translate {
            lines: lines.to_vec(),
            from: from.to_owned(),
            to: to.to_owned(),
            reply,
        };
        let gone = || TranslateError::Client("the translation thread stopped".into());
        self.jobs.send(job).map_err(|_| gone())?;
        answer.recv().map_err(|_| gone())?
    }
}

/// The loaded pair, kept for the next song in the same language.
struct Loaded {
    from: String,
    to: String,
    translation: Translation,
}

/// The translation thread: owns the HTTP client, the cache and the model.
fn work(cache_dir: &Path, source: ModelSource, inbox: &mpsc::Receiver<Job>) {
    let cache = Cache::with_root(cache_dir);
    let mut fetch: Option<ReqwestFetch> = None;
    let mut loaded: Option<Loaded> = None;
    while let Ok(job) = inbox.recv() {
        let (lines, from, to, reply) = match job {
            Job::Unload => {
                loaded = None;
                continue;
            }
            Job::Translate {
                lines,
                from,
                to,
                reply,
            } => (lines, from, to, reply),
        };
        let current = loaded
            .as_ref()
            .is_some_and(|l| l.from == from && l.to == to);
        if !current {
            // Free the old model before the new one loads.
            loaded = None;
            match load(&cache, &mut fetch, &source, &from, &to) {
                Ok(translation) => {
                    loaded = Some(Loaded {
                        from: from.clone(),
                        to: to.clone(),
                        translation,
                    });
                }
                Err(e) => {
                    let _ = reply.send(Err(e));
                    continue;
                }
            }
        }
        let Some(l) = loaded.as_ref() else { continue };
        let out = lines
            .iter()
            .map(|line| match line.trim() {
                "" => String::new(),
                text => l.translation.translate_long(text),
            })
            .collect();
        let _ = reply.send(Ok(out));
    }
}

fn load(
    cache: &Cache,
    fetch: &mut Option<ReqwestFetch>,
    source: &ModelSource,
    from: &str,
    to: &str,
) -> Result<Translation, TranslateError> {
    let no_model = |reason: String| TranslateError::NoModel {
        from: from.to_owned(),
        to: to.to_owned(),
        reason,
    };
    // The cache first, so a pair already here never touches the network.
    if let Some(cached) = cached_translation(cache, from, to) {
        return cached.map_err(no_model);
    }
    let fetch = match fetch {
        Some(fetch) => fetch,
        None => fetch.insert(ReqwestFetch::new(source.clone())?),
    };
    load_translation(fetch, cache, from, to).map_err(no_model)
}

fn engine(files: &ModelFiles) -> Result<Engine, String> {
    Engine::load_mmapped(&files.model, &files.src_vocab, &files.trg_vocab)
}

/// The pair from the cache alone: a direct model, else both legs through
/// English. `None` when not everything is here.
fn cached_translation(cache: &Cache, from: &str, to: &str) -> Option<Result<Translation, String>> {
    if let Some(files) = cache.cached_model(from, to) {
        return Some(engine(&files).map(Translation::Direct));
    }
    let hub = PREFERRED_HUB;
    if from == hub || to == hub {
        return None;
    }
    let first = cache.cached_model(from, hub)?;
    let second = cache.cached_model(hub, to)?;
    Some((|| {
        Ok(Translation::Pivot {
            pivot: hub.to_owned(),
            first: engine(&first)?,
            second: engine(&second)?,
        })
    })())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mozillas_addresses_are_used_as_they_are_by_default() {
        let records = format!("{PROD_ENDPOINT}/v1/buckets/main/collections/x/records");
        assert_eq!(rewrite(&ModelSource::Mozilla, &records), records);
    }

    #[test]
    fn a_mirror_takes_both_hosts() {
        let mirror = ModelSource::Mirror("https://models.example.org/aoide/".into());
        assert_eq!(
            rewrite(
                &mirror,
                &format!("{PROD_ENDPOINT}/v1/buckets/main/collections/x/records")
            ),
            "https://models.example.org/aoide/settings/v1/buckets/main/collections/x/records"
        );
        assert_eq!(
            rewrite(
                &mirror,
                &format!("{CDN_ROOT}/main-workspace/x/model.bin.zst")
            ),
            "https://models.example.org/aoide/attachments/main-workspace/x/model.bin.zst"
        );
        assert_eq!(
            rewrite(&mirror, "https://elsewhere.example.org/a"),
            "https://elsewhere.example.org/a"
        );
    }

    #[test]
    fn the_handle_can_go_to_a_blocking_task() {
        fn send_and_sync<T: Send + Sync>() {}
        send_and_sync::<LocalTranslator>();
    }

    #[test]
    fn an_empty_cache_has_nothing_downloaded() {
        let dir = tempfile::tempdir().unwrap();
        let translator = LocalTranslator::new(dir.path(), ModelSource::Mozilla).unwrap();
        assert!(!translator.is_downloaded("fr", "en"));
        assert!(!translator.is_downloaded("fr", "de"));
        assert_eq!(translator.cache_dir(), dir.path());
    }

    #[test]
    fn the_same_language_is_returned_as_it_is_without_a_model() {
        let dir = tempfile::tempdir().unwrap();
        let translator = LocalTranslator::new(dir.path(), ModelSource::Mozilla).unwrap();
        let lines = vec!["Bonjour".to_owned(), String::new()];
        assert_eq!(
            translator.translate_lines(&lines, "fr", "fr").unwrap(),
            lines
        );
    }

    #[test]
    fn a_missing_model_with_no_catalogue_is_an_error_not_a_panic() {
        // A mirror on a port nothing listens on: the catalogue cannot be
        // fetched and the cache is empty. Stays on this machine.
        let address = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", listener.local_addr().unwrap())
        };
        let dir = tempfile::tempdir().unwrap();
        let translator = LocalTranslator::new(dir.path(), ModelSource::Mirror(address)).unwrap();
        let error = translator
            .translate_lines(&["Bonjour tout le monde".into()], "fr", "en")
            .unwrap_err();
        assert!(matches!(error, TranslateError::NoModel { .. }), "{error}");
    }

    /// A real model, if one is on disk: set `AOIDE_TRANSLATION_MODELS` to a
    /// cache directory holding `fr-en/` (e.g. from `fxtranslate models add
    /// fr-en`). Ignored by default; it never downloads.
    #[test]
    #[ignore = "needs a downloaded fr-en model in AOIDE_TRANSLATION_MODELS"]
    fn translates_french_with_a_real_model() {
        let dir = std::env::var_os("AOIDE_TRANSLATION_MODELS")
            .expect("AOIDE_TRANSLATION_MODELS names a model cache");
        let translator = LocalTranslator::new(dir, ModelSource::Mozilla).unwrap();
        assert!(translator.is_downloaded("fr", "en"));
        let lines = vec!["Le chat dort sur le canapé.".to_owned(), String::new()];
        let out = translator.translate_lines(&lines, "fr", "en").unwrap();
        assert_eq!(out.len(), 2);
        assert!(out[0].to_lowercase().contains("cat"), "{out:?}");
        assert_eq!(out[1], "");
    }
}
