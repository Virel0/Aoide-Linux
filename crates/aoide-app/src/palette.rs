//! The command palette (Ctrl+K; ⌘K on macOS): go to any screen or Aoide
//! playlist, drive the player, and find songs, albums and artists, all from
//! the keyboard. Up and Down choose, Enter runs, Escape closes.
//!
//! The commands are matched as typed; the library is searched (Jellyfin's
//! plain search, the same as the Search screen's) once typing pauses.

use std::time::Duration;

use aoide_jellyfin::SearchResults;
use aoide_store::PlaySource;
use iced::keyboard::{self, Key, key::Named};
use iced::widget::{Column, button, column, container, row, scrollable, text, text_input};
use iced::{Center, Element, Fill, Subscription, Task};

use crate::actions::PlayRequest;
use crate::app::{App, Message as AppMessage, Panel, Transport};
use crate::playback::tracks::TrackMeta;
use crate::screens::search::Debouncer;
use crate::screens::{PlaylistSummary, Route, Section};
use crate::theme;
use crate::widgets;

pub const FIELD: &str = "palette-field";

/// How long typing must pause before the library is searched.
const DEBOUNCE: Duration = Duration::from_millis(250);
/// Commands listed at once.
const COMMANDS: usize = 8;
/// Results of each kind.
const RESULTS: u32 = 5;

/// One line of the palette, and what choosing it does.
#[derive(Debug, Clone)]
pub struct Entry {
    pub label: String,
    /// What kind of thing it is ("Go to", "Song", …).
    pub kind: &'static str,
    pub detail: Option<String>,
    pub action: AppMessage,
}

impl Entry {
    fn new(kind: &'static str, label: impl Into<String>, action: AppMessage) -> Self {
        Self {
            label: label.into(),
            kind,
            detail: None,
            action,
        }
    }
}

/// Every command: a screen to go to, a playlist, a player control.
pub fn commands(playlists: &[PlaylistSummary]) -> Vec<Entry> {
    let mut all: Vec<Entry> = Section::LIBRARY
        .into_iter()
        .chain([Section::Settings])
        .map(|section| Entry::new("Go to", section.title(), AppMessage::Section(section)))
        .collect();
    let player =
        |label: &str, control: Transport| Entry::new("Player", label, AppMessage::Player(control));
    all.extend([
        player("Play or Pause", Transport::Toggle),
        player("Next Song", Transport::Next),
        player("Previous Song", Transport::Previous),
        player("Shuffle", Transport::Shuffle),
        player("Repeat", Transport::Repeat),
        player("Crossfade", Transport::Crossfade),
        player("Infinity", Transport::Infinity),
        player("Show Queue", Transport::Panel(Panel::Queue)),
        player("Show Lyrics", Transport::Panel(Panel::Lyrics)),
    ]);
    all.extend(playlists.iter().map(|p| {
        Entry::new(
            "Playlist",
            p.name.clone(),
            AppMessage::Section(Section::Playlist(p.id.clone())),
        )
    }));
    all
}

/// Whether every word typed is in the label, ignoring case: "nx so" finds
/// "Next Song".
pub fn matches(label: &str, query: &str) -> bool {
    let label = label.to_lowercase();
    query
        .split_whitespace()
        .all(|word| label.contains(&word.to_lowercase()))
}

/// The commands for what is typed: all of them on an empty field, else the
/// matching ones, those whose label starts with the query first.
pub fn matching(all: Vec<Entry>, query: &str) -> Vec<Entry> {
    let query = query.trim();
    let mut found: Vec<Entry> = all
        .into_iter()
        .filter(|e| matches(&e.label, query) || matches(&format!("{} {}", e.kind, e.label), query))
        .collect();
    if !query.is_empty() {
        let lowered = query.to_lowercase();
        found.sort_by_key(|e| !e.label.to_lowercase().starts_with(&lowered));
    }
    found
}

/// The selection moved by `delta`, wrapping at both ends.
pub fn moved(selected: usize, delta: i32, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let len = len as i64;
    (i64::try_from(selected).unwrap_or(0) + i64::from(delta)).rem_euclid(len) as usize
}

/// Search results as entries: a song plays, an album or artist opens.
pub fn results(found: &SearchResults) -> Vec<Entry> {
    let mut entries = Vec::new();
    for item in &found.songs {
        let track = TrackMeta::from_item(item);
        let mut entry = Entry::new(
            "Song",
            track.title.clone(),
            AppMessage::Play(PlayRequest::from(
                std::slice::from_ref(&track),
                0,
                PlaySource::Search,
            )),
        );
        entry.detail = Some(track.artist.clone()).filter(|a| !a.is_empty());
        entries.push(entry);
    }
    for item in &found.albums {
        let mut entry = Entry::new(
            "Album",
            item.name.clone(),
            AppMessage::Open(Route::Album {
                id: item.id.clone(),
                title: item.name.clone(),
            }),
        );
        entry.detail = item.album_artist.clone();
        entries.push(entry);
    }
    for item in &found.artists {
        entries.push(Entry::new(
            "Artist",
            item.name.clone(),
            AppMessage::Open(Route::Artist {
                id: item.id.clone(),
                name: item.name.clone(),
            }),
        ));
    }
    entries
}

/// The open palette.
#[derive(Debug, Default)]
pub struct Palette {
    pub query: String,
    pub selected: usize,
    debouncer: Debouncer,
    /// The query the results are for, and the results.
    found: Option<(String, Vec<Entry>)>,
}

impl Palette {
    /// Everything listed now, in order.
    pub fn entries(&self, playlists: &[PlaylistSummary]) -> Vec<Entry> {
        let mut entries: Vec<Entry> = matching(commands(playlists), &self.query)
            .into_iter()
            .take(COMMANDS)
            .collect();
        if let Some((query, found)) = &self.found
            && query == self.query.trim()
        {
            entries.extend(found.iter().cloned());
        }
        entries
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    Toggle,
    Close,
    /// Tab (forward) or Shift+Tab: the next or previous field.
    Tab(bool),
    Typed(String),
    Due(u64),
    Found(String, Result<SearchResults, String>),
    Move(i32),
    Run(usize),
    RunSelected,
}

/// The palette's keys, wherever the focus is. Escape is taken even from a
/// focused field (which would otherwise only lose its focus); with the
/// palette shut it closes whatever else is open on top. Tab moves between
/// fields.
fn key(
    event: iced::Event,
    _status: iced::event::Status,
    _window: iced::window::Id,
) -> Option<Message> {
    let iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event else {
        return None;
    };
    match key.as_ref() {
        Key::Character(c) if modifiers.command() && c.eq_ignore_ascii_case("k") => {
            Some(Message::Toggle)
        }
        Key::Named(Named::Escape) => Some(Message::Close),
        Key::Named(Named::Tab) => Some(Message::Tab(!modifiers.shift())),
        Key::Named(Named::ArrowDown) => Some(Message::Move(1)),
        Key::Named(Named::ArrowUp) => Some(Message::Move(-1)),
        _ => None,
    }
}

/// iced moves no focus on Tab itself: on to the next field, or back.
fn tab<T>(forward: bool) -> Task<T> {
    if forward {
        iced::widget::operation::focus_next()
    } else {
        iced::widget::operation::focus_previous()
    }
}

pub fn keys() -> Subscription<Message> {
    iced::event::listen_with(key)
}

impl App {
    pub(crate) fn palette_message(&mut self, message: Message) -> Task<AppMessage> {
        let Some(signed) = &self.session else {
            self.system.palette = None;
            return match message {
                Message::Tab(forward) => tab(forward),
                _ => Task::none(),
            };
        };
        let open = &mut self.system.palette;
        match message {
            Message::Toggle => {
                if open.take().is_some() {
                    return Task::none();
                }
                *open = Some(Palette::default());
                iced::widget::operation::focus(FIELD)
            }
            Message::Tab(forward) => tab(forward),
            Message::Close => {
                if open.take().is_some() || self.listening.full.is_some() {
                    return Task::none();
                }
                // Otherwise Escape closes what is on top, as a click outside
                // it does: Get Info, the release notes, a player bar
                // popover, then a row's or a tile's actions.
                let close = if signed.info.is_some() {
                    Some(AppMessage::Info(crate::info::Msg::Close))
                } else if self.system.updates.notes_open {
                    Some(AppMessage::Update(crate::updates::Message::ShowNotes(
                        false,
                    )))
                } else if self.listening.menu.is_some() {
                    Some(AppMessage::Listening(crate::listening::Msg::Menu(None)))
                } else if signed.menu.is_some() {
                    Some(AppMessage::Row(crate::actions::RowAction::Close))
                } else {
                    None
                };
                close.map_or_else(Task::none, Task::done)
            }
            Message::Typed(query) => {
                let Some(palette) = open.as_mut() else {
                    return Task::none();
                };
                palette.query = query;
                palette.selected = 0;
                let generation = palette.debouncer.typed();
                Task::perform(tokio::time::sleep(DEBOUNCE), move |()| {
                    AppMessage::Palette(Message::Due(generation))
                })
            }
            Message::Due(generation) => {
                let Some(palette) = open.as_mut() else {
                    return Task::none();
                };
                let query = palette.query.trim().to_owned();
                if !palette.debouncer.is_current(generation) || query.chars().count() < 2 {
                    return Task::none();
                }
                let client = signed.connection.client.clone();
                let library = signed.library.clone();
                let asked = query.clone();
                Task::perform(
                    async move { client.search(&asked, library.as_deref(), RESULTS).await },
                    move |result| match result {
                        Err(aoide_jellyfin::Error::Unauthorized) => AppMessage::TokenRejected,
                        result => AppMessage::Palette(Message::Found(
                            query.clone(),
                            result.map_err(|e| crate::session::describe(&e)),
                        )),
                    },
                )
            }
            Message::Found(query, result) => {
                if let Some(palette) = open.as_mut()
                    && query == palette.query.trim()
                {
                    palette.found = Some((query, result.map(|f| results(&f)).unwrap_or_default()));
                }
                Task::none()
            }
            Message::Move(delta) => {
                if let Some(palette) = open.as_mut() {
                    let len = palette.entries(&signed.playlists).len();
                    palette.selected = moved(palette.selected, delta, len);
                }
                Task::none()
            }
            Message::Run(index) => {
                let entry = open
                    .as_ref()
                    .and_then(|p| p.entries(&signed.playlists).into_iter().nth(index));
                let Some(entry) = entry else {
                    return Task::none();
                };
                *open = None;
                Task::done(entry.action)
            }
            Message::RunSelected => {
                let selected = open.as_ref().map_or(0, |p| p.selected);
                self.palette_message(Message::Run(selected))
            }
        }
    }
}

/// The palette, over the window.
pub fn view(app: &App) -> Option<Element<'_, AppMessage>> {
    let palette = app.system.palette.as_ref()?;
    let signed = app.session.as_ref()?;
    let send = |m: Message| AppMessage::Palette(m);
    let field = text_input(
        "Go to, play, or find a song, album or artist",
        &palette.query,
    )
    .id(FIELD)
    .on_input(|q| AppMessage::Palette(Message::Typed(q)))
    .on_submit(send(Message::RunSelected))
    .padding(10)
    .size(16)
    .style(theme::field);
    let entries = palette.entries(&signed.playlists);
    let mut list: Column<'_, AppMessage> = column![].spacing(2);
    if entries.is_empty() {
        list = list.push(widgets::secondary("Nothing matches that.").size(13));
    }
    for (index, entry) in entries.into_iter().enumerate() {
        let mut line = row![
            container(widgets::secondary(entry.kind).size(12)).width(70),
            text(entry.label).size(14),
        ]
        .spacing(10)
        .align_y(Center);
        if let Some(detail) = entry.detail {
            line = line.push(widgets::secondary(detail).size(12));
        }
        list = list.push(
            button(line)
                .width(Fill)
                .padding([5, 10])
                .style(if index == palette.selected {
                    theme::row_selected
                } else {
                    theme::row
                })
                .on_press(send(Message::Run(index))),
        );
    }
    let card = column![
        field,
        scrollable(list).height(iced::Length::Shrink),
        widgets::secondary("↑ ↓ to choose · Enter to run · Esc to close").size(11),
    ]
    .spacing(10);
    Some(crate::system::sheet(card.into(), send(Message::Close)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(entries: &[Entry]) -> Vec<String> {
        entries.iter().map(|e| e.label.clone()).collect()
    }

    #[test]
    fn every_screen_and_playlist_is_a_command() {
        let playlists = [PlaylistSummary {
            id: "p1".into(),
            name: "Road Trip".into(),
            count: 3,
            smart: false,
            folder_id: None,
        }];
        let all = commands(&playlists);
        for section in Section::LIBRARY.iter().chain([&Section::Settings]) {
            assert!(
                all.iter()
                    .any(|e| matches!(&e.action, AppMessage::Section(s) if s == section)),
                "{section:?}"
            );
        }
        assert!(all.iter().any(|e| e.label == "Road Trip"
            && matches!(&e.action, AppMessage::Section(Section::Playlist(id)) if id == "p1")));
        assert!(
            all.iter()
                .any(|e| matches!(e.action, AppMessage::Player(Transport::Toggle)))
        );
    }

    #[test]
    fn typing_narrows_by_every_word() {
        assert!(matches("Next Song", "nex so"));
        assert!(matches("Next Song", ""));
        assert!(!matches("Next Song", "next album"));
        let found = matching(commands(&[]), "so");
        assert_eq!(
            labels(&found)[0],
            "Songs",
            "a label that starts with it comes first"
        );
        assert!(labels(&found).contains(&"Next Song".to_owned()));
        // The kind counts too: "go to set" finds Settings.
        assert_eq!(labels(&matching(commands(&[]), "go to set")), ["Settings"]);
    }

    #[test]
    fn the_selection_wraps() {
        assert_eq!(moved(0, -1, 5), 4);
        assert_eq!(moved(4, 1, 5), 0);
        assert_eq!(moved(2, 1, 5), 3);
        assert_eq!(moved(3, 0, 0), 0);
    }

    #[test]
    fn results_play_songs_and_open_albums_and_artists() {
        let item = |id: &str, name: &str, kind: &str| {
            serde_json::from_value::<aoide_jellyfin::Item>(serde_json::json!({
                "Id": id, "Name": name, "Type": kind, "Artists": ["Mara Vey"]
            }))
            .unwrap()
        };
        let found = SearchResults {
            songs: vec![item("t1", "Harbour Lights", "Audio")],
            albums: vec![item("a1", "Saltwater", "MusicAlbum")],
            artists: vec![item("ar1", "Mara Vey", "MusicArtist")],
        };
        let entries = results(&found);
        assert_eq!(
            labels(&entries),
            ["Harbour Lights", "Saltwater", "Mara Vey"]
        );
        assert!(matches!(&entries[0].action, AppMessage::Play(r) if r.tracks[0].id == "t1"));
        assert_eq!(entries[0].detail.as_deref(), Some("Mara Vey"));
        assert!(
            matches!(&entries[1].action, AppMessage::Open(Route::Album { id, .. }) if id == "a1")
        );
        assert!(
            matches!(&entries[2].action, AppMessage::Open(Route::Artist { id, .. }) if id == "ar1")
        );
    }

    #[test]
    fn the_keys_are_ctrl_k_the_arrows_escape_and_tab() {
        let press = |key: Key, modifiers: keyboard::Modifiers| {
            iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key,
                physical_key: keyboard::key::Physical::Unidentified(
                    keyboard::key::NativeCode::Unidentified,
                ),
                location: keyboard::Location::Standard,
                modifiers,
                text: None,
                repeat: false,
            })
        };
        let status = iced::event::Status::Ignored;
        let window = iced::window::Id::unique();
        let command = keyboard::Modifiers::COMMAND;
        assert!(matches!(
            key(press(Key::Character("k".into()), command), status, window),
            Some(Message::Toggle)
        ));
        assert!(
            key(
                press(Key::Character("k".into()), keyboard::Modifiers::empty()),
                status,
                window
            )
            .is_none()
        );
        assert!(matches!(
            key(
                press(Key::Named(Named::Escape), keyboard::Modifiers::empty()),
                iced::event::Status::Captured,
                window
            ),
            Some(Message::Close)
        ));
        assert!(matches!(
            key(
                press(Key::Named(Named::ArrowDown), keyboard::Modifiers::empty()),
                status,
                window
            ),
            Some(Message::Move(1))
        ));
        assert!(matches!(
            key(
                press(Key::Named(Named::Tab), keyboard::Modifiers::empty()),
                status,
                window
            ),
            Some(Message::Tab(true))
        ));
        assert!(matches!(
            key(
                press(Key::Named(Named::Tab), keyboard::Modifiers::SHIFT),
                status,
                window
            ),
            Some(Message::Tab(false))
        ));
    }
}
