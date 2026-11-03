//! The playlist's songs as drawn, in its order: each track a row that plays
//! the list from it, each song not in the library yet greyed in its place with
//! where it stands. While editing, a track has Up, Down and Remove, and a
//! waiting song Remove.

use aoide_store::PendingSong;
use iced::widget::{Column, Row, button, column, container, row, space};
use iced::{Center, Element, Fill, Length};

use super::waiting::{self, Line};
use super::{LIST, Message, State};
use crate::actions::PlayRequest;
use crate::app::Message as App;
use crate::ellipsis::ellipsis;
use crate::icons::{Icon, Tone, icon};
use crate::playback::tracks::TrackMeta;
use crate::screens::{PageMsg, View};
use crate::{theme, widgets};

impl State {
    pub(super) fn songs<'a>(
        &'a self,
        tracks: &'a [TrackMeta],
        view: &View<'a>,
    ) -> Element<'a, App> {
        let editing = self.editing && self.smart.is_none();
        let mut list = Column::with_capacity(self.lines.len()).spacing(2);
        for line in &self.lines {
            list = match *line {
                Line::Track(index) => self.track_line(list, tracks, index, view, editing),
                Line::Waiting(index) => match self.waiting.get(index) {
                    Some((entry, song)) => list.push(self.waiting_line(entry, song, view, editing)),
                    None => list,
                },
            };
        }
        list.into()
    }

    /// A track's row, its edit buttons, and its action strip when open.
    fn track_line<'a>(
        &'a self,
        list: Column<'a, App>,
        tracks: &'a [TrackMeta],
        index: usize,
        view: &View<'a>,
        editing: bool,
    ) -> Column<'a, App> {
        let Some(track) = tracks.get(index) else {
            return list;
        };
        let anchor = format!("{}:{index}", view.key);
        let source = self.source();
        let play = move || App::Play(PlayRequest::from(tracks, index, source));
        let song = widgets::track_row(track, &anchor, view, LIST, play);
        let list = match self.entries.get(index).filter(|_| editing) {
            Some(entry) => list.push(self.edit_buttons(song, entry, index, tracks.len(), view.key)),
            None => list.push(song),
        };
        match view.menu.filter(|m| m.anchor == anchor) {
            Some(menu) => list.push(widgets::menu_strip(menu, view)),
            None => list,
        }
    }

    /// Up, Down and Remove beside a track's row.
    fn edit_buttons<'a>(
        &self,
        song: Element<'a, App>,
        entry: &str,
        index: usize,
        count: usize,
        key: u64,
    ) -> Row<'a, App> {
        let message = move |m: Message| App::Page(key, PageMsg::Playlist(m));
        let step = |up: bool| {
            message(Message::Move {
                entry: entry.to_owned(),
                up,
            })
        };
        row![
            song,
            small(Icon::Up, (index > 0).then(|| step(true))),
            small(Icon::Down, (index + 1 < count).then(|| step(false))),
            small(
                Icon::Remove,
                Some(message(Message::Remove(entry.to_owned())))
            ),
        ]
        .spacing(2)
        .align_y(Center)
    }

    /// A song not in the library yet: greyed, not playable, with its artist
    /// and where it stands.
    fn waiting_line<'a>(
        &self,
        entry: &str,
        song: &'a PendingSong,
        view: &View<'a>,
        editing: bool,
    ) -> Element<'a, App> {
        let quiet = |t: &iced::Theme| Some(theme::secondary(t));
        let status = waiting::status(song, &self.requests);
        let under = match &song.artist {
            Some(artist) => format!("{artist} \u{b7} {status}"),
            None => status,
        };
        let lines = column![
            ellipsis(song.title.as_str()).size(14).color(quiet),
            ellipsis(under).size(12).color(quiet),
        ]
        .spacing(2)
        .clip(true);
        let body = row![
            container(icon(Icon::Song, 16.0, Tone::Secondary))
                .width(40)
                .height(40)
                .center(40)
                .style(theme::placeholder),
            container(lines)
                .width(Fill)
                .height(Length::Fixed(34.0))
                .align_y(Center),
        ]
        .spacing(12)
        .align_y(Center);
        let line = container(body).padding([4, 8]).width(Fill);
        if !editing {
            return line.into();
        }
        let key = view.key;
        let remove = App::Page(key, PageMsg::Playlist(Message::Remove(entry.to_owned())));
        // Where a track's Up and Down are, so Remove lines up under theirs.
        row![line, space().width(58), small(Icon::Remove, Some(remove))]
            .spacing(2)
            .align_y(Center)
            .into()
    }
}

/// An edit button: a quiet glyph, off without a message.
fn small<'a>(glyph: Icon, message: Option<App>) -> Element<'a, App> {
    button(icon(glyph, 16.0, Tone::Secondary))
        .padding(6)
        .style(theme::row)
        .on_press_maybe(message)
        .into()
}
