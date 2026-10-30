//! What the lyrics panel adds to the words themselves: a translation under
//! each line, when the lyrics aren't in the listener's language; and for
//! Japanese, romaji under each line and readings over the kanji.
//!
//! Kana to romaji is always there (kanji pass through it unread). With the
//! `furigana` feature, aoide-lyrics' `JapaneseReader` (lindera with IPADIC)
//! reads the kanji too: readings over them, and romaji of the whole line.
//!
//! Translation is done by the listener's own LibreTranslate server
//! (aoide-integrations' `LibreTranslate`), named in Settings → Lyrics with
//! its API key if it wants one. Until a server is named the Translate switch
//! isn't there. The lyrics go to that server and nowhere else.

use aoide_integrations::translate::libre::LibreTranslate;
use aoide_melete::Lyrics;
use iced::widget::{button, column, container, row, text};
use iced::{Center, Element, Task};

use crate::app::{App, Message};
use crate::listening::Msg;
use crate::screens::Load;
use crate::{theme, widgets};

mod server;

use server::read_key;
pub use server::{ServerMsg, ServerSettings, key_state, settings_row};

/// The keyring entry for the translation server's API key.
pub const KEY_ENTRY: &str = "libretranslate-key";

#[derive(Debug, Default)]
pub struct State {
    /// The track the lyrics below were read for.
    pub item: Option<String>,
    /// The lyrics' language, when it isn't the listener's.
    pub foreign: Option<&'static str>,
    /// Translate is switched on (for this session).
    pub translate: bool,
    pub translation: Option<Translation>,
    /// The lyrics have kana: Japanese.
    pub japanese: bool,
    /// Each line in romaji, for Japanese lyrics.
    pub romaji: Vec<String>,
    /// Each line in runs with their readings (the `furigana` feature).
    pub ruby: Vec<Vec<RubyRun>>,
    #[cfg(feature = "furigana")]
    reader: Option<aoide_lyrics::JapaneseReader>,
    /// Settings → Lyrics → Translation.
    pub settings: ServerSettings,
}

/// A run of a line and, over its kanji, how they're read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RubyRun {
    pub text: String,
    pub reading: Option<String>,
}

/// Whether this build reads kanji.
pub const CAN_READ_KANJI: bool = cfg!(feature = "furigana");

/// Japanese lyrics' romaji, line by line: by the reader when there is one,
/// else kana by kana.
pub fn romaji_lines(lyrics: &Lyrics, reader: Option<&dyn Fn(&str) -> String>) -> Vec<String> {
    lyrics
        .lines
        .iter()
        .map(|line| match reader {
            Some(read) => read(&line.text),
            None => aoide_lyrics::kana_to_romaji(&line.text),
        })
        .collect()
}

/// Whether lyrics are Japanese: they have kana.
pub fn is_japanese(lyrics: &Lyrics) -> bool {
    lyrics
        .lines
        .iter()
        .any(|l| aoide_lyrics::is_japanese(&l.text))
}

#[derive(Debug, Clone, PartialEq)]
pub enum Translation {
    Working,
    Ready(Vec<String>),
    Failed(String),
}

/// The listener's language: the locale's, else English.
pub fn target_language() -> String {
    aoide_integrations::translate::system_language(|name| std::env::var_os(name))
        .unwrap_or_else(|| "en".to_owned())
}

/// The language `lyrics` are in, when a translation into `target` is worth
/// offering.
pub fn foreign_language(lyrics: &Lyrics, target: &str) -> Option<&'static str> {
    let lines: Vec<String> = lyrics.lines.iter().map(|l| l.text.clone()).collect();
    aoide_integrations::translate::needs_translation(&lines, target)
}

/// A failed translation in a sentence.
pub fn failure(error: &str) -> String {
    format!("The lyrics couldn't be translated: {error}.")
}

impl App {
    /// The lyrics showing, for the playing track.
    fn shown_lyrics(&self) -> Option<(String, Lyrics)> {
        let signed = self.session.as_ref()?;
        let state = signed.lyrics.as_ref()?;
        let current = self.playback.current()?.1.meta.id.clone();
        match &state.lyrics {
            Load::Ready(Some(lyrics)) if state.item == current => {
                Some((state.item.clone(), lyrics.clone()))
            }
            _ => None,
        }
    }

    /// New lyrics are showing: what language, and a translation if one is
    /// switched on.
    pub(crate) fn lyrics_shown(&mut self) -> Task<Message> {
        let Some((item, lyrics)) = self.shown_lyrics() else {
            return Task::none();
        };
        let extras = &mut self.listening.lyric_extras;
        if extras.item.as_deref() == Some(item.as_str()) {
            return Task::none();
        }
        extras.item = Some(item);
        extras.foreign = foreign_language(&lyrics, &target_language());
        extras.translation = None;
        extras.japanese = is_japanese(&lyrics);
        extras.romaji.clear();
        extras.ruby.clear();
        if extras.japanese {
            self.read_japanese(&lyrics);
        }
        self.start_translation()
    }

    pub(crate) fn set_translate(&mut self, on: bool) -> Task<Message> {
        self.listening.lyric_extras.translate = on;
        // Off and on again is how a failed one is tried again.
        if !on
            && matches!(
                self.listening.lyric_extras.translation,
                Some(Translation::Failed(_))
            )
        {
            self.listening.lyric_extras.translation = None;
        }
        if on {
            self.start_translation()
        } else {
            Task::none()
        }
    }

    /// Romaji (and with the feature, readings) for Japanese lyrics.
    #[cfg(feature = "furigana")]
    fn read_japanese(&mut self, lyrics: &Lyrics) {
        let extras = &mut self.listening.lyric_extras;
        if extras.reader.is_none() {
            extras.reader = aoide_lyrics::JapaneseReader::new().ok();
        }
        match &extras.reader {
            Some(reader) => {
                let read = |line: &str| reader.romaji(line);
                extras.romaji = romaji_lines(lyrics, Some(&read));
                extras.ruby = lyrics
                    .lines
                    .iter()
                    .map(|line| {
                        reader
                            .furigana(&line.text)
                            .into_iter()
                            .map(|r| RubyRun {
                                text: r.text,
                                reading: r.reading,
                            })
                            .collect()
                    })
                    .collect();
            }
            None => extras.romaji = romaji_lines(lyrics, None),
        }
    }

    #[cfg(not(feature = "furigana"))]
    fn read_japanese(&mut self, lyrics: &Lyrics) {
        self.listening.lyric_extras.romaji = romaji_lines(lyrics, None);
    }

    /// Translates the lyrics showing, if asked, worth it and a server is
    /// named.
    fn start_translation(&mut self) -> Task<Message> {
        let extras = &self.listening.lyric_extras;
        let (Some(_), true, None) = (extras.foreign, extras.translate, &extras.translation) else {
            return Task::none();
        };
        let Some(server) = self.config.translation_server.clone() else {
            return Task::none();
        };
        let Some((item, lyrics)) = self.shown_lyrics() else {
            return Task::none();
        };
        self.listening.lyric_extras.translation = Some(Translation::Working);
        let keyring = self.keyring.clone();
        let to = target_language();
        let lines: Vec<String> = lyrics.lines.iter().map(|l| l.text.clone()).collect();
        Task::perform(
            async move {
                let key = read_key(keyring).await;
                let result = match LibreTranslate::new(&server, key) {
                    Ok(libre) => libre
                        .translate_lines(&lines, &to)
                        .await
                        .map_err(|e| e.to_string()),
                    Err(error) => Err(error.to_string()),
                };
                (item, result)
            },
            |(item, result)| Message::Listening(Msg::Translated(item, result)),
        )
    }

    pub(crate) fn translated(&mut self, item: String, result: Result<Vec<String>, String>) {
        let extras = &mut self.listening.lyric_extras;
        if extras.item.as_deref() != Some(item.as_str()) {
            return;
        }
        extras.translation = Some(match result {
            Ok(lines) => Translation::Ready(lines),
            Err(error) => Translation::Failed(failure(&error)),
        });
    }
}

/// The switches above the lyrics, and what translating says; `None` when
/// there is nothing to switch.
pub fn toolbar<'a>(app: &'a App, item: &str) -> Option<Element<'a, Message>> {
    let extras = &app.listening.lyric_extras;
    if extras.item.as_deref() != Some(item) {
        return None;
    }
    let offer_translation = app.config.translation_server.is_some() && extras.foreign.is_some();
    if !offer_translation && !extras.japanese {
        return None;
    }
    let interface = &app.config.interface;
    let switch = |label: &'a str, on: bool, message: Msg| {
        button(text(label).size(12))
            .padding([4, 10])
            .style(if on { theme::filled } else { theme::tonal })
            .on_press(Message::Listening(message))
    };
    let mut bar = row![].spacing(6).align_y(Center);
    let mut notes = column![].spacing(4);
    if extras.japanese {
        let mut romaji = interface.clone();
        romaji.romaji = !interface.romaji;
        bar = bar.push(switch("Romaji", interface.romaji, Msg::Interface(romaji)));
        if CAN_READ_KANJI {
            let mut furigana = interface.clone();
            furigana.furigana = !interface.furigana;
            bar = bar.push(switch(
                "Furigana",
                interface.furigana,
                Msg::Interface(furigana),
            ));
        } else if interface.romaji {
            notes = notes
                .push(widgets::secondary("Kana in romaji; kanji are left as they are.").size(11));
        }
    }
    if offer_translation {
        bar = bar.push(switch(
            "Translate",
            extras.translate,
            Msg::Translate(!extras.translate),
        ));
        if extras.translate {
            let status = match &extras.translation {
                Some(Translation::Working) => "Translating\u{2026}".to_owned(),
                Some(Translation::Failed(why)) => why.clone(),
                _ => String::new(),
            };
            if !status.is_empty() {
                notes = notes.push(widgets::secondary(status).size(12));
            }
            notes = notes.push(
                widgets::secondary(
                    "Translated by your LibreTranslate server; the lyrics go only to it.",
                )
                .size(11),
            );
        }
    }
    Some(column![bar, notes].spacing(6).into())
}

/// What goes under line `index`: its romaji, its translation.
pub fn under<'a>(app: &'a App, item: &str, index: usize) -> Option<Element<'a, Message>> {
    let extras = &app.listening.lyric_extras;
    if extras.item.as_deref() != Some(item) {
        return None;
    }
    let mut lines = column![].spacing(2);
    let mut any = false;
    if app.config.interface.romaji
        && let Some(romaji) = extras.romaji.get(index).filter(|l| !l.trim().is_empty())
    {
        lines = lines.push(widgets::secondary(romaji.as_str()).size(13));
        any = true;
    }
    if extras.translate
        && let Some(Translation::Ready(translated)) = &extras.translation
        && let Some(line) = translated.get(index).filter(|l| !l.trim().is_empty())
    {
        lines = lines.push(widgets::secondary(line.as_str()).size(13));
        any = true;
    }
    any.then(|| {
        container(lines)
            .padding(iced::Padding::ZERO.left(4).bottom(4))
            .into()
    })
}

/// Line `index` with its readings over the kanji, when furigana are on:
/// each run a small reading above its text, the runs wrapping as words do.
pub fn ruby_line<'a>(
    app: &'a App,
    item: &str,
    index: usize,
    size: f32,
    font: iced::Font,
    colour: fn(&iced::Theme) -> iced::Color,
) -> Option<Element<'a, Message>> {
    let extras = &app.listening.lyric_extras;
    if !CAN_READ_KANJI || !app.config.interface.furigana || extras.item.as_deref() != Some(item) {
        return None;
    }
    let runs = extras.ruby.get(index).filter(|r| !r.is_empty())?;
    let style = move |t: &iced::Theme| text::Style {
        color: Some(colour(t)),
    };
    let cells = runs.iter().map(|run| {
        column![
            text(run.reading.clone().unwrap_or_default())
                .size(size * 0.5)
                .style(style),
            text(run.text.as_str()).size(size).font(font).style(style),
        ]
        .align_x(Center)
        .into()
    });
    Some(row(cells).wrap().into())
}

#[cfg(test)]
mod tests {
    use super::server::{forgets_key, parse_address};
    use super::*;

    fn lyrics(text: &str) -> Lyrics {
        aoide_lyrics::parse_plain(text)
    }

    #[test]
    fn a_translation_is_offered_only_out_of_the_listeners_language() {
        let french = lyrics(
            "Sous le ciel de Paris\ns'envole une chanson\nelle est née d'aujourd'hui\n\
             dans le cœur d'un garçon",
        );
        assert_eq!(foreign_language(&french, "en"), Some("fr"));
        assert_eq!(foreign_language(&french, "fr"), None);
        assert_eq!(foreign_language(&lyrics("la la"), "en"), None);
    }

    #[test]
    fn japanese_lyrics_get_romaji_kana_by_kana() {
        let japanese = lyrics("さよならは言わないで\nありがとう");
        assert!(is_japanese(&japanese));
        assert!(!is_japanese(&lyrics("Sous le ciel de Paris")));
        let romaji = romaji_lines(&japanese, None);
        assert_eq!(romaji[1], "arigatou");
        // Without a dictionary the kanji pass through.
        assert!(romaji[0].contains('言'), "{}", romaji[0]);
        let upper = |s: &str| s.to_uppercase();
        assert_eq!(
            romaji_lines(&japanese, Some(&upper))[1],
            "ありがとう".to_uppercase()
        );
    }

    #[cfg(feature = "furigana")]
    #[test]
    fn with_the_dictionary_kanji_are_read() {
        let reader = aoide_lyrics::JapaneseReader::new().unwrap();
        let japanese = lyrics("夜空に輝く");
        let read = |line: &str| reader.romaji(line);
        let romaji = romaji_lines(&japanese, Some(&read));
        assert!(romaji[0].starts_with("yozora"), "{}", romaji[0]);
        let runs = reader.furigana("夜空に輝く");
        assert_eq!(runs[0].reading.as_deref(), Some("よぞら"));
    }

    #[test]
    fn a_key_is_never_kept_for_another_server() {
        let ours = Some("https://translate.example.net");
        assert!(forgets_key(ours, Some("https://translate.example.org")));
        assert!(forgets_key(ours, None));
        assert!(!forgets_key(ours, ours));
        assert!(!forgets_key(None, ours));
    }

    #[test]
    fn the_address_is_read_as_the_sign_in_reads_one() {
        assert_eq!(parse_address("  "), Ok(None));
        assert_eq!(
            parse_address("translate.example.net").unwrap().as_deref(),
            Some("https://translate.example.net")
        );
        assert_eq!(
            parse_address("192.168.1.10:5000").unwrap().as_deref(),
            Some("http://192.168.1.10:5000")
        );
        assert!(parse_address("ftp://translate.example.net").is_err());
    }
}
