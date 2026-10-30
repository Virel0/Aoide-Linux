//! The provider trait, and what goes in and out of one.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use aoide_melete::{Item, Lyrics};

/// A boxed future, so providers can sit side by side as trait objects.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A source of lyrics on the internet.
///
/// A provider makes requests only from [`search`](Self::search); building one
/// sends nothing.
pub trait LyricsProvider: Send + Sync {
    /// A short, stable name: shown as the lyrics' source, and how the
    /// preference order refers to it.
    fn name(&self) -> &'static str;

    /// Every candidate this source has for `track`, best first by its own
    /// judgement; none is `Ok(vec![])`. The finder does the ranking.
    fn search<'a>(
        &'a self,
        track: &'a TrackQuery,
    ) -> BoxFuture<'a, Result<Vec<Candidate>, ProviderError>>;
}

/// The track to find lyrics for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrackQuery {
    /// The track's title, as the server has it.
    pub title: String,
    /// The track's (first) artist, rather than the album artist: a compilation's
    /// album artist is "Various Artists", which matches nothing.
    pub artist: String,
    /// The album, when known. Narrows an exact lookup; a search goes without it.
    pub album: Option<String>,
    /// The track's length, when known. Candidates timed against a recording
    /// too far from it are dropped ([`DEFAULT_MAX_DURATION_GAP`](crate::DEFAULT_MAX_DURATION_GAP)).
    pub duration: Option<Duration>,
}

impl TrackQuery {
    /// The query for a Melete track: its name, first artist (else album artist),
    /// album and duration.
    pub fn from_item(item: &Item) -> Self {
        let artist = item
            .artists
            .first()
            .cloned()
            .or_else(|| item.album_artist.clone())
            .unwrap_or_default();
        Self {
            title: item.name.clone(),
            artist,
            album: item.album.clone(),
            duration: item.duration(),
        }
    }

    /// A title and an artist are the least any provider can search with.
    pub fn is_searchable(&self) -> bool {
        !self.title.trim().is_empty() && !self.artist.trim().is_empty()
    }
}

/// One set of lyrics a provider found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The lyrics, with line and word times when the provider had them.
    pub lyrics: Lyrics,
    /// The provider says the track has no words. `lyrics` is then empty.
    pub instrumental: bool,
    /// The length of the recording the lyrics were timed against, if known.
    pub duration: Option<Duration>,
    /// The title the provider matched, for showing and for debugging a bad
    /// match.
    pub title: String,
    /// The artist the provider matched.
    pub artist: String,
    /// The album the provider matched, if it names one.
    pub album: Option<String>,
}

/// Why one provider gave no answer. `Clone` and `Eq` so it can travel inside UI
/// messages, like aoide-melete's errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    /// No HTTP answer: DNS, refused connection, TLS.
    #[error("{0}")]
    Transport(String),
    /// A non-2xx answer other than the provider's "not found".
    #[error("HTTP {status}")]
    Http {
        /// The status code the provider answered with.
        status: u16,
    },
    /// The provider asked us to wait (HTTP 429, or 503 with `Retry-After`). It
    /// sends nothing more until the wait is over: asking again sooner gets this
    /// error back at once, with what is left of the wait.
    #[error("asked to wait {}s", .retry_after.as_secs())]
    RateLimited {
        /// How much of the wait is left.
        retry_after: Duration,
    },
    /// A 2xx answer that didn't decode.
    #[error("couldn't read the answer: {0}")]
    Decoding(String),
    /// No answer within the finder's timeout.
    #[error("no answer in time")]
    TimedOut,
}
