//! A collection's page as drawn: its name, what it filters, Delete (asked
//! once more), Play and Shuffle, and its songs.

use aoide_store::PlaySource;
use iced::widget::{button, column, row, text};
use iced::{Center, Element};

use super::{Message, State, words};
use crate::app::Message as AppMessage;
use crate::screens::{Load, PageMsg, View};
use crate::{theme, widgets};

impl State {
    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, AppMessage> {
        let key = view.key;
        let title = self
            .collection
            .as_ref()
            .map_or("Collection", |c| c.name.as_str());
        let mut body = column![widgets::title_bar(title, view.can_go_back)].spacing(14);
        if let Some(collection) = &self.collection {
            body = body
                .push(widgets::secondary(words::summary(collection)).size(13))
                .push(self.delete_row(key));
        }
        if let Some(why) = &self.delete_failed {
            body = body.push(widgets::secondary(why.as_str()).size(13));
        }
        let anchor = format!("{key}:web-collection");
        body = body.push(match &self.songs {
            Load::Loading => widgets::loading(),
            Load::Failed(error) => view.failure(error),
            Load::Ready(songs) if songs.is_empty() => {
                widgets::empty("No songs in your library match this collection.")
            }
            Load::Ready(songs) => {
                let mut list = column![widgets::play_buttons(
                    songs,
                    anchor.clone(),
                    PlaySource::Unknown
                )]
                .spacing(12);
                if let Some(menu) = view.menu.filter(|m| m.anchor == anchor) {
                    list = list.push(widgets::menu_strip(menu, view));
                }
                list = list.push(widgets::track_list(
                    songs,
                    view,
                    widgets::ListStyle {
                        numbered: false,
                        show_album: true,
                    },
                    PlaySource::Unknown,
                ));
                if let Some(more) = words::more(songs.len(), self.total) {
                    list = list.push(widgets::secondary(more).size(13));
                }
                list.into()
            }
        });
        widgets::page(body)
    }

    /// Delete, or the question it asks first.
    fn delete_row(&self, key: u64) -> Element<'_, AppMessage> {
        let message = move |m: Message| AppMessage::Page(key, PageMsg::WebCollection(m));
        if !self.confirm_delete {
            return button(text("Delete collection").size(13))
                .style(theme::tonal)
                .padding([6, 12])
                .on_press(message(Message::Delete))
                .into();
        }
        row![
            text("Delete this collection from your server? It goes from Melete's web too.")
                .size(13),
            button(
                text(if self.deleting {
                    "Deleting…"
                } else {
                    "Delete"
                })
                .size(13)
            )
            .style(theme::destructive)
            .padding([6, 12])
            .on_press_maybe((!self.deleting).then(|| message(Message::ConfirmDelete(true)))),
            button(text("Cancel").size(13))
                .style(theme::tonal)
                .padding([6, 12])
                .on_press(message(Message::ConfirmDelete(false))),
        ]
        .spacing(8)
        .align_y(Center)
        .wrap()
        .into()
    }
}
