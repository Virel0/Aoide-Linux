use std::time::Duration;

use serde::Deserialize;

use crate::client::MeleteClient;
use crate::{Error, Result};

/// A track's lyrics, as lines.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Lyrics {
    /// The lines in the order they're sung. [`line_at`](Self::line_at) expects
    /// the timed ones in time order.
    pub lines: Vec<LyricLine>,
}

/// One line of lyrics, with its start time and word times when the source has
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricLine {
    /// The line as shown. May be empty: a timed empty line marks a break.
    pub text: String,
    /// `None` when the source had no timings: the line can be shown, not followed.
    pub start: Option<Duration>,
    /// Empty unless the source has per-word timings (an online source's enhanced
    /// LRC; Melete's lyrics have none).
    pub words: Vec<LyricWord>,
}

/// One timed word in a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricWord {
    /// The word, without the spaces around it.
    pub text: String,
    /// When the word is sung, from the start of the track.
    pub start: Duration,
    /// When the word's highlight should finish: the next word's start, or the cue's
    /// own end for the last word, capped at [`MAX_SWEEP`](Self::MAX_SWEEP) after `start`.
    pub end: Duration,
}

impl LyricWord {
    /// The longest a word's highlight sweeps, 1.2 s. Every source of word timings
    /// caps `end` at `start` plus this.
    pub const MAX_SWEEP: Duration = Duration::from_millis(1_200);
}

impl Lyrics {
    /// Whether the lines can be followed along with the music (a `.lrc`, not a `.txt`).
    pub fn is_synced(&self) -> bool {
        self.lines.iter().any(|line| line.start.is_some())
    }

    /// True only for enhanced LRC. A plain `.lrc` has one time per line and simply
    /// doesn't contain word timings.
    pub fn has_word_timing(&self) -> bool {
        self.lines.iter().any(|line| !line.words.is_empty())
    }

    /// No lines at all.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The last line that has started by `time`, or `None` before the first one and
    /// for unsynced lyrics. A binary search, since it runs on every clock tick.
    pub fn line_at(&self, time: Duration) -> Option<usize> {
        let (mut low, mut high) = (0usize, self.lines.len());
        let mut found = None;
        while low < high {
            let mid = (low + high) / 2;
            match self.lines[mid].start {
                // An untimed line can't anchor the search; step past it.
                None => low = mid + 1,
                Some(start) if start <= time => {
                    found = Some(mid);
                    low = mid + 1;
                }
                Some(_) => high = mid,
            }
        }
        found
    }
}

impl LyricsAnswer {
    fn from_wire(wire: WireLyrics, ask: &LyricsAsk) -> Self {
        let translations = ask.translate.is_some().then(|| {
            wire.lines
                .iter()
                .map(|line| line.translation.clone().unwrap_or_default())
                .collect()
        });
        let pronunciations = ask
            .pronounce
            .then(|| wire.lines.iter().map(|l| l.pronunciation.clone()).collect());
        let lines = wire
            .lines
            .into_iter()
            .map(|line| LyricLine {
                start: line
                    .start_seconds
                    .filter(|s| s.is_finite() && *s >= 0.0)
                    .map(Duration::from_secs_f64),
                text: line.text,
                words: Vec::new(),
            })
            .collect();
        Self {
            lyrics: Lyrics { lines },
            source: wire.source,
            translations,
            pronunciations,
        }
    }
}

/// `MusicLyrics`: lines with their starts for synced lyrics (an `.lrc` beside
/// the track, its tags, or LRCLIB), without for plain ones. Melete has no word
/// timings. Asked for, each line also carries its translation and its
/// pronunciation.
#[derive(Deserialize)]
struct WireLyrics {
    source: Option<String>,
    #[serde(default)]
    lines: Vec<WireLine>,
}

#[derive(Deserialize)]
struct WireLine {
    #[serde(default)]
    text: String,
    start_seconds: Option<f64>,
    translation: Option<String>,
    pronunciation: Option<String>,
}

/// What to ask the server to add to a track's lyrics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LyricsAsk {
    /// A language to translate them into, as LibreTranslate names it (`en`,
    /// `pt-BR`), through the LibreTranslate the server has set up.
    pub translate: Option<String>,
    /// Each line in Latin letters too, when the server has pronunciation on.
    pub pronounce: bool,
}

/// A track's lyrics as the server answered, with what it added.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LyricsAnswer {
    /// The lines.
    pub lyrics: Lyrics,
    /// Where they came from: `file` (an `.lrc` beside the track), `tags` or
    /// `lrclib`.
    pub source: Option<String>,
    /// Each line's translation, line for line, when one was asked for.
    pub translations: Option<Vec<String>>,
    /// Each line in Latin letters, line for line, when asked for; `None` for a
    /// line with nothing to romanise.
    pub pronunciations: Option<Vec<Option<String>>>,
}

impl MeleteClient {
    /// A track's lyrics, or `None` when it has none. A 404 here is the ordinary
    /// "no lyrics" answer, not an error; every other failure still is one.
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors) other than a 404. A signed-out
    /// client sends no token, which the server answers with
    /// [`Error::Unauthorized`].
    pub async fn lyrics(&self, track_id: &str) -> Result<Option<Lyrics>> {
        let answer = self.lyrics_with(track_id, &LyricsAsk::default()).await?;
        Ok(answer.map(|a| a.lyrics))
    }

    /// A track's lyrics with what `ask` adds, or `None` when it has none.
    ///
    /// # Errors
    ///
    /// As [`lyrics`](Self::lyrics); and HTTP 409 as [`Error::Http`] when a
    /// translation was asked for and the server has none set up, or
    /// pronunciation and the server has it off; 502 when LibreTranslate didn't
    /// translate.
    pub async fn lyrics_with(
        &self,
        track_id: &str,
        ask: &LyricsAsk,
    ) -> Result<Option<LyricsAnswer>> {
        let mut query = Vec::new();
        if let Some(language) = &ask.translate {
            query.push(("translate", language.clone()));
        }
        if ask.pronounce {
            query.push(("pronounce", "true".to_owned()));
        }
        match self
            .get::<WireLyrics>(
                &["api", "v1", "music", "tracks", track_id, "lyrics"],
                &query,
            )
            .await
        {
            Ok(wire) => {
                let answer = LyricsAnswer::from_wire(wire, ask);
                Ok((!answer.lyrics.is_empty()).then_some(answer))
            }
            Err(Error::NotFound { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(json: &str) -> Lyrics {
        LyricsAnswer::from_wire(serde_json::from_str(json).unwrap(), &LyricsAsk::default()).lyrics
    }

    #[test]
    fn synced_lyrics_from_the_server_have_line_starts() {
        let lyrics = decode(include_str!("../tests/fixtures/lyrics.json"));
        assert!(lyrics.is_synced());
        assert!(!lyrics.has_word_timing());
        assert_eq!(lyrics.lines[0].text, "First line");
        assert_eq!(lyrics.lines[0].start, Some(Duration::from_secs(1)));
        assert_eq!(lyrics.lines[1].start, Some(Duration::from_secs(5)));
    }

    #[test]
    fn plain_lyrics_are_unsynced_lines_that_can_still_be_shown() {
        let lyrics = decode(
            r#"{"track_id":1,"source":"tags","synced":false,"lines":[
                {"start_seconds":null,"text":"one"},{"start_seconds":null,"text":"two"}]}"#,
        );
        assert!(!lyrics.is_synced());
        assert_eq!(lyrics.lines.len(), 2);
    }

    #[test]
    fn translations_and_pronunciations_come_line_for_line_when_asked() {
        let wire = r#"{"track_id":1,"source":"lrclib","synced":false,"language":"en","lines":[
            {"start_seconds":null,"text":"夜空","translation":"Night sky","pronunciation":"yozora"},
            {"start_seconds":null,"text":"","translation":"","pronunciation":null}]}"#;
        let ask = LyricsAsk {
            translate: Some("en".into()),
            pronounce: true,
        };
        let answer = LyricsAnswer::from_wire(serde_json::from_str(wire).unwrap(), &ask);
        assert_eq!(answer.source.as_deref(), Some("lrclib"));
        assert_eq!(answer.lyrics.lines[0].text, "夜空");
        assert_eq!(
            answer.translations,
            Some(vec!["Night sky".to_owned(), String::new()])
        );
        assert_eq!(
            answer.pronunciations,
            Some(vec![Some("yozora".to_owned()), None])
        );
        let plain =
            LyricsAnswer::from_wire(serde_json::from_str(wire).unwrap(), &LyricsAsk::default());
        assert_eq!((plain.translations, plain.pronunciations), (None, None));
    }

    #[test]
    fn an_empty_document_is_empty() {
        assert!(decode(r#"{"lines":[]}"#).is_empty());
    }

    #[test]
    fn a_bad_start_is_untimed() {
        let lyrics = decode(r#"{"lines":[{"start_seconds":-2,"text":"a"}]}"#);
        assert_eq!(lyrics.lines[0].start, None);
    }

    fn synced() -> Lyrics {
        decode(
            r#"{"lines":[
                {"text":"a","start_seconds":1},
                {"text":"b","start_seconds":5},
                {"text":"c","start_seconds":9}
            ]}"#,
        )
    }

    #[test]
    fn line_lookup_follows_the_clock() {
        let lyrics = synced();
        assert_eq!(lyrics.line_at(Duration::from_millis(500)), None);
        assert_eq!(lyrics.line_at(Duration::from_secs(5)), Some(1));
        assert_eq!(lyrics.line_at(Duration::from_secs(7)), Some(1));
        assert_eq!(lyrics.line_at(Duration::from_secs(600)), Some(2));
    }

    #[test]
    fn unsynced_or_empty_lyrics_have_no_current_line() {
        let unsynced = decode(r#"{"lines":[{"text":"a"},{"text":"b"}]}"#);
        assert_eq!(unsynced.line_at(Duration::from_secs(30)), None);
        assert_eq!(Lyrics::default().line_at(Duration::from_secs(1)), None);
    }
}
