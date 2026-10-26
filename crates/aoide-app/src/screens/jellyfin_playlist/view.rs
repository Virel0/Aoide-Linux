//! The Jellyfin playlist page as drawn: the cover, the title or the rename
//! field, the buttons, and the songs (with Up, Down and Remove while
//! editing).

use aoide_store::PlaySource;
use iced::widget::{Column, Row, button, column, row, text, text_input};
use iced::{Center, Element, Length};

use super::{LIST, Message, State};
use crate::actions::PlayRequest;
use crate::app::Message as App;
use crate::icons::{Icon, Tone, icon};
use crate::playback::tracks::TrackMeta;
use crate::screens::{Load, PageMsg, View};
use crate::{theme, widgets};

impl State {
    /// What playing from this page is remembered as.
    fn context(&self) -> aoide_store::RecentContext {
        aoide_store::RecentContext::new(
            aoide_store::RecentContextKind::JellyfinPlaylist,
            self.id.clone(),
            self.name.clone(),
            aoide_store::now_millis(),
        )
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let mut body = column![
            widgets::title_bar("", view.can_go_back),
            row![
                widgets::art(self.art.as_ref(), view.artwork, Length::Fixed(180.0), 10.0),
                self.info(key)
            ]
            .spacing(24),
        ]
        .spacing(16);
        if let Some(note) = &self.cover_note {
            body = body.push(widgets::secondary(note.as_str()).size(13));
        }
        if let Some(menu) = view
            .menu
            .filter(|m| m.anchor == format!("{key}:jellyfin-playlist"))
        {
            body = body.push(widgets::menu_strip(menu, view));
        }
        if let Some(error) = &self.error {
            body = body.push(view.problem(error));
        }
        body = body.push(match &self.tracks {
            Load::Loading => widgets::loading(),
            Load::Failed(error) => view.failure(error),
            Load::Ready(tracks) if tracks.is_empty() => {
                widgets::empty("No songs yet. Add some from any song's … menu.")
            }
            Load::Ready(tracks) if self.editing => self.editable(tracks, view),
            Load::Ready(tracks) => widgets::track_list(tracks, view, LIST, PlaySource::Playlist),
        });
        crate::handoff::played_from(widgets::page(body), Some(self.context()))
    }

    /// Beside the cover: the title (or the rename field), the count, Play,
    /// Shuffle and Start Radio, and the playlist's own buttons.
    fn info<'a>(&'a self, key: u64) -> Column<'a, App> {
        let message = move |m: Message| App::Page(key, PageMsg::JellyfinPlaylist(m));
        let tracks: &[TrackMeta] = self.tracks.ready().map(Vec::as_slice).unwrap_or(&[]);
        let title: Element<'a, App> = match &self.renaming {
            Some(draft) => row![
                text_input("Playlist name", draft)
                    .on_input(move |s| message(Message::RenameText(s)))
                    .on_submit(message(Message::SaveName))
                    .padding(8)
                    .size(20)
                    .style(theme::field),
                button("Save")
                    .style(theme::filled)
                    .on_press(message(Message::SaveName)),
            ]
            .spacing(10)
            .align_y(Center)
            .into(),
            None => text(self.name.as_str()).size(30).font(widgets::BOLD).into(),
        };
        let count = match tracks.len() {
            1 => "1 song".to_owned(),
            n => format!("{n} songs"),
        };
        let mut info = column![
            title,
            widgets::secondary(format!("Jellyfin playlist · {count}")).size(13),
            widgets::play_buttons_with(
                tracks,
                format!("{key}:jellyfin-playlist"),
                PlaySource::Playlist,
                vec![crate::radio::start_button(crate::radio::Station::new(
                    aoide_jellyfin::MixSeed::Playlist(self.id.clone()),
                    self.name.clone(),
                ))],
            ),
            self.tools(key).wrap().vertical_spacing(8),
        ]
        .spacing(10)
        .width(Length::Fill);
        // On a line of its own: a question beside five buttons runs off the page.
        if self.confirm_delete {
            info = info.push(
                row![
                    text("Delete this playlist from the server, for every app?").size(13),
                    button(text("Delete").size(13))
                        .style(theme::destructive)
                        .padding([6, 12])
                        .on_press(message(Message::ConfirmDelete(true))),
                    small(key, "Cancel", Some(Message::ConfirmDelete(false))),
                ]
                .spacing(8)
                .align_y(Center)
                .wrap()
                .vertical_spacing(8),
            );
        }
        info
    }

    /// Rename, Edit (or Done), the cover's two buttons and Delete.
    fn tools<'a>(&'a self, key: u64) -> Row<'a, App> {
        let message = move |m: Message| App::Page(key, PageMsg::JellyfinPlaylist(m));
        let mut tools = row![].spacing(8).align_y(Center);
        if self.renaming.is_none() {
            tools = tools.push(small(key, "Rename", Some(Message::Rename)));
        }
        tools = tools.push(if self.editing {
            small(key, "Done", Some(Message::Edit(false)))
        } else {
            small(key, "Edit", Some(Message::Edit(true)))
        });
        tools = tools
            .push(small(
                key,
                "Choose cover…",
                (!self.busy).then_some(Message::ChooseCover),
            ))
            .push(small(
                key,
                "Remove cover",
                (!self.busy && self.art.is_some()).then_some(Message::RemoveCover),
            ));
        tools.push(
            button(text("Delete").size(13))
                .style(theme::destructive)
                .padding([6, 12])
                .on_press_maybe((!self.confirm_delete).then(|| message(Message::Delete))),
        )
    }

    fn editable<'a>(&'a self, tracks: &'a [TrackMeta], view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let message = move |m: Message| App::Page(key, PageMsg::JellyfinPlaylist(m));
        let small = |glyph: Icon, m: Option<App>| {
            button(icon(glyph, 16.0, Tone::Secondary))
                .padding(6)
                .style(theme::row)
                .on_press_maybe(m)
        };
        let mut list = Column::with_capacity(tracks.len()).spacing(2);
        for (index, (track, entry)) in tracks.iter().zip(&self.entries).enumerate() {
            let anchor = format!("{key}:{index}");
            let play = move || App::Play(PlayRequest::from(tracks, index, PlaySource::Playlist));
            let up = (index > 0).then(|| {
                message(Message::Move {
                    entry: entry.clone(),
                    up: true,
                })
            });
            let down = (index + 1 < tracks.len()).then(|| {
                message(Message::Move {
                    entry: entry.clone(),
                    up: false,
                })
            });
            list = list.push(
                row![
                    widgets::track_row(track, &anchor, view, LIST, play),
                    small(Icon::Up, up),
                    small(Icon::Down, down),
                    small(Icon::Remove, Some(message(Message::Remove(entry.clone())))),
                ]
                .spacing(2)
                .align_y(Center),
            );
            if let Some(menu) = view.menu.filter(|m| m.anchor == anchor) {
                list = list.push(widgets::menu_strip(menu, view));
            }
        }
        list.into()
    }
}

/// One of the page's small buttons; off without a message.
fn small(key: u64, label: &str, m: Option<Message>) -> iced::widget::Button<'_, App> {
    button(text(label).size(13))
        .style(theme::tonal)
        .padding([6, 12])
        .on_press_maybe(m.map(|m| App::Page(key, PageMsg::JellyfinPlaylist(m))))
}
