//! Asking every provider at once and picking the best answer.

use std::sync::Arc;
use std::time::Duration;

use aoide_melete::Lyrics;
use futures_util::future::join_all;

use crate::lrclib::Lrclib;
use crate::provider::{Candidate, LyricsProvider, ProviderError, TrackQuery};

/// How long a provider has to answer. Lyrics that come later than this are
/// lyrics for a song that's half over.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(6);

/// How far a candidate's duration may be from the track's before it's taken
/// for another recording (a live take, a radio edit) and dropped. LRCLIB's own
/// exact match allows ±2 s; a little more covers rounding at both ends.
pub const DEFAULT_MAX_DURATION_GAP: Duration = Duration::from_secs(3);

/// The lyrics chosen for a track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Which provider they came from ([`LyricsProvider::name`]), to show as the
    /// source.
    pub provider: &'static str,
    /// Empty when `instrumental`.
    pub lyrics: Lyrics,
    /// The provider says the track has no words.
    pub instrumental: bool,
    /// How far the lyrics' recording is from the track's duration, when both
    /// are known.
    pub duration_gap: Option<Duration>,
}

/// Every provider failed, so "no lyrics" can't be concluded: try again later
/// rather than remembering the track as having none.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("no lyrics provider answered: {}", describe(.0))]
pub struct AllFailed(pub Vec<(&'static str, ProviderError)>);

fn describe(failures: &[(&'static str, ProviderError)]) -> String {
    failures
        .iter()
        .map(|(name, error)| format!("{name}: {error}"))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Internet lyrics, for when the Melete server has none.
///
/// The app asks [`MeleteClient::lyrics`](aoide_melete::MeleteClient::lyrics)
/// first and comes here only on `Ok(None)`. Turning internet lyrics off is the
/// app's switch: with it off, don't build a finder or don't call it. Nothing in
/// this crate makes a request on its own; building a finder sends nothing.
///
/// Providers are asked all at once, each with its own deadline. The answers are
/// ranked:
///
/// 1. Candidates whose duration is more than the allowed gap from the track's
///    are dropped (when both durations are known).
/// 2. Word-timed lyrics beat line-timed, which beat plain text, which beats an
///    "instrumental" verdict: synced lyrics from any provider are worth more than
///    plain ones from a preferred one.
/// 3. Then the preference order: the order the providers were given in.
/// 4. Then the closest duration, an unknown one last.
pub struct LyricsFinder {
    providers: Vec<Arc<dyn LyricsProvider>>,
    timeout: Duration,
    max_duration_gap: Duration,
}

impl std::fmt::Debug for LyricsFinder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<_> = self.providers.iter().map(|p| p.name()).collect();
        f.debug_struct("LyricsFinder")
            .field("providers", &names)
            .field("timeout", &self.timeout)
            .field("max_duration_gap", &self.max_duration_gap)
            .finish()
    }
}

impl LyricsFinder {
    /// A finder over `providers`, most preferred first.
    pub fn new(providers: Vec<Arc<dyn LyricsProvider>>) -> Self {
        Self {
            providers,
            timeout: DEFAULT_TIMEOUT,
            max_duration_gap: DEFAULT_MAX_DURATION_GAP,
        }
    }

    /// The finder with every provider this crate has: LRCLIB, for now the only
    /// one (the crate docs say why).
    ///
    /// # Errors
    ///
    /// [`ProviderError::Transport`] when the HTTP client can't be built (no
    /// TLS backend). Nothing is sent either way.
    pub fn with_default_providers() -> Result<Self, ProviderError> {
        Ok(Self::new(vec![Arc::new(Lrclib::new()?)]))
    }

    /// Each provider's deadline. [`DEFAULT_TIMEOUT`] unless changed.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The largest duration gap kept. [`DEFAULT_MAX_DURATION_GAP`] unless changed.
    pub fn max_duration_gap(mut self, gap: Duration) -> Self {
        self.max_duration_gap = gap;
        self
    }

    /// The provider names, in preference order.
    pub fn providers(&self) -> Vec<&'static str> {
        self.providers.iter().map(|p| p.name()).collect()
    }

    /// The best lyrics for `track`, or `Ok(None)` when every provider that
    /// answered had nothing that fits. `Err` only when none answered at all.
    ///
    /// A track without a title or an artist is `Ok(None)` without a request.
    ///
    /// # Errors
    ///
    /// [`AllFailed`], with each provider's [`ProviderError`] in preference
    /// order, when every provider failed or timed out. One provider answering,
    /// even with nothing, makes the result `Ok`.
    pub async fn find(&self, track: &TrackQuery) -> Result<Option<Found>, AllFailed> {
        if !track.is_searchable() || self.providers.is_empty() {
            return Ok(None);
        }
        let asks = self.providers.iter().map(|provider| async move {
            let answer = tokio::time::timeout(self.timeout, provider.search(track))
                .await
                .unwrap_or(Err(ProviderError::TimedOut));
            (provider.name(), answer)
        });
        let answers = join_all(asks).await;

        let mut failures = Vec::new();
        let mut ranked = Vec::new();
        for (preference, (name, answer)) in answers.into_iter().enumerate() {
            match answer {
                Ok(candidates) => ranked.extend(
                    candidates
                        .into_iter()
                        .map(|candidate| (preference, name, candidate)),
                ),
                Err(error) => failures.push((name, error)),
            }
        }
        if failures.len() == self.providers.len() {
            return Err(AllFailed(failures));
        }

        let best = ranked
            .into_iter()
            .filter_map(|(preference, name, candidate)| {
                let gap = gap(track.duration, candidate.duration);
                if gap.is_some_and(|gap| gap > self.max_duration_gap) {
                    return None;
                }
                Some((tier(&candidate), preference, gap, name, candidate))
            })
            // A known gap before an unknown one, then the smaller.
            .min_by_key(|(tier, preference, gap, _, _)| (*tier, *preference, gap.is_none(), *gap));
        Ok(best.map(|(_, _, duration_gap, provider, candidate)| Found {
            provider,
            lyrics: candidate.lyrics,
            instrumental: candidate.instrumental,
            duration_gap,
        }))
    }
}

/// Lower is better: word-timed, line-timed, plain, instrumental.
fn tier(candidate: &Candidate) -> u8 {
    if candidate.lyrics.has_word_timing() {
        0
    } else if candidate.lyrics.is_synced() {
        1
    } else if !candidate.lyrics.is_empty() {
        2
    } else {
        3
    }
}

fn gap(track: Option<Duration>, candidate: Option<Duration>) -> Option<Duration> {
    Some(track?.abs_diff(candidate?))
}
