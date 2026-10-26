//! Furigana and romaji for Japanese lyrics, through lindera with IPADIC.
//!
//! Only with the `furigana` feature. The dictionary is built into the binary
//! (about 48 MB), loads in a millisecond or two, and reads a line in about as
//! long.

use std::fmt;

use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;

use crate::romaji::{kana_to_romaji, to_hiragana};

/// A run of a line with, over its kanji, how they're read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ruby {
    /// The run as written in the line. The runs of a line, joined, give the
    /// line back.
    pub text: String,
    /// In hiragana. `None` for kana, punctuation and anything not Japanese.
    pub reading: Option<String>,
}

/// The analyser could not start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("couldn't load the Japanese dictionary: {0}")]
pub struct ReaderError(String);

/// Reads Japanese lines: the kanji's readings, and the line in romaji.
///
/// Make one and keep it; it's cheap to share (`Clone` shares the dictionary).
#[derive(Clone)]
pub struct JapaneseReader {
    segmenter: Segmenter,
}

impl fmt::Debug for JapaneseReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("JapaneseReader(ipadic)")
    }
}

/// One morpheme as IPADIC describes it.
struct Morpheme {
    surface: String,
    /// Part of speech, and its first subdivision: `助詞`, `格助詞`.
    pos: String,
    pos_detail: String,
    /// Katakana, as written: トウキョウ.
    reading: Option<String>,
    /// Katakana, as said: トーキョー, and ワ for the particle は.
    pronunciation: Option<String>,
}

impl JapaneseReader {
    /// Loads the built-in IPADIC dictionary.
    ///
    /// # Errors
    ///
    /// [`ReaderError`], with lindera's message, when the embedded dictionary
    /// doesn't load.
    pub fn new() -> Result<Self, ReaderError> {
        let dictionary =
            load_dictionary("embedded://ipadic").map_err(|e| ReaderError(e.to_string()))?;
        Ok(Self {
            segmenter: Segmenter::new(Mode::Normal, dictionary, None),
        })
    }

    fn morphemes(&self, line: &str) -> Vec<Morpheme> {
        let mut worker = self.segmenter.new_worker();
        let Ok(mut tokens) = worker.segment(line) else {
            return vec![Morpheme {
                surface: line.to_owned(),
                pos: String::new(),
                pos_detail: String::new(),
                reading: None,
                pronunciation: None,
            }];
        };
        tokens
            .iter_mut()
            .map(|token| {
                let surface = token.surface.to_string();
                let details: Vec<String> = token.details().iter().map(|d| d.to_string()).collect();
                let field = |i: usize| {
                    details
                        .get(i)
                        .filter(|value| !value.is_empty() && *value != "*")
                        .cloned()
                };
                Morpheme {
                    surface,
                    pos: field(0).unwrap_or_default(),
                    pos_detail: field(1).unwrap_or_default(),
                    reading: field(7),
                    pronunciation: field(8),
                }
            })
            .collect()
    }

    /// The line in runs, each kanji run with its reading: 夜空に輝く becomes
    /// 夜空 (よぞら), に, 輝 (かがや), く. The texts put together are the line.
    pub fn furigana(&self, line: &str) -> Vec<Ruby> {
        let mut runs: Vec<Ruby> = Vec::new();
        let mut covered = 0;
        for morpheme in self.morphemes(line) {
            // The analyser drops spaces; put back whatever it skipped.
            if let Some(at) = line[covered..].find(&morpheme.surface) {
                let skipped = &line[covered..covered + at];
                if !skipped.is_empty() {
                    push(&mut runs, skipped, None);
                }
                covered += at + morpheme.surface.len();
            }
            for ruby in align(&morpheme.surface, morpheme.reading.as_deref()) {
                push(&mut runs, &ruby.text, ruby.reading);
            }
        }
        if covered < line.len() {
            push(&mut runs, &line[covered..], None);
        }
        runs
    }

    /// The line in modified Hepburn romaji, a space between words:
    /// 君の名は。 becomes "kimi no na wa." Particles read as said (は "wa",
    /// へ "e", を "o"); long vowels take a macron (東京 "tōkyō"), except えい, kept
    /// as written ("sensei"); endings stay on their word ("ikimasu", "miagete").
    /// Words the dictionary doesn't know, Latin letters and numbers among them,
    /// are kept as they are.
    pub fn romaji(&self, line: &str) -> String {
        let mut out = String::new();
        for morpheme in self.morphemes(line) {
            let word = match spoken(&morpheme) {
                Some(kana) => kana_to_romaji(&kana),
                None => kana_to_romaji(&morpheme.surface),
            };
            if word.is_empty() {
                continue;
            }
            let attach = morpheme.pos == "記号" || attaches(&morpheme);
            if !out.is_empty() && !attach {
                out.push(' ');
            }
            out.push_str(&word);
        }
        out
    }
}

/// Adds a run, joining it to the previous one when neither has a reading.
fn push(runs: &mut Vec<Ruby>, text: &str, reading: Option<String>) {
    if reading.is_none()
        && let Some(last) = runs.last_mut()
        && last.reading.is_none()
    {
        last.text.push_str(text);
        return;
    }
    runs.push(Ruby {
        text: text.to_owned(),
        reading,
    });
}

/// Endings and helpers that belong to the word before them in romaji: auxiliary
/// verbs (ます, た), conjunctive particles (て, ば), dependent verbs (いる in
/// ている) and suffixes (さん).
fn attaches(morpheme: &Morpheme) -> bool {
    matches!(
        (morpheme.pos.as_str(), morpheme.pos_detail.as_str()),
        ("助動詞", _)
            | ("助詞", "接続助詞")
            | ("動詞", "非自立")
            | ("動詞", "接尾")
            | ("名詞", "接尾")
    )
}

/// How a morpheme is said, in katakana. The pronunciation field marks long
/// vowels with ー and spells particles as said; where it lengthens an e with ー
/// over an イ in the reading (セー for せい), the イ is kept, as Hepburn writes
/// "ei". `None` for what the dictionary doesn't know.
fn spoken(morpheme: &Morpheme) -> Option<String> {
    if morpheme.pos == "助詞" {
        match morpheme.surface.as_str() {
            "は" => return Some("ワ".to_owned()),
            "へ" => return Some("エ".to_owned()),
            "を" => return Some("オ".to_owned()),
            _ => {}
        }
    }
    let reading = morpheme.reading.as_deref()?;
    let Some(pronunciation) = morpheme.pronunciation.as_deref() else {
        return Some(reading.to_owned());
    };
    let said: Vec<char> = pronunciation.chars().collect();
    let written: Vec<char> = reading.chars().collect();
    if said.len() != written.len() {
        return Some(pronunciation.to_owned());
    }
    Some(
        said.iter()
            .zip(&written)
            .enumerate()
            .map(|(i, (&s, &w))| {
                let after_e = i > 0
                    && matches!(
                        kana_to_romaji(&said[i - 1].to_string()).chars().last(),
                        Some('e')
                    );
                if s == 'ー' && w == 'イ' && after_e {
                    'イ'
                } else {
                    s
                }
            })
            .collect(),
    )
}

fn is_kanji(c: char) -> bool {
    matches!(c,
        '\u{4E00}'..='\u{9FFF}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{20000}'..='\u{2FA1F}'
        // 々 repeats a kanji; 〆 and ヶ stand for one.
        | '々' | '〆' | 'ヶ')
}

/// Splits one morpheme's surface into kanji and kana runs and gives each kanji
/// run its share of the reading, by matching the kana runs against the reading
/// (行き / イキ: 行 い, き). When the kana can't be matched, the whole morpheme
/// gets the whole reading.
fn align(surface: &str, reading: Option<&str>) -> Vec<Ruby> {
    let plain = || {
        vec![Ruby {
            text: surface.to_owned(),
            reading: None,
        }]
    };
    if !surface.chars().any(is_kanji) {
        return plain();
    }
    let Some(reading) = reading else {
        return plain();
    };
    let reading: Vec<char> = reading.chars().map(to_hiragana).collect();

    // Runs of kanji and of everything else, in order.
    let mut runs: Vec<(bool, String)> = Vec::new();
    for c in surface.chars() {
        let kanji = is_kanji(c);
        match runs.last_mut() {
            Some((k, text)) if *k == kanji => text.push(c),
            _ => runs.push((kanji, c.to_string())),
        }
    }

    let whole = || {
        vec![Ruby {
            text: surface.to_owned(),
            reading: Some(reading.iter().collect()),
        }]
    };
    let mut out = Vec::new();
    let mut at = 0;
    for (i, (kanji, text)) in runs.iter().enumerate() {
        if !kanji {
            let kana: Vec<char> = text.chars().map(to_hiragana).collect();
            if !reading[at..].starts_with(&kana) {
                return whole();
            }
            at += kana.len();
            out.push(Ruby {
                text: text.clone(),
                reading: None,
            });
            continue;
        }
        let end = match runs.get(i + 1) {
            None => reading.len(),
            Some((_, next)) => {
                let kana: Vec<char> = next.chars().map(to_hiragana).collect();
                // The kanji read as at least one kana, up to where the next kana run is.
                match (at + 1..=reading.len()).find(|&j| reading[j..].starts_with(&kana)) {
                    Some(j) => j,
                    None => return whole(),
                }
            }
        };
        if end <= at {
            return whole();
        }
        out.push(Ruby {
            text: text.clone(),
            reading: Some(reading[at..end].iter().collect()),
        });
        at = end;
    }
    if at != reading.len() {
        return whole();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reader() -> JapaneseReader {
        JapaneseReader::new().unwrap()
    }

    fn shown(runs: &[Ruby]) -> String {
        runs.iter()
            .map(|r| match &r.reading {
                Some(reading) => format!("{}({reading})", r.text),
                None => r.text.clone(),
            })
            .collect()
    }

    #[test]
    fn kanji_get_their_readings_and_okurigana_stays_outside() {
        let runs = reader().furigana("夜空に輝く星を見上げて");
        assert_eq!(
            shown(&runs),
            "夜空(よぞら)に輝(かがや)く星(ほし)を見上(みあ)げて"
        );
        let text: String = runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(text, "夜空に輝く星を見上げて");
    }

    #[test]
    fn kana_and_latin_have_no_reading_and_spaces_survive() {
        let runs = reader().furigana("Hello 世界 ありがとう");
        assert_eq!(shown(&runs), "Hello 世界(せかい) ありがとう");
    }

    #[test]
    fn romaji_reads_particles_long_vowels_and_endings() {
        let reader = reader();
        assert_eq!(reader.romaji("君の名は。"), "kimi no na wa.");
        assert_eq!(reader.romaji("東京に行きます"), "tōkyō ni ikimasu");
        assert_eq!(
            reader.romaji("夜空に輝く星を見上げて"),
            "yozora ni kagayaku hoshi o miagete"
        );
        assert_eq!(reader.romaji("先生"), "sensei");
    }

    #[test]
    fn alignment_falls_back_to_the_whole_word() {
        // A reading that doesn't contain the kana: one ruby over everything.
        assert_eq!(shown(&align("行き", Some("ユク"))), "行き(ゆく)");
        assert_eq!(shown(&align("行き", Some("イキ"))), "行(い)き");
        assert_eq!(shown(&align("カタカナ", Some("カタカナ"))), "カタカナ");
        assert_eq!(shown(&align("漢字", None)), "漢字");
    }

    #[test]
    fn kana_only_morphemes_are_never_annotated() {
        let runs = reader().furigana("ありがとう");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].reading, None);
    }
}
