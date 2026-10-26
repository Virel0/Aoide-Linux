//! LRCLIB (<https://lrclib.net>): a free, open database of synced lyrics with a
//! documented, keyless API.
//!
//! Two endpoints are used, read-only: `GET /api/get`, an exact match on track,
//! artist, album and duration (±2 s, per LRCLIB's docs), and, only when that
//! finds nothing, `GET /api/search` on track and artist, a moment later.
//!
//! LRCLIB's API terms ask clients to identify themselves, honour `Retry-After`,
//! limit concurrent requests and cache results. Every request here names the app
//! in its `User-Agent`; a `Retry-After` stops all requests from this provider
//! until it has passed (plus a little jitter); one lookup makes at most two
//! requests, one after the other. Caching what was found is the app's: look a
//! track up once, keep the answer, and don't look up tracks nobody is playing.

use std::sync::{Mutex, Once};
use std::time::{Duration, Instant, SystemTime};

use reqwest::header::RETRY_AFTER;
use reqwest::{StatusCode, Url};
use serde::Deserialize;

use crate::lrc::{parse_lrc, parse_plain};
use crate::provider::{BoxFuture, Candidate, LyricsProvider, ProviderError, TrackQuery};

/// The public service.
pub const LRCLIB_URL: &str = "https://lrclib.net";

/// Who is asking: the app, its version and where it lives.
pub const USER_AGENT: &str = concat!(
    "Aoide for Linux/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/Virel0/Aoide-Linux)"
);

/// A connection that takes longer than this isn't going to make the finder's
/// deadline anyway.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// LRCLIB's `/api/get` takes durations from 1 s to an hour.
const MAX_QUERY_DURATION: u64 = 3_600;

/// The pause between the exact lookup and the search that follows it. LRCLIB
/// asks for 200–500 ms between requests.
const BETWEEN_REQUESTS: Duration = Duration::from_millis(250);

/// How long to hold off after a 429 that names no wait.
const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(30);

/// The LRCLIB provider.
#[derive(Debug)]
pub struct Lrclib {
    http: reqwest::Client,
    base: Url,
    /// Until when LRCLIB asked us to wait.
    blocked_until: Mutex<Option<Instant>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    #[serde(default)]
    track_name: Option<String>,
    #[serde(default)]
    artist_name: Option<String>,
    #[serde(default)]
    album_name: Option<String>,
    /// Seconds, with a fraction.
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    instrumental: bool,
    #[serde(default)]
    plain_lyrics: Option<String>,
    #[serde(default)]
    synced_lyrics: Option<String>,
}

impl Lrclib {
    /// The provider for lrclib.net. Sends nothing until searched.
    pub fn new() -> Result<Self, ProviderError> {
        Self::with_base_url(Url::parse(LRCLIB_URL).expect("LRCLIB_URL is a URL"))
    }

    /// The provider against another LRCLIB server (LRCLIB's server is open
    /// source, and the tests use a local stand-in).
    pub fn with_base_url(base: Url) -> Result<Self, ProviderError> {
        install_crypto();
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(transport)?;
        Ok(Self {
            http,
            base,
            blocked_until: Mutex::new(None),
        })
    }

    /// `Err(RateLimited)` with the rest of the wait while LRCLIB's `Retry-After`
    /// hasn't passed.
    fn check_not_blocked(&self) -> Result<(), ProviderError> {
        let mut blocked = self.blocked_until.lock().unwrap_or_else(|e| e.into_inner());
        match *blocked {
            Some(until) if until > Instant::now() => Err(ProviderError::RateLimited {
                retry_after: until - Instant::now(),
            }),
            Some(_) => {
                *blocked = None;
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// Holds every request off for `wait` plus up to 30 % jitter, as LRCLIB asks,
    /// so throttled clients don't come back all at once.
    fn block_for(&self, wait: Duration) -> Duration {
        let wait = wait + jitter(wait);
        let mut blocked = self.blocked_until.lock().unwrap_or_else(|e| e.into_inner());
        *blocked = Some(Instant::now() + wait);
        wait
    }

    fn endpoint(&self, name: &str, query: &[(&str, String)]) -> Url {
        let mut url = self.base.clone();
        if let Ok(mut path) = url.path_segments_mut() {
            path.pop_if_empty().extend(["api", name]);
        }
        {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }
        url
    }

    /// `GET`, with LRCLIB's 404 ("TrackNotFound") as `None`.
    async fn fetch<T: for<'de> Deserialize<'de>>(
        &self,
        url: Url,
    ) -> Result<Option<T>, ProviderError> {
        self.check_not_blocked()?;
        let response = self.http.get(url).send().await.map_err(transport)?;
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        match response.status() {
            StatusCode::NOT_FOUND => return Ok(None),
            StatusCode::TOO_MANY_REQUESTS => {
                let wait = self.block_for(retry_after.unwrap_or(DEFAULT_RETRY_AFTER));
                return Err(ProviderError::RateLimited { retry_after: wait });
            }
            StatusCode::SERVICE_UNAVAILABLE if retry_after.is_some() => {
                let wait = self.block_for(retry_after.unwrap_or_default());
                return Err(ProviderError::RateLimited { retry_after: wait });
            }
            status if !status.is_success() => {
                return Err(ProviderError::Http {
                    status: status.as_u16(),
                });
            }
            _ => {}
        }
        let bytes = response.bytes().await.map_err(transport)?;
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| ProviderError::Decoding(e.to_string()))
    }

    async fn find(&self, track: &TrackQuery) -> Result<Vec<Candidate>, ProviderError> {
        if !track.is_searchable() {
            return Ok(Vec::new());
        }
        let mut exact = vec![
            ("track_name", track.title.trim().to_owned()),
            ("artist_name", track.artist.trim().to_owned()),
        ];
        if let Some(album) = track
            .album
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
        {
            exact.push(("album_name", album.to_owned()));
        }
        if let Some(seconds) = track
            .duration
            .map(|d| d.as_secs_f64().round() as u64)
            .filter(|s| (1..=MAX_QUERY_DURATION).contains(s))
        {
            exact.push(("duration", seconds.to_string()));
        }
        if let Some(record) = self.fetch::<Record>(self.endpoint("get", &exact)).await?
            && let Some(candidate) = candidate(record)
        {
            return Ok(vec![candidate]);
        }

        tokio::time::sleep(BETWEEN_REQUESTS).await;
        let loose = [
            ("track_name", track.title.trim().to_owned()),
            ("artist_name", track.artist.trim().to_owned()),
        ];
        let records: Vec<Record> = self
            .fetch(self.endpoint("search", &loose))
            .await?
            .unwrap_or_default();
        Ok(records.into_iter().filter_map(candidate).collect())
    }
}

impl LyricsProvider for Lrclib {
    fn name(&self) -> &'static str {
        "LRCLIB"
    }

    fn search<'a>(
        &'a self,
        track: &'a TrackQuery,
    ) -> BoxFuture<'a, Result<Vec<Candidate>, ProviderError>> {
        Box::pin(self.find(track))
    }
}

/// A record as a candidate: synced lyrics where there are some, else plain. A
/// record with neither, and not marked instrumental, is no candidate.
fn candidate(record: Record) -> Option<Candidate> {
    let synced = record
        .synced_lyrics
        .as_deref()
        .map(parse_lrc)
        .filter(|l| l.is_synced());
    let lyrics = synced.or_else(|| {
        record
            .plain_lyrics
            .as_deref()
            .map(parse_plain)
            .filter(|l| !l.is_empty())
    });
    if lyrics.is_none() && !record.instrumental {
        return None;
    }
    Some(Candidate {
        lyrics: lyrics.unwrap_or_default(),
        instrumental: record.instrumental,
        duration: record
            .duration
            .filter(|d| d.is_finite() && *d > 0.0)
            .and_then(|d| Duration::try_from_secs_f64(d).ok()),
        title: record.track_name.unwrap_or_default(),
        artist: record.artist_name.unwrap_or_default(),
        album: record.album_name,
    })
}

/// Up to 30 % of `wait`. Not random enough for anything but spreading retries,
/// which is all it's for.
fn jitter(wait: Duration) -> Duration {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    wait.mul_f64(f64::from(nanos % 300) / 1_000.0)
}

/// rustls with the ring provider, as every crate in the workspace installs it.
fn install_crypto() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// A reqwest failure with its causes spelled out and the URL left out: the URL
/// carries what's playing.
fn transport(error: reqwest::Error) -> ProviderError {
    let error = error.without_url();
    let mut message = error.to_string();
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        let text = cause.to_string();
        if !message.contains(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }
        source = cause.source();
    }
    ProviderError::Transport(message)
}
