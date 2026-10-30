//! Importing a playlist from elsewhere — a Spotify link or a CSV export — and deciding
//! which of its tracks this library has.
//!
//! The steps, for the app:
//!
//! 1. Read the list: [`fetch_spotify_playlist`] for a pasted Spotify link (the embed
//!    page, so no account and no key, and at most the first 100 tracks —
//!    [`ImportedPlaylist::is_truncated`] says when there may be more), or
//!    [`parse_playlist_csv`] for an Exportify-style file.
//! 2. Match it: [`match_playlist`] asks the sidecar's `POST /aoide/match` first — one
//!    request, answered from inside the library — and when that is unreachable falls
//!    back to the same rules here ([`TrackMatcher`]) over the library's search,
//!    which the app supplies through [`LibrarySearch`] so this crate does not depend
//!    on a library client.
//! 3. Show what was found and what was not, and save the found ones as an Aoide
//!    playlist (`Store::create_playlist` and `Store::add_tracks`).
//!
//! The matching rules are a specification shared by the phone (`TrackMatcher.swift`),
//! the desktop (`playlist-import.ts`) and the sidecar, all held to one table of cases
//! (`tests/match-table.json` here, a copy of the desktop's `docs/match-table.json`):
//! a track that matches on one must match on all, or two devices would disagree about
//! what is missing from the same playlist.

use std::future::Future;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use crate::client::SidecarClient;
use crate::error::SidecarError;

// MARK: - What is imported

/// A playlist as another service describes it: names and lengths, no ids we can use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedPlaylist {
    /// The playlist's name at the source, or the name given for a CSV file.
    pub name: String,
    /// The tracks in playlist order. Rows without a title are left out.
    pub tracks: Vec<ImportedTrack>,
    /// The source is known to hold more than it handed over: the Spotify embed page
    /// stops at a hundred tracks and says nothing about it.
    pub is_truncated: bool,
}

/// One track of an imported playlist, spelled as `POST /aoide/match` reads it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedTrack {
    /// The title as the source spells it, noise and all ("Song - Remastered").
    pub title: String,
    /// Every credited artist, the main one first.
    pub artists: Vec<String>,
    /// The album, when the source names one (CSV exports do, the Spotify embed
    /// page doesn't).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album: Option<String>,
    /// The track's length in milliseconds, when the source gives it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    /// Only CSV exports carry it. Decisive when the library has it too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isrc: Option<String>,
}

impl ImportedTrack {
    /// A track with a title, its artists (main one first) and maybe a length,
    /// and no album or ISRC.
    pub fn new(title: impl Into<String>, artists: &[&str], duration_ms: Option<i64>) -> Self {
        Self {
            title: title.into(),
            artists: artists.iter().map(|artist| (*artist).to_owned()).collect(),
            album: None,
            duration_ms,
            isrc: None,
        }
    }
}

/// A library track, as the library's search returned it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryTrack {
    /// The track's id in the library.
    pub jellyfin_id: String,
    /// The track's title, as tagged.
    pub title: String,
    /// The track's artists, with the album artist added by the caller when there is
    /// one: either may be the name the other service credits.
    pub artists: Vec<String>,
    /// The album title, when tagged.
    pub album: Option<String>,
    /// The track's length in milliseconds, when known.
    pub duration_ms: Option<i64>,
    /// The track's ISRC, when the library knows one.
    pub isrc: Option<String>,
}

// MARK: - The rules

/// How one library track compares with one imported track. Every score is 0…1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Verdict {
    /// How alike the normalised titles are.
    pub title: f64,
    /// The best likeness over every pair of credited names; 0.5 when either
    /// side has none.
    pub artist: f64,
    /// 1 within 5 s, 0.8 within 15 s, 0 beyond (a reject), 0.9 when either
    /// length is unknown.
    pub duration: f64,
    /// Whether the candidate counts as the same recording, by the rules on
    /// [`TrackMatcher`].
    pub is_accepted: bool,
    /// Only for ranking accepted candidates: 0.5·title + 0.35·artist + 0.15·duration,
    /// or 2 above that when the ISRCs agree, so an ISRC match outranks every fuzzy one.
    pub overall: f64,
}

/// The shared matching rules.
///
/// Normalisation, in order: lower-case, then fold diacritics ("Beyoncé" → "beyonce");
/// "&" → " and "; drop a bracketed group — `(…)` or `[…]` — whose text contains a
/// noise word (a group without one, like "(What's the Story)", stays); drop a
/// ` - …` suffix that contains a noise word; keep only letters, digits and spaces.
///
/// Scores from the two normalised strings' word sets: equal → 1; one's words all
/// inside the other's → 0.9; otherwise the Jaccard overlap. Artist is the best over
/// every pair of credited names. Duration, when both are known: within 5 s → 1,
/// within 15 s → 0.8, else 0 (a reject); unknown on either side → 0.9.
///
/// Accepted when title ≥ 0.75, artist ≥ 0.7 and duration did not reject; **with no
/// artist on either side, only an exact title is trusted**. An ISRC that matches on
/// both sides is decisive on its own.
pub struct TrackMatcher;

impl TrackMatcher {
    /// The least title score a match needs.
    pub const TITLE_THRESHOLD: f64 = 0.75;
    /// The least artist score a match needs, when both sides name an artist.
    pub const ARTIST_THRESHOLD: f64 = 0.7;
    /// Lengths this close score 1 on duration.
    pub const CLOSE_DURATION_MS: i64 = 5_000;
    /// Lengths this close score 0.8; further apart is a reject.
    pub const LOOSE_DURATION_MS: i64 = 15_000;

    /// One list across every codebase.
    pub const NOISE_WORDS: [&'static str; 28] = [
        "acoustic",
        "anniversary",
        "bonus",
        "clean",
        "deluxe",
        "demo",
        "edit",
        "edition",
        "explicit",
        "extended",
        "feat",
        "featuring",
        "from",
        "ft",
        "instrumental",
        "live",
        "mix",
        "mono",
        "original",
        "radio",
        "reissue",
        "remaster",
        "remastered",
        "remix",
        "single",
        "stereo",
        "version",
        "with",
    ];

    /// `text` normalised as described above: lower-case, no diacritics, no noise
    /// groups or suffix, letters, digits and single spaces only.
    pub fn normalize(text: &str) -> String {
        let folded: String = text
            .to_lowercase()
            .nfd()
            .filter(|c| !is_combining_mark(*c))
            .collect();
        let mut s = folded.replace('&', " and ");
        s = strip_noisy_groups(&s, '(', ')');
        s = strip_noisy_groups(&s, '[', ']');
        s = strip_noisy_suffix(&s);
        let kept: String = s
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { ' ' })
            .collect();
        kept.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// The title without its noise groups and suffix, spelling otherwise untouched.
    pub fn strip_noise(title: &str) -> String {
        let mut s = strip_noisy_groups(title, '(', ')');
        s = strip_noisy_groups(&s, '[', ']');
        s = strip_noisy_suffix(&s);
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// How alike two normalised strings are, 0…1: equal is 1 (two empty
    /// strings are 0), one's words all in the other's 0.9, else the share of
    /// words they have in common.
    pub fn similarity(a: &str, b: &str) -> f64 {
        if a == b {
            return if a.is_empty() { 0.0 } else { 1.0 };
        }
        let words_a: std::collections::HashSet<&str> = a.split_whitespace().collect();
        let words_b: std::collections::HashSet<&str> = b.split_whitespace().collect();
        if words_a.is_empty() || words_b.is_empty() {
            return 0.0;
        }
        if words_a.is_subset(&words_b) || words_b.is_subset(&words_a) {
            return 0.9;
        }
        let shared = words_a.intersection(&words_b).count();
        let union = words_a.union(&words_b).count();
        shared as f64 / union as f64
    }

    /// How `candidate` compares with `track`, every score worked out.
    pub fn verdict(track: &ImportedTrack, candidate: &LibraryTrack) -> Verdict {
        let title = Self::similarity(
            &Self::normalize(&track.title),
            &Self::normalize(&candidate.title),
        );
        let no_artist = candidate.artists.is_empty() || track.artists.is_empty();
        let artist = if no_artist {
            0.5
        } else {
            let theirs: Vec<String> = candidate
                .artists
                .iter()
                .map(|a| Self::normalize(a))
                .collect();
            track
                .artists
                .iter()
                .map(|a| Self::normalize(a))
                .flat_map(|ours| {
                    theirs
                        .iter()
                        .map(|other| Self::similarity(&ours, other))
                        .collect::<Vec<_>>()
                })
                .fold(0.0, f64::max)
        };
        let duration = match (track.duration_ms, candidate.duration_ms) {
            (Some(a), Some(b)) => {
                let delta = (a - b).abs();
                if delta <= Self::CLOSE_DURATION_MS {
                    1.0
                } else if delta <= Self::LOOSE_DURATION_MS {
                    0.8
                } else {
                    0.0
                }
            }
            _ => 0.9,
        };
        let overall = 0.5 * title + 0.35 * artist + 0.15 * duration;
        if same_isrc(track.isrc.as_deref(), candidate.isrc.as_deref()) {
            return Verdict {
                title,
                artist,
                duration,
                is_accepted: true,
                overall: 2.0 + overall,
            };
        }
        let artist_accepted = if no_artist {
            title == 1.0
        } else {
            artist >= Self::ARTIST_THRESHOLD
        };
        Verdict {
            title,
            artist,
            duration,
            is_accepted: title >= Self::TITLE_THRESHOLD && artist_accepted && duration > 0.0,
            overall,
        }
    }

    /// The best accepted candidate's index; the first of equals.
    pub fn best(track: &ImportedTrack, candidates: &[LibraryTrack]) -> Option<usize> {
        let mut best = None;
        let mut best_score = -1.0;
        for (index, candidate) in candidates.iter().enumerate() {
            let verdict = Self::verdict(track, candidate);
            if verdict.is_accepted && verdict.overall > best_score {
                best_score = verdict.overall;
                best = Some(index);
            }
        }
        best
    }

    /// Phrases to search the library with, most specific first: the title as given
    /// (the server matches names as typed, so "don t stop me now" finds nothing);
    /// without its noise; normalised; and the longest word, when it has four letters
    /// or more.
    pub fn search_terms(title: &str) -> Vec<String> {
        let mut terms: Vec<String> = Vec::new();
        let mut add = |term: &str| {
            let trimmed = term.trim();
            if !trimmed.is_empty() && !terms.iter().any(|t| t == trimmed) {
                terms.push(trimmed.to_owned());
            }
        };
        add(title);
        add(&Self::strip_noise(title));
        let normalized = Self::normalize(title);
        add(&normalized);
        let mut longest = "";
        for word in normalized.split_whitespace() {
            if word.chars().count() > longest.chars().count() {
                longest = word;
            }
        }
        if longest.chars().count() >= 4 {
            add(longest);
        }
        terms
    }
}

fn same_isrc(a: Option<&str>, b: Option<&str>) -> bool {
    match (a.map(str::trim), b.map(str::trim)) {
        (Some(a), Some(b)) => !a.is_empty() && a.eq_ignore_ascii_case(b),
        _ => false,
    }
}

fn contains_noise(text: &str) -> bool {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .any(|word| TrackMatcher::NOISE_WORDS.contains(&word))
}

fn strip_noisy_groups(text: &str, open: char, close: char) -> String {
    let mut result = text.to_owned();
    loop {
        let Some(start) = result.find(open) else {
            return result;
        };
        let Some(end) = result[start..].find(close).map(|offset| start + offset) else {
            return result;
        };
        let inside = &result[start + open.len_utf8()..end];
        let after = &result[end + close.len_utf8()..];
        result = if contains_noise(inside) {
            format!("{}{}", &result[..start], after)
        } else {
            // A group worth keeping stays; only its brackets go.
            format!("{} {} {}", &result[..start], inside, after)
        };
    }
}

fn strip_noisy_suffix(text: &str) -> String {
    match text.find(" - ") {
        Some(at) if contains_noise(&text[at + 3..]) => text[..at].to_owned(),
        _ => text.to_owned(),
    }
}

// MARK: - Matching a list

/// The library's song search, as the importer needs it. The app implements it over
/// its Melete client (songs whose title matches `term`, at most `limit`).
pub trait LibrarySearch: Send + Sync {
    /// Library tracks whose names match `term`, at most `limit`. `Err` carries
    /// a message; the importer treats it as no results for that term.
    fn search_songs(
        &self,
        term: &str,
        limit: usize,
    ) -> impl Future<Output = Result<Vec<LibraryTrack>, String>> + Send;
}

/// The sidecar's answer for one imported track.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarMatch {
    /// The library track chosen.
    pub jellyfin_id: String,
    /// How sure the sidecar is of the match: its ranking score.
    pub confidence: f64,
    /// As [`Verdict::title`].
    #[serde(default)]
    pub title_score: f64,
    /// As [`Verdict::artist`].
    #[serde(default)]
    pub artist_score: f64,
    /// As [`Verdict::duration`].
    #[serde(default)]
    pub duration_score: f64,
    /// The chosen track's title, as tagged.
    #[serde(default)]
    pub title: Option<String>,
    /// The chosen track's artists.
    #[serde(default)]
    pub artists: Option<Vec<String>>,
    /// The chosen track's album.
    #[serde(default)]
    pub album: Option<String>,
    /// The chosen track's length in milliseconds.
    #[serde(default)]
    pub duration_ms: Option<i64>,
}

/// The sidecar refuses more than this many rows in one call.
pub const MATCH_ROWS_PER_REQUEST: usize = 5000;

impl SidecarClient {
    /// `POST /aoide/match`: one answer per row, in order, `None` where the library has
    /// nothing. Split into calls of [`MATCH_ROWS_PER_REQUEST`].
    ///
    /// # Errors
    ///
    /// The [request errors](SidecarClient#errors); [`SidecarError::Decode`] when an
    /// answer doesn't have one result per row it was sent.
    pub async fn match_tracks(
        &self,
        tracks: &[ImportedTrack],
    ) -> Result<Vec<Option<SidecarMatch>>, SidecarError> {
        #[derive(Deserialize)]
        struct Response {
            results: Vec<Option<SidecarMatch>>,
        }
        let mut results = Vec::with_capacity(tracks.len());
        for chunk in tracks.chunks(MATCH_ROWS_PER_REQUEST) {
            let page: Response = self
                .send_json(reqwest::Method::POST, "aoide/match", &[], &chunk)
                .await?;
            if page.results.len() != chunk.len() {
                return Err(SidecarError::Decode {
                    path: "aoide/match".to_owned(),
                    detail: format!("{} results for {} rows", page.results.len(), chunk.len()),
                });
            }
            results.extend(page.results);
        }
        Ok(results)
    }
}

/// Which path answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchedBy {
    /// `POST /aoide/match`.
    Sidecar,
    /// This device's rules over the library's search.
    Library,
}

/// The library track chosen for an imported one.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchedTrack {
    /// The library track's id: what goes in the saved playlist.
    pub jellyfin_id: String,
    /// The chosen track's own tags, so the screen can show what was picked.
    pub title: Option<String>,
    /// The chosen track's artists.
    pub artists: Vec<String>,
    /// The chosen track's album.
    pub album: Option<String>,
    /// The chosen track's length in milliseconds.
    pub duration_ms: Option<i64>,
    /// The ranking score: the sidecar's confidence, or [`Verdict::overall`].
    pub score: f64,
}

/// One imported track and what, if anything, the library has for it.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportMatch {
    /// The track as imported.
    pub track: ImportedTrack,
    /// The library's track for it, or `None` when the library has none.
    pub found: Option<MatchedTrack>,
}

/// What [`match_playlist`] decided, in the playlist's order.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportResult {
    /// One per imported track, in the playlist's order.
    pub matches: Vec<ImportMatch>,
    /// Which path did the matching.
    pub by: MatchedBy,
}

impl ImportResult {
    /// The track ids to save, in order.
    pub fn found_ids(&self) -> Vec<String> {
        self.matches
            .iter()
            .filter_map(|m| m.found.as_ref().map(|found| found.jellyfin_id.clone()))
            .collect()
    }

    /// The tracks the library does not have.
    pub fn missing(&self) -> Vec<&ImportedTrack> {
        self.matches
            .iter()
            .filter(|m| m.found.is_none())
            .map(|m| &m.track)
            .collect()
    }
}

/// How many results one library search asks for.
pub const SEARCH_LIMIT: usize = 40;
/// A search term is cut to this many characters.
pub const SEARCH_TERM_CHARS: usize = 60;

/// Matches `tracks` against the library: `POST /aoide/match` when there is a sidecar
/// and it answers, otherwise [`TrackMatcher`] over `library`'s search results — the
/// two follow the same rules, so the result is the same either way, only slower.
/// `progress(done, total)` is called as the local path goes. A search that fails is a
/// track reported missing, which is the honest answer from here.
pub async fn match_playlist<S: LibrarySearch>(
    sidecar: Option<&SidecarClient>,
    library: &S,
    tracks: &[ImportedTrack],
    mut progress: impl FnMut(usize, usize) + Send,
) -> ImportResult {
    if let Some(sidecar) = sidecar
        && let Ok(answers) = sidecar.match_tracks(tracks).await
    {
        let matches = tracks
            .iter()
            .zip(answers)
            .map(|(track, answer)| ImportMatch {
                track: track.clone(),
                found: answer.map(|answer| MatchedTrack {
                    jellyfin_id: answer.jellyfin_id,
                    title: answer.title,
                    artists: answer.artists.unwrap_or_default(),
                    album: answer.album,
                    duration_ms: answer.duration_ms,
                    score: answer.confidence,
                }),
            })
            .collect();
        progress(tracks.len(), tracks.len());
        return ImportResult {
            matches,
            by: MatchedBy::Sidecar,
        };
    }

    let mut matches = Vec::with_capacity(tracks.len());
    for (done, track) in tracks.iter().enumerate() {
        matches.push(ImportMatch {
            track: track.clone(),
            found: look_up(library, track).await,
        });
        progress(done + 1, tracks.len());
    }
    ImportResult {
        matches,
        by: MatchedBy::Library,
    }
}

async fn look_up<S: LibrarySearch>(library: &S, track: &ImportedTrack) -> Option<MatchedTrack> {
    for term in TrackMatcher::search_terms(&track.title) {
        let term: String = term.chars().take(SEARCH_TERM_CHARS).collect();
        let Ok(candidates) = library.search_songs(&term, SEARCH_LIMIT).await else {
            continue;
        };
        if candidates.is_empty() {
            continue;
        }
        if let Some(index) = TrackMatcher::best(track, &candidates) {
            let chosen = &candidates[index];
            return Some(MatchedTrack {
                jellyfin_id: chosen.jellyfin_id.clone(),
                title: Some(chosen.title.clone()),
                artists: chosen.artists.clone(),
                album: chosen.album.clone(),
                duration_ms: chosen.duration_ms,
                score: TrackMatcher::verdict(track, chosen).overall,
            });
        }
    }
    None
}

// MARK: - Spotify

/// Why a playlist could not be read in.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ImportError {
    /// No 22-character playlist id in what was pasted.
    #[error("that doesn't look like a Spotify playlist link")]
    NotASpotifyLink,
    /// The embed page answered with a non-2xx status: a private or deleted
    /// playlist, usually.
    #[error("Spotify didn't share that playlist (HTTP {status}); it may be private")]
    NotShared {
        /// The HTTP status Spotify answered with.
        status: u16,
    },
    /// The embed page changed shape. Said loudly, never read as an empty playlist.
    #[error(
        "Spotify has changed its page and the playlist couldn't be read ({0}); a CSV export still works"
    )]
    SpotifyChanged(String),
    /// No answer from Spotify, or the page didn't finish arriving.
    #[error("couldn't reach Spotify: {0}")]
    Network(String),
    /// The CSV has no rows with anything in them, not even a header.
    #[error("that file is empty")]
    EmptyCsv,
    /// The CSV's header has none of the names a title column goes by.
    #[error("that file has no title column; Exportify's CSV is the expected shape")]
    NoTitleColumn,
}

/// The embed page never lists more than this many tracks, whatever the playlist holds.
pub const SPOTIFY_EMBED_TRACK_LIMIT: usize = 100;

/// The 22-character id out of whatever was pasted: a share link, a
/// `spotify:playlist:` URI or an embed link.
pub fn spotify_playlist_id(text: &str) -> Option<String> {
    const PREFIXES: [&str; 3] = [
        "spotify:playlist:",
        "open.spotify.com/playlist/",
        "open.spotify.com/embed/playlist/",
    ];
    let text = text.trim();
    for (at, _) in text.char_indices() {
        let rest = &text[at..];
        for prefix in PREFIXES {
            if let Some(after) = rest.strip_prefix(prefix) {
                let id: String = after.chars().take(22).collect();
                if id.len() == 22 && id.chars().all(|c| c.is_ascii_alphanumeric()) {
                    return Some(id);
                }
            }
        }
    }
    None
}

/// The page that carries a playlist's tracks without an account or a key.
pub fn spotify_embed_url(playlist_id: &str) -> String {
    format!("https://open.spotify.com/embed/playlist/{playlist_id}")
}

/// The track list out of Spotify's embed page, which ships its state in a
/// `__NEXT_DATA__` script. Undocumented, so a shape change is an error naming what
/// was missing rather than an empty playlist.
///
/// # Errors
///
/// [`ImportError::SpotifyChanged`], naming what was missing, when the page has no
/// `__NEXT_DATA__` state, the state isn't JSON, or it has no playlist entity or
/// track list.
pub fn parse_spotify_embed(html: &str) -> Result<ImportedPlaylist, ImportError> {
    const OPEN: &str = r#"<script id="__NEXT_DATA__" type="application/json">"#;
    let no_state = || ImportError::SpotifyChanged("no embedded state".to_owned());
    let start = html.find(OPEN).ok_or_else(no_state)? + OPEN.len();
    let end = html[start..].find("</script>").ok_or_else(no_state)? + start;
    let root: Value = serde_json::from_str(&html[start..end])
        .map_err(|_| ImportError::SpotifyChanged("state is not JSON".to_owned()))?;
    let entity = ["props", "pageProps", "state", "data", "entity"]
        .iter()
        .try_fold(&root, |value, key| value.get(*key))
        .filter(|entity| entity.is_object())
        .ok_or_else(|| {
            ImportError::SpotifyChanged("no entity at props.pageProps.state.data.entity".to_owned())
        })?;
    let list = entity
        .get("trackList")
        .and_then(Value::as_array)
        .ok_or_else(|| ImportError::SpotifyChanged("entity has no trackList".to_owned()))?;
    let tracks = list
        .iter()
        .filter_map(|row| {
            let title = row
                .get("title")?
                .as_str()
                .filter(|title| !title.is_empty())?;
            // One comma-joined string: a name with a comma in it splits wrong, and the
            // page has nothing to tell those apart.
            let artists = row
                .get("subtitle")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect();
            Some(ImportedTrack {
                title: title.to_owned(),
                artists,
                album: None,
                duration_ms: row
                    .get("duration")
                    .and_then(Value::as_f64)
                    .map(|ms| ms as i64),
                isrc: None,
            })
        })
        .collect();
    let name = entity
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.is_empty())
        .unwrap_or("Spotify playlist")
        .to_owned();
    Ok(ImportedPlaylist {
        name,
        tracks,
        is_truncated: list.len() >= SPOTIFY_EMBED_TRACK_LIMIT,
    })
}

/// Reads a pasted Spotify playlist link: fetches its embed page and parses it. Sends
/// nothing but the request for the public page — no account, no cookie, no token.
///
/// # Errors
///
/// [`ImportError::NotASpotifyLink`] before any request when `link` holds no
/// playlist id; [`ImportError::Network`] when Spotify can't be reached;
/// [`ImportError::NotShared`] for a non-2xx answer; then as for
/// [`parse_spotify_embed`].
pub async fn fetch_spotify_playlist(link: &str) -> Result<ImportedPlaylist, ImportError> {
    fetch_spotify_playlist_from("https://open.spotify.com", link).await
}

/// [`fetch_spotify_playlist`] against another origin serving the same page — a test
/// server.
///
/// # Errors
///
/// As for [`fetch_spotify_playlist`].
pub async fn fetch_spotify_playlist_from(
    origin: &str,
    link: &str,
) -> Result<ImportedPlaylist, ImportError> {
    let id = spotify_playlist_id(link).ok_or(ImportError::NotASpotifyLink)?;
    let url = format!("{}/embed/playlist/{id}", origin.trim_end_matches('/'));
    // ring, never aws-lc: see the manifest.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30))
        // The page is served to browsers and says nothing to anything else.
        .user_agent("Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0")
        .build()
        .map_err(|error| ImportError::Network(error.to_string()))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| ImportError::Network(error.to_string()))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(ImportError::NotShared { status });
    }
    let html = response
        .text()
        .await
        .map_err(|error| ImportError::Network(error.to_string()))?;
    parse_spotify_embed(&html)
}

// MARK: - CSV exports

const TITLE_HEADERS: [&str; 5] = ["track name", "title", "track", "name", "song"];
const ARTIST_HEADERS: [&str; 5] = [
    "artist name(s)",
    "artist name",
    "artist",
    "artists",
    "artist(s)",
];
const ALBUM_HEADERS: [&str; 2] = ["album name", "album"];
const DURATION_HEADERS: [&str; 4] = [
    "track duration (ms)",
    "duration (ms)",
    "duration_ms",
    "duration",
];
const ISRC_HEADERS: [&str; 1] = ["isrc"];

/// A playlist out of a CSV export: Exportify's headers first, then the other tools'
/// names for the same columns. Durations in milliseconds, or `m:ss`.
///
/// # Errors
///
/// [`ImportError::EmptyCsv`] when there is no non-blank row at all;
/// [`ImportError::NoTitleColumn`] when the header has none of the title column's
/// names. Rows without a title are skipped, not errors.
pub fn parse_playlist_csv(text: &str, name: &str) -> Result<ImportedPlaylist, ImportError> {
    // A byte-order mark would glue itself to the first header and hide it.
    let clean = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let rows: Vec<Vec<String>> = csv_rows(clean)
        .into_iter()
        .filter(|row| row.iter().any(|cell| !cell.is_empty()))
        .collect();
    let header = rows.first().ok_or(ImportError::EmptyCsv)?;
    let columns: Vec<String> = header
        .iter()
        .map(|cell| cell.trim().to_lowercase())
        .collect();
    let index_of = |names: &[&str]| {
        names
            .iter()
            .find_map(|name| columns.iter().position(|column| column == name))
    };
    let title = index_of(&TITLE_HEADERS).ok_or(ImportError::NoTitleColumn)?;
    let artist = index_of(&ARTIST_HEADERS);
    let album = index_of(&ALBUM_HEADERS);
    let duration = index_of(&DURATION_HEADERS);
    let isrc = index_of(&ISRC_HEADERS);

    let tracks = rows[1..]
        .iter()
        .filter_map(|row| {
            let cell = |column: Option<usize>| {
                column
                    .and_then(|column| row.get(column))
                    .map(|value| value.trim())
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
            };
            let title = cell(Some(title))?;
            let artists = cell(artist)
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect();
            Some(ImportedTrack {
                title,
                artists,
                album: cell(album),
                duration_ms: cell(duration).and_then(|value| parse_duration(&value)),
                isrc: cell(isrc),
            })
        })
        .collect();
    Ok(ImportedPlaylist {
        name: name.to_owned(),
        tracks,
        is_truncated: false,
    })
}

fn parse_duration(value: &str) -> Option<i64> {
    if value.chars().all(|c| c.is_ascii_digit()) {
        return value.parse().ok();
    }
    let parts: Vec<&str> = value.split(':').collect();
    let [minutes, seconds] = parts.as_slice() else {
        return None;
    };
    let whole = |part: &str| -> Option<i64> {
        (!part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
            .then(|| part.parse().ok())
            .flatten()
    };
    Some((whole(minutes)? * 60 + whole(seconds)?) * 1000)
}

/// Just enough of RFC 4180: quoted fields, doubled quotes, newlines inside quotes.
fn csv_rows(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}
