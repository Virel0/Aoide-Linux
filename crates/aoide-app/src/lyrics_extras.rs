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

use std::sync::Arc;

use aoide_integrations::translate::libre::LibreTranslate;
use aoide_jellyfin::{Lyrics, ServerAddress};
use iced::widget::{button, column, container, row, text, text_input};
use iced::{Center, Element, Fill, Task};

use crate::app::{App, Message};
use crate::credentials::CredentialStore;
use crate::listening::{Msg, SettingsExtras};
use crate::screens::Load;
use crate::smart::Draft;
use crate::{theme, widgets};

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

/// The translation server's rows in Settings, as they are being edited.
#[derive(Debug, Default)]
pub struct ServerSettings {
    /// The address as typed; `None` until typed in, when the saved one shows.
    pub address: Option<String>,
    pub key: Draft,
    /// Whether the keyring holds a key; `None` before it has been asked.
    pub key_set: Option<bool>,
    /// What the last check of the server found, or why it failed.
    pub check: Option<Result<String, String>>,
    pub busy: bool,
}

/// Settings → Lyrics → Translation.
#[derive(Debug, Clone)]
pub enum ServerMsg {
    AddressTyped(String),
    SaveAddress,
    Checked(Result<usize, String>),
    KeyTyped(Draft),
    SaveKey,
    ClearKey,
    /// The keyring answered: whether a key is there now.
    KeyStored(Result<bool, String>),
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

/// What the address field holds, as the server's URL; `Ok(None)` for an
/// empty field, which turns translation off.
pub fn parse_address(typed: &str) -> Result<Option<String>, String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return Ok(None);
    }
    ServerAddress::parse(typed)
        .map(|address| Some(address.as_str().to_owned()))
        .map_err(|_| "That isn't an address Aoide can use.".to_owned())
}

/// The key from the keyring, off the async threads. A keyring that can't
/// be read is treated as no key; a server that wants one will say so.
async fn read_key(keyring: Arc<dyn CredentialStore>) -> Option<String> {
    tokio::task::spawn_blocking(move || keyring.get(KEY_ENTRY).ok().flatten())
        .await
        .ok()
        .flatten()
}

/// Whether the keyring holds a key, for Settings.
pub fn key_state(keyring: Arc<dyn CredentialStore>) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || keyring.get(KEY_ENTRY).map(|k| k.is_some()))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| e.to_string()))
        },
        |result| Message::Listening(Msg::Server(ServerMsg::KeyStored(result))),
    )
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

    /// Settings → Lyrics → Translation.
    pub(crate) fn translation_server(&mut self, msg: ServerMsg) -> Task<Message> {
        let state = &mut self.listening.lyric_extras.settings;
        match msg {
            ServerMsg::AddressTyped(typed) => {
                state.address = Some(typed);
                Task::none()
            }
            ServerMsg::SaveAddress => {
                let typed = state
                    .address
                    .clone()
                    .unwrap_or_else(|| self.config.translation_server.clone().unwrap_or_default());
                match parse_address(&typed) {
                    Err(why) => {
                        state.check = Some(Err(why));
                        Task::none()
                    }
                    Ok(server) => {
                        state.address = None;
                        state.check = None;
                        self.config.translation_server = server.clone();
                        self.save_config();
                        // Whatever was shown came from the old server, or none.
                        self.listening.lyric_extras.translation = None;
                        let Some(server) = server else {
                            return Task::none();
                        };
                        self.listening.lyric_extras.settings.busy = true;
                        let keyring = self.keyring.clone();
                        let check = Task::perform(
                            async move {
                                let libre = LibreTranslate::new(&server, read_key(keyring).await)
                                    .map_err(|e| e.to_string())?;
                                libre
                                    .languages()
                                    .await
                                    .map(|l| l.len())
                                    .map_err(|e| e.to_string())
                            },
                            |result| Message::Listening(Msg::Server(ServerMsg::Checked(result))),
                        );
                        Task::batch([check, self.start_translation()])
                    }
                }
            }
            ServerMsg::Checked(result) => {
                state.busy = false;
                state.check = Some(match result {
                    Ok(0) => Err("It answered, but has no languages installed.".to_owned()),
                    Ok(n) => Ok(format!("Connected. It translates {n} languages.")),
                    Err(error) => Err(format!("Not connected: {error}.")),
                });
                Task::none()
            }
            ServerMsg::KeyTyped(draft) => {
                state.key = draft;
                Task::none()
            }
            ServerMsg::SaveKey => {
                let key = state.key.0.trim().to_owned();
                if key.is_empty() {
                    return Task::none();
                }
                state.key = Draft::default();
                state.busy = true;
                let keyring = self.keyring.clone();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            keyring.set(KEY_ENTRY, &key).map(|()| true)
                        })
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r.map_err(|e| e.to_string()))
                    },
                    |result| Message::Listening(Msg::Server(ServerMsg::KeyStored(result))),
                )
            }
            ServerMsg::ClearKey => {
                state.busy = true;
                let keyring = self.keyring.clone();
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            keyring.delete(KEY_ENTRY).map(|()| false)
                        })
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r.map_err(|e| e.to_string()))
                    },
                    |result| Message::Listening(Msg::Server(ServerMsg::KeyStored(result))),
                )
            }
            ServerMsg::KeyStored(result) => {
                state.busy = false;
                match result {
                    Ok(set) => state.key_set = Some(set),
                    Err(error) => state.check = Some(Err(format!("The keyring said: {error}."))),
                }
                Task::none()
            }
        }
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

/// Settings → Lyrics → Translation: the server's address and its key.
pub fn settings_row<'a>(extras: &SettingsExtras<'a>) -> Element<'a, Message> {
    let state = &extras.listening.lyric_extras.settings;
    let send = |m: ServerMsg| Message::Listening(Msg::Server(m));
    let saved = extras
        .config
        .translation_server
        .as_deref()
        .unwrap_or_default();
    let address = state.address.as_deref().unwrap_or(saved);
    let mut rows = column![
        text("Translation Server"),
        widgets::secondary(
            "Your own LibreTranslate server translates lyrics that aren't in your language. \
             The lyrics go only to it. Leave the address empty to turn translation off."
        )
        .size(13),
        row![
            text_input("translate.example.net", address)
                .on_input(move |s| send(ServerMsg::AddressTyped(s)))
                .on_submit(send(ServerMsg::SaveAddress))
                .padding(8)
                .style(theme::field),
            button(text("Save").size(13))
                .style(theme::tonal)
                .padding([6, 12])
                .on_press_maybe((!state.busy).then(|| send(ServerMsg::SaveAddress))),
        ]
        .spacing(8)
        .align_y(Center),
    ]
    .spacing(6);
    match &state.check {
        _ if state.busy => rows = rows.push(widgets::secondary("Checking\u{2026}").size(13)),
        Some(Ok(line)) => rows = rows.push(widgets::secondary(line.as_str()).size(13)),
        Some(Err(line)) => rows = rows.push(text(line.as_str()).size(13)),
        None => {}
    }
    if extras.config.translation_server.is_none() {
        return rows.into();
    }
    // The key: saved, cleared, or said to be set. Never shown.
    let key_line = match state.key_set {
        None => "Checking the keyring\u{2026}",
        Some(true) => "A key is set.",
        Some(false) => "No key is set. Only needed if your server asks for one.",
    };
    let mut key_row = row![
        column![text("API Key"), widgets::secondary(key_line).size(13)]
            .spacing(2)
            .width(Fill)
    ]
    .spacing(8)
    .align_y(Center);
    if state.key_set == Some(true) {
        key_row = key_row.push(
            button(text("Clear").size(13))
                .style(theme::destructive)
                .padding([4, 10])
                .on_press_maybe((!state.busy).then(|| send(ServerMsg::ClearKey))),
        );
    }
    let can_save = !state.busy && !state.key.0.trim().is_empty();
    rows.push(key_row)
        .push(
            row![
                text_input("Paste the server's API key", &state.key.0)
                    .secure(true)
                    .on_input(move |s| send(ServerMsg::KeyTyped(Draft(s))))
                    .on_submit_maybe(can_save.then(|| send(ServerMsg::SaveKey)))
                    .padding(8)
                    .style(theme::field),
                button(text("Save").size(13))
                    .style(theme::tonal)
                    .padding([6, 12])
                    .on_press_maybe(can_save.then(|| send(ServerMsg::SaveKey))),
            ]
            .spacing(8)
            .align_y(Center),
        )
        .into()
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
