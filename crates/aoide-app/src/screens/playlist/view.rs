//! The playlist page as drawn: the cover and its tools, the title or the
//! rename field, the notes, and the songs (`songs`).

use aoide_store::PlaySource;
use iced::widget::{Column, Row, button, column, row, text, text_editor, text_input};
use iced::{Center, Element, Length};

use super::{Message, State};
use crate::app::Message as App;
use crate::playback::tracks::TrackMeta;
use crate::screens::{Load, PageMsg, View};
use crate::theme;
use crate::widgets;

impl State {
    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let cover = widgets::cover(self.cover.as_ref(), view.artwork, 180.0, 10.0);
        let header = row![
            widgets::lifted(cover, 10.0, view),
            self.head(view).width(Length::Fill)
        ]
        .spacing(24);
        let mut body = column![].spacing(16);
        if let Some(menu) = view.menu.filter(|m| m.anchor == format!("{key}:playlist")) {
            body = body.push(widgets::menu_strip(menu, view));
        }
        if let Some(error) = &self.error {
            body = body.push(widgets::plain_failure(error));
        }
        if self.smart.is_some() {
            body = body.push(
                widgets::secondary(
                    "A smart playlist: its songs are whatever its rules match in your listening, \
                 worked out each time it opens, so they can't be moved or removed one by one.",
                )
                .size(13),
            );
        }
        if self.missing > 0 {
            body = body.push(widgets::secondary(format!(
                "{} {} no longer on the server.",
                self.missing,
                if self.missing == 1 {
                    "song is"
                } else {
                    "songs are"
                }
            )));
        }
        body = body.push(match &self.tracks {
            Load::Loading => widgets::loading(),
            Load::Failed(error) => view.failure(error),
            Load::Ready(tracks) if tracks.is_empty() && self.smart.is_some() => widgets::empty(
                "Nothing matches its rules yet. Songs appear as your listening fits them.",
            ),
            Load::Ready(_) if self.lines.is_empty() => {
                widgets::empty("No songs yet. Add some from any song's … menu.")
            }
            Load::Ready(tracks) => self.songs(tracks, view),
        });
        let page = widgets::page_with_header(header, body, view);
        crate::handoff::played_from(page, self.context())
    }

    fn name(&self) -> &str {
        self.playlist
            .as_ref()
            .map_or("Playlist", |p| p.name.as_str())
    }

    /// Plays from a smart playlist are filed as smart, as on the phone.
    pub(super) fn source(&self) -> PlaySource {
        if self.smart.is_some() {
            PlaySource::Smart
        } else {
            PlaySource::Playlist
        }
    }

    /// Beside the cover: the title, the notes, the buttons, and what the last
    /// cover change said.
    fn head<'a>(&'a self, view: &View<'a>) -> Column<'a, App> {
        let key = view.key;
        let message = move |m: Message| App::Page(key, PageMsg::Playlist(m));
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
            None => widgets::title_bar(self.name(), view.can_go_back),
        };
        let mut head = column![title].spacing(12);
        if self.editing {
            head = head.push(
                text_editor(&self.notes)
                    .placeholder("Notes")
                    .on_action(move |a| message(Message::Notes(a)))
                    .height(Length::Shrink)
                    .padding(8),
            );
        } else if let Some(notes) = self.playlist.as_ref().and_then(|p| p.notes.as_deref()) {
            head = head.push(widgets::secondary(notes));
        }
        // Wrapped: at the narrowest window the buttons don't fit on one line.
        head = head.push(self.tools(key).wrap().vertical_spacing(10));
        if self.confirm_delete {
            head = head.push(self.confirm(key));
        }
        head = head.push(self.cover_tools(key));
        if let Some(note) = &self.cover_note {
            head = head.push(match note {
                Ok(note) => widgets::secondary(note.as_str()).size(13).into(),
                Err(error) => widgets::plain_failure(error),
            });
        }
        head
    }

    /// Play, Shuffle and the menu; Rename, Edit (or Done) and Delete.
    fn tools<'a>(&'a self, key: u64) -> Row<'a, App> {
        let message = move |m: Message| App::Page(key, PageMsg::Playlist(m));
        let tracks: &[TrackMeta] = self.tracks.ready().map(Vec::as_slice).unwrap_or(&[]);
        let mut tools = row![widgets::play_buttons(
            tracks,
            format!("{key}:playlist"),
            self.source()
        )]
        .spacing(10)
        .align_y(Center);
        if self.renaming.is_none() {
            tools = tools.push(
                button(text("Rename"))
                    .style(theme::tonal)
                    .padding([8, 14])
                    .on_press(message(Message::Rename)),
            );
        }
        tools = tools.push(if self.editing {
            button(text("Done"))
                .style(theme::filled)
                .padding([8, 14])
                .on_press(message(Message::Edit(false)))
        } else {
            button(text("Edit"))
                .style(theme::tonal)
                .padding([8, 14])
                .on_press(message(Message::Edit(true)))
        });
        tools.push(
            button(text("Delete"))
                .style(theme::destructive)
                .padding([8, 14])
                .on_press_maybe((!self.confirm_delete).then(|| message(Message::Delete))),
        )
    }

    /// On a line of its own: beside the other buttons the question runs off
    /// a narrow window.
    fn confirm<'a>(&'a self, key: u64) -> Element<'a, App> {
        let message = move |m: Message| App::Page(key, PageMsg::Playlist(m));
        let name = self.name();
        row![
            text(format!(
                "Delete \u{201c}{name}\u{201d}? Its songs stay in your library."
            )),
            button("Delete")
                .style(theme::destructive)
                .padding([8, 14])
                .on_press(message(Message::ConfirmDelete(true))),
            button("Cancel")
                .style(theme::tonal)
                .padding([8, 14])
                .on_press(message(Message::ConfirmDelete(false))),
        ]
        .spacing(8)
        .align_y(Center)
        .wrap()
        .vertical_spacing(8)
        .into()
    }

    /// Choose Cover, Remove Cover and Share.
    fn cover_tools<'a>(&'a self, key: u64) -> Row<'a, App> {
        let message = move |m: Message| App::Page(key, PageMsg::Playlist(m));
        let small = |label: &'a str, m: Option<Message>| {
            button(text(label).size(13))
                .style(theme::tonal)
                .padding([6, 12])
                .on_press_maybe(m.map(message))
        };
        let has_picture = self
            .playlist
            .as_ref()
            .is_some_and(|p| p.image_hash.is_some());
        let share = button(text("Share…").size(13))
            .style(theme::tonal)
            .padding([6, 12])
            .on_press_maybe(self.playlist.as_ref().map(|p| {
                App::Open(crate::screens::Route::Share {
                    id: p.id.clone(),
                    name: p.name.clone(),
                })
            }));
        row![
            small("Choose cover…", Some(Message::ChooseCover)),
            small("Remove cover", has_picture.then_some(Message::RemoveCover)),
            share,
        ]
        .spacing(8)
    }

    /// What playing from this page is remembered as: the playlist, drawn by
    /// its first track's cover.
    fn context(&self) -> Option<aoide_store::RecentContext> {
        let playlist = self.playlist.as_ref()?;
        let mut context = aoide_store::RecentContext::new(
            aoide_store::RecentContextKind::LocalPlaylist,
            playlist.id.clone(),
            playlist.name.clone(),
            aoide_store::now_millis(),
        );
        context.smart = Some(playlist.is_smart);
        context.artwork_item_id = playlist.artwork_item_id.clone().or_else(|| {
            self.tracks
                .ready()
                .and_then(|t| t.first())
                .and_then(|t| t.album_id.clone().or_else(|| Some(t.id.clone())))
        });
        Some(context)
    }
}
