use std::time::Duration;

use serde::Deserialize;

use crate::client::JellyfinClient;
use crate::{Error, Result, ticks};

/// The longest one word's highlight may take to sweep. Without a cap, the word
/// before an instrumental break crawls across the screen for the whole break.
const MAX_SWEEP: Duration = LyricWord::MAX_SWEEP;

/// A track's lyrics, as lines.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Lyrics {
    pub lines: Vec<LyricLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricLine {
    pub text: String,
    /// `None` when the source had no timings: the line can be shown, not followed.
    pub start: Option<Duration>,
    /// Empty unless the source was enhanced LRC with per-word timings.
    pub words: Vec<LyricWord>,
}

/// One timed word in a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricWord {
    pub text: String,
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

    fn from_document(document: LyricsDocument) -> Self {
        let lines = document
            .lyrics
            .into_iter()
            .map(|line| LyricLine {
                words: words(&line),
                start: line.start.map(ticks::to_duration),
                text: line.text,
            })
            .collect();
        Self { lines }
    }
}

// The wire format of `GET /Audio/{id}/Lyrics`. One endpoint, three shapes:
// enhanced LRC (`Start` and populated `Cues`), plain LRC (`Start`, empty `Cues`),
// and plain text (neither). `Metadata` is not read: Jellyfin never fills in its
// `IsSynced` or `Offset`.

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LyricsDocument {
    #[serde(default)]
    lyrics: Vec<WireLine>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WireLine {
    #[serde(default)]
    text: String,
    start: Option<i64>,
    #[serde(default)]
    cues: Option<Vec<WireCue>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WireCue {
    #[serde(default)]
    position: i64,
    end_position: Option<i64>,
    #[serde(default)]
    start: i64,
    end: Option<i64>,
}

fn words(line: &WireLine) -> Vec<LyricWord> {
    let Some(cues) = line.cues.as_deref().filter(|cues| !cues.is_empty()) else {
        return Vec::new();
    };
    // Cue positions are .NET UTF-16 offsets, not byte or char offsets: slicing by
    // anything else goes wrong at the first emoji or accented letter.
    let units: Vec<u16> = line.text.encode_utf16().collect();
    let len = units.len() as i64;

    let mut words = Vec::new();
    for (i, cue) in cues.iter().enumerate() {
        let lo = cue.position.clamp(0, len);
        let hi = cue.end_position.unwrap_or(len).clamp(lo, len);
        let slice = String::from_utf16_lossy(&units[lo as usize..hi as usize]);
        // Contiguous cues each carry their trailing space.
        let text = slice.trim();
        if text.is_empty() {
            continue;
        }
        let start = ticks::to_duration(cue.start);
        // The next cue's start is a better end than this cue's own, often absent.
        let natural_end = match cues.get(i + 1) {
            Some(next) => ticks::to_duration(next.start),
            None => cue.end.map_or(start + MAX_SWEEP, ticks::to_duration),
        };
        words.push(LyricWord {
            text: text.to_owned(),
            start,
            end: natural_end.min(start + MAX_SWEEP),
        });
    }
    words
}

impl JellyfinClient {
    /// A track's lyrics, or `None` when it has none. A 404 here is the ordinary
    /// "no lyrics" answer, not an error; every other failure still is one.
    pub async fn lyrics(&self, item_id: &str) -> Result<Option<Lyrics>> {
        match self
            .get::<LyricsDocument>(&["Audio", item_id, "Lyrics"], &[])
            .await
        {
            Ok(document) => {
                let lyrics = Lyrics::from_document(document);
                Ok((!lyrics.is_empty()).then_some(lyrics))
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
        Lyrics::from_document(serde_json::from_str(json).unwrap())
    }

    fn texts(line: &LyricLine) -> Vec<&str> {
        line.words.iter().map(|w| w.text.as_str()).collect()
    }

    #[test]
    fn plain_lrc_is_synced_lines_without_word_timing() {
        let lyrics = decode(
            r#"{"Lyrics":[
                {"Text":"First line","Start":10000000,"Cues":[]},
                {"Text":"Second line","Start":25000000,"Cues":[]}
            ]}"#,
        );
        assert_eq!(lyrics.lines.len(), 2);
        assert!(lyrics.is_synced());
        assert!(!lyrics.has_word_timing());
        assert_eq!(lyrics.lines[0].start, Some(Duration::from_secs(1)));
        assert_eq!(lyrics.lines[1].start, Some(Duration::from_millis(2_500)));
    }

    #[test]
    fn plain_text_is_unsynced_lines_that_can_still_be_shown() {
        let lyrics = decode(r#"{"Lyrics":[{"Text":"No timings here"},{"Text":"Second"}]}"#);
        assert_eq!(lyrics.lines.len(), 2);
        assert!(!lyrics.is_synced());
        assert_eq!(lyrics.lines[0].start, None);
        assert_eq!(lyrics.lines[0].text, "No timings here");
    }

    #[test]
    fn enhanced_lrc_has_per_word_timings() {
        let lyrics = decode(include_str!("../tests/fixtures/lyrics_enhanced.json"));
        assert!(lyrics.has_word_timing());
        assert_eq!(texts(&lyrics.lines[0]), ["Hello", "big", "world"]);
        let words = &lyrics.lines[0].words;
        assert_eq!(words[0].start, Duration::from_secs(1));
        assert_eq!(words[0].end, Duration::from_millis(1_500));
        // The last word ends at its own End, 2 s.
        assert_eq!(words[2].end, Duration::from_secs(2));
        // A plain line in the same file has no words.
        assert!(lyrics.lines[1].words.is_empty());
    }

    #[test]
    fn an_empty_document_is_empty() {
        assert!(decode(r#"{"Lyrics":[]}"#).is_empty());
        assert!(decode("{}").is_empty());
    }

    #[test]
    fn a_missing_text_field_doesnt_fail_the_document() {
        let lyrics = decode(r#"{"Lyrics":[{"Start":10000000}]}"#);
        assert_eq!(lyrics.lines.len(), 1);
        assert!(lyrics.lines[0].text.is_empty());
    }

    #[test]
    fn cues_slice_by_utf16_offset() {
        // The note is two UTF-16 units, four UTF-8 bytes, one char.
        let lyrics = decode(
            r#"{"Lyrics":[{"Text":"🎵 hey","Start":0,"Cues":[
                {"Position":0,"EndPosition":2,"Start":0},
                {"Position":3,"EndPosition":6,"Start":5000000}
            ]}]}"#,
        );
        assert_eq!(texts(&lyrics.lines[0]), ["🎵", "hey"]);
    }

    #[test]
    fn out_of_range_offsets_are_clamped() {
        let lyrics = decode(
            r#"{"Lyrics":[{"Text":"short","Start":0,"Cues":[
                {"Position":-5,"EndPosition":99,"Start":0}
            ]}]}"#,
        );
        assert_eq!(texts(&lyrics.lines[0]), ["short"]);
    }

    #[test]
    fn an_inverted_range_gives_no_word() {
        let lyrics = decode(
            r#"{"Lyrics":[{"Text":"abcdef","Start":0,"Cues":[
                {"Position":4,"EndPosition":1,"Start":0}
            ]}]}"#,
        );
        assert!(lyrics.lines[0].words.is_empty());
    }

    #[test]
    fn words_are_trimmed_and_blank_ones_dropped() {
        let lyrics = decode(
            r#"{"Lyrics":[{"Text":"one two","Start":0,"Cues":[
                {"Position":0,"EndPosition":4,"Start":0},
                {"Position":4,"EndPosition":7,"Start":5000000},
                {"Position":7,"EndPosition":7,"Start":9000000}
            ]}]}"#,
        );
        assert_eq!(texts(&lyrics.lines[0]), ["one", "two"]);
    }

    #[test]
    fn a_word_before_a_long_pause_has_its_sweep_capped() {
        let lyrics = decode(
            r#"{"Lyrics":[{"Text":"aa bb","Start":0,"Cues":[
                {"Position":0,"EndPosition":3,"Start":0},
                {"Position":3,"EndPosition":5,"Start":300000000}
            ]}]}"#,
        );
        assert_eq!(lyrics.lines[0].words[0].end, Duration::from_millis(1_200));
    }

    fn synced() -> Lyrics {
        decode(
            r#"{"Lyrics":[
                {"Text":"a","Start":10000000},
                {"Text":"b","Start":50000000},
                {"Text":"c","Start":90000000}
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
        let unsynced = decode(r#"{"Lyrics":[{"Text":"a"},{"Text":"b"}]}"#);
        assert_eq!(unsynced.line_at(Duration::from_secs(30)), None);
        assert_eq!(Lyrics::default().line_at(Duration::from_secs(1)), None);
    }
}
