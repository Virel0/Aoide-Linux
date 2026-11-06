//! What the lyrics panel adds to the words themselves, from Melete: each
//! line's pronunciation in Latin letters (Japanese, Korean, Cyrillic), while
//! the server has pronunciation on; and a translation under each line, when
//! the lyrics aren't in the listener's language and the server has
//! LibreTranslate set up. For Japanese, with the `furigana` feature,
//! aoide-lyrics' `JapaneseReader` (lindera with IPADIC) also puts readings
//! over the kanji, on this computer.

use aoide_melete::{Error, Lyrics, LyricsAsk, MeleteClient};
use iced::widget::{button, column, container, row, text};
use iced::{Center, Element, Task};

use crate::app::{App, Message};
use crate::listening::Msg;
use crate::screens::Load;
use crate::{theme, widgets};

#[derive(Debug, Default)]
pub struct State {
    /// The track the lyrics below were read for.
    pub item: Option<String>,
    /// The lyrics' language, when it isn't the listener's.
    pub foreign: Option<&'static str>,
    /// Translate is switched on (for this session).
    pub translate: bool,
    pub translation: Option<Translation>,
    /// The server said it has no translation set up: Translate isn't offered.
    pub translation_off: bool,
    /// The server said pronunciation is off: it isn't asked for again.
    pub pronunciation_off: bool,
    /// Each line's pronunciation, for the track named, as the server gave it.
    pub pronounced: Option<(String, Vec<Option<String>>)>,
    /// The lyrics have kana: Japanese.
    pub japanese: bool,
    /// Each line in runs with their readings (the `furigana` feature).
    pub ruby: Vec<Vec<RubyRun>>,
    #[cfg(feature = "furigana")]
    reader: Option<aoide_lyrics::JapaneseReader>,
}

/// A run of a line and, over its kanji, how they're read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RubyRun {
    pub text: String,
    pub reading: Option<String>,
}

/// Whether this build reads kanji.
pub const CAN_READ_KANJI: bool = cfg!(feature = "furigana");

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
pub fn failure(error: &Error) -> String {
    match error {
        Error::Http { status: 409, .. } => "Your server has no translation set up.".to_owned(),
        error => format!(
            "The lyrics couldn't be translated: {}.",
            crate::failure::describe(error)
        ),
    }
}

/// The playing track's lyrics from Melete, with each line's pronunciation
/// unless the server has said it's off. A 409 is pronunciation off: asked
/// again without, and the second value says so.
pub async fn fetch(
    client: MeleteClient,
    item: String,
    pronounce: bool,
) -> (Result<Option<aoide_melete::LyricsAnswer>, Error>, bool) {
    let ask = LyricsAsk {
        translate: None,
        pronounce,
    };
    match client.lyrics_with(&item, &ask).await {
        Err(Error::Http { status: 409, .. }) if pronounce => {
            (client.lyrics_with(&item, &LyricsAsk::default()).await, true)
        }
        result => (result, false),
    }
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

    /// New lyrics are showing: what language, readings for Japanese, and a
    /// translation if one is switched on.
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

    /// Readings over the kanji of Japanese lyrics.
    #[cfg(feature = "furigana")]
    fn read_japanese(&mut self, lyrics: &Lyrics) {
        let extras = &mut self.listening.lyric_extras;
        if extras.reader.is_none() {
            extras.reader = aoide_lyrics::JapaneseReader::new().ok();
        }
        if let Some(reader) = &extras.reader {
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
    }

    #[cfg(not(feature = "furigana"))]
    fn read_japanese(&mut self, _lyrics: &Lyrics) {}

    /// Asks the server to translate the lyrics showing, if asked and worth it.
    fn start_translation(&mut self) -> Task<Message> {
        let extras = &self.listening.lyric_extras;
        let (Some(_), true, None, false) = (
            extras.foreign,
            extras.translate,
            &extras.translation,
            extras.translation_off,
        ) else {
            return Task::none();
        };
        let Some((item, _)) = self.shown_lyrics() else {
            return Task::none();
        };
        let Some(signed) = self.session.as_ref() else {
            return Task::none();
        };
        let client = signed.connection.client.clone();
        self.listening.lyric_extras.translation = Some(Translation::Working);
        let ask = LyricsAsk {
            translate: Some(target_language()),
            pronounce: false,
        };
        Task::perform(
            async move {
                let result = client.lyrics_with(&item, &ask).await;
                (item, result)
            },
            |(item, result)| match result {
                Err(Error::Unauthorized) => Message::TokenRejected,
                Ok(answer) => Message::Listening(Msg::Translated(
                    item,
                    answer
                        .and_then(|a| a.translations)
                        .ok_or_else(|| "The server sent no translation.".to_owned()),
                    false,
                )),
                Err(error) => Message::Listening(Msg::Translated(
                    item,
                    Err(failure(&error)),
                    matches!(error, Error::Http { status: 409, .. }),
                )),
            },
        )
    }

    pub(crate) fn translated(
        &mut self,
        item: String,
        result: Result<Vec<String>, String>,
        not_set_up: bool,
    ) {
        let extras = &mut self.listening.lyric_extras;
        extras.translation_off |= not_set_up;
        if extras.item.as_deref() != Some(item.as_str()) {
            return;
        }
        extras.translation = Some(match result {
            Ok(lines) => Translation::Ready(lines),
            Err(why) => Translation::Failed(why),
        });
    }

    /// Each line's pronunciation for `item`, or that the server has it off.
    pub(crate) fn pronounced(&mut self, item: &str, lines: Option<Vec<Option<String>>>, off: bool) {
        let extras = &mut self.listening.lyric_extras;
        extras.pronunciation_off |= off;
        extras.pronounced = lines
            .filter(|l| l.iter().any(Option::is_some))
            .map(|l| (item.to_owned(), l));
    }
}

/// The switches above the lyrics, and what translating says; `None` when
/// there is nothing to switch.
pub fn toolbar<'a>(app: &'a App, item: &str) -> Option<Element<'a, Message>> {
    let extras = &app.listening.lyric_extras;
    if extras.item.as_deref() != Some(item) {
        return None;
    }
    let offer_translation =
        extras.foreign.is_some() && (!extras.translation_off || extras.translate);
    let pronounced = pronunciations(app, item).is_some();
    let furigana = CAN_READ_KANJI && extras.japanese;
    if !offer_translation && !pronounced && !furigana {
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
    if pronounced {
        let mut changed = interface.clone();
        changed.romaji = !interface.romaji;
        bar = bar.push(switch(
            "Pronunciation",
            interface.romaji,
            Msg::Interface(changed),
        ));
    }
    if furigana {
        let mut changed = interface.clone();
        changed.furigana = !interface.furigana;
        bar = bar.push(switch(
            "Furigana",
            interface.furigana,
            Msg::Interface(changed),
        ));
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
            if !extras.translation_off {
                notes = notes.push(
                    widgets::secondary("Translated by your server's LibreTranslate.").size(11),
                );
            }
        }
    }
    Some(column![bar, notes].spacing(6).into())
}

/// The server's pronunciation of each line of `item`'s lyrics, when it gave any.
fn pronunciations<'a>(app: &'a App, item: &str) -> Option<&'a [Option<String>]> {
    match &app.listening.lyric_extras.pronounced {
        Some((for_item, lines)) if for_item == item => Some(lines),
        _ => None,
    }
}

/// What goes under line `index`: its pronunciation, its translation.
pub fn under<'a>(app: &'a App, item: &str, index: usize) -> Option<Element<'a, Message>> {
    let said = under_text(app, item, index);
    if said.is_empty() {
        return None;
    }
    let lines = said
        .into_iter()
        .map(|line| widgets::secondary(line).size(13).into());
    Some(
        container(column(lines).spacing(2))
            .padding(iced::Padding::ZERO.left(4).bottom(4))
            .into(),
    )
}

/// The words under line `index`, as switched on: its pronunciation, then
/// its translation.
pub fn under_text<'a>(app: &'a App, item: &str, index: usize) -> Vec<&'a str> {
    let extras = &app.listening.lyric_extras;
    if extras.item.as_deref() != Some(item) {
        return Vec::new();
    }
    let mut said = Vec::new();
    if app.config.interface.romaji
        && let Some(line) = pronunciations(app, item)
            .and_then(|all| all.get(index))
            .and_then(Option::as_deref)
            .filter(|l| !l.trim().is_empty())
    {
        said.push(line);
    }
    if extras.translate
        && let Some(Translation::Ready(translated)) = &extras.translation
        && let Some(line) = translated.get(index).filter(|l| !l.trim().is_empty())
    {
        said.push(line.as_str());
    }
    said
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
    fn japanese_lyrics_are_told_by_their_kana() {
        assert!(is_japanese(&lyrics("さよならは言わないで\nありがとう")));
        assert!(!is_japanese(&lyrics("Sous le ciel de Paris")));
    }

    #[test]
    fn a_translation_the_server_cant_make_says_why() {
        let off = Error::Http {
            status: 409,
            path: "api/v1/music/tracks/1/lyrics".into(),
            detail: None,
            reason: None,
        };
        assert_eq!(failure(&off), "Your server has no translation set up.");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pronunciation_turned_off_is_asked_again_without_it() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/music/tracks/2/lyrics"))
            .and(query_param("pronounce", "true"))
            .respond_with(ResponseTemplate::new(409).set_body_json(
                serde_json::json!({"error": "lyrics' pronunciation isn't turned on on this server"}),
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/music/tracks/2/lyrics"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "track_id": 2, "source": "file", "synced": true,
                "lines": [{"start_seconds": 1.0, "text": "First line"}],
            })))
            .mount(&server)
            .await;
        let client = MeleteClient::new(
            aoide_melete::ServerAddress::parse(&server.uri()).unwrap(),
            aoide_melete::Device::new("Test", "device-1"),
        )
        .unwrap()
        .with_credentials(aoide_melete::Credentials {
            user_id: "u1".into(),
            access_token: "tok".into(),
            username: "alice".into(),
        });
        let (answer, off) = fetch(client.clone(), "2".into(), true).await;
        let answer = answer.unwrap().unwrap();
        assert!(off, "the server said pronunciation is off");
        assert_eq!(answer.lyrics.lines[0].text, "First line");
        assert_eq!(answer.pronunciations, None);
        let (_, off) = fetch(client, "2".into(), false).await;
        assert!(!off);
    }

    #[cfg(feature = "furigana")]
    #[test]
    fn with_the_dictionary_kanji_are_read() {
        let reader = aoide_lyrics::JapaneseReader::new().unwrap();
        let runs = reader.furigana("夜空に輝く");
        assert_eq!(runs[0].reading.as_deref(), Some("よぞら"));
    }
}
