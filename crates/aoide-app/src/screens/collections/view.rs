//! A collection's page as drawn: Songs or Albums, the filter's chips, Only
//! Favourites, the name and Save (or Delete), and what the filter finds.

use aoide_jellyfin::QueryFilters;
use aoide_store::PlaySource;
use iced::widget::{Column, Row, button, column, row, text, text_input};
use iced::{Center, Element};

use super::{Kind, Message, State};
use crate::app::Message as AppMessage;
use crate::screens::{Load, PageMsg, View};
use crate::{theme, widgets};

impl State {
    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, AppMessage> {
        let key = view.key;
        let message = move |m: Message| AppMessage::Page(key, PageMsg::Collections(m));
        let title = if self.saved {
            self.filter.name.as_str()
        } else {
            "New collection"
        };
        let kinds = row![
            chip(
                key,
                "Songs".into(),
                self.filter.kind == Kind::Songs,
                Message::Kind(Kind::Songs)
            ),
            chip(
                key,
                "Albums".into(),
                self.filter.kind == Kind::Albums,
                Message::Kind(Kind::Albums)
            ),
        ]
        .spacing(8);
        let mut body = column![
            widgets::title_bar(title, view.can_go_back),
            widgets::secondary(self.filter.summary()).size(13),
            kinds,
        ]
        .spacing(14);
        match &self.filters {
            Load::Loading => body = body.push(widgets::loading()),
            Load::Failed(error) => body = body.push(view.failure(error)),
            Load::Ready(filters) => body = self.filter_groups(body, filters, key),
        }
        body = body.push(
            widgets::switch("Only favourites", self.filter.favourites_only, move |on| {
                message(Message::Favourites(on))
            })
            .width(iced::Length::Fixed(260.0)),
        );
        body = body.push(self.save_row(key));
        body = body.push(self.results(view));
        if let Some(menu) = view
            .menu
            .filter(|m| m.anchor == format!("{key}:collection"))
        {
            body = body.push(widgets::menu_strip(menu, view));
        }
        widgets::page(body)
    }

    /// The genres, years and tags the library has, each a chip to pick.
    fn filter_groups<'a>(
        &'a self,
        mut body: Column<'a, AppMessage>,
        filters: &'a QueryFilters,
        key: u64,
    ) -> Column<'a, AppMessage> {
        let group = |label: &'a str, chips: Vec<Element<'a, AppMessage>>| {
            column![
                widgets::secondary(label).size(12).font(widgets::SEMIBOLD),
                row(chips).spacing(6).wrap().vertical_spacing(6),
            ]
            .spacing(6)
        };
        if !filters.genres.is_empty() {
            body = body.push(group(
                "Genres",
                filters
                    .genres
                    .iter()
                    .map(|g| {
                        chip(
                            key,
                            g.name.clone(),
                            self.filter.genre_ids.contains(&g.id),
                            Message::Genre(g.id.clone(), g.name.clone()),
                        )
                    })
                    .collect(),
            ));
        }
        if !filters.years.is_empty() {
            body = body.push(group(
                "Years",
                filters
                    .years
                    .iter()
                    .map(|y| {
                        chip(
                            key,
                            y.to_string(),
                            self.filter.years.contains(y),
                            Message::Year(*y),
                        )
                    })
                    .collect(),
            ));
        }
        if !filters.tags.is_empty() {
            body = body.push(group(
                "Tags",
                filters
                    .tags
                    .iter()
                    .map(|t| {
                        chip(
                            key,
                            t.clone(),
                            self.filter.tags.contains(t),
                            Message::Tag(t.clone()),
                        )
                    })
                    .collect(),
            ));
        }
        body
    }

    /// The name, Save, and (for a saved collection) Delete or its question.
    fn save_row(&self, key: u64) -> Row<'_, AppMessage> {
        let message = move |m: Message| AppMessage::Page(key, PageMsg::Collections(m));
        let ready = !self.name.trim().is_empty();
        let save = row![
            text_input("Collection name", &self.name)
                .on_input(move |s| message(Message::Name(s)))
                .on_submit_maybe(ready.then(|| message(Message::Save)))
                .padding(8)
                .style(theme::field),
            button(text(if self.saved {
                "Save changes"
            } else {
                "Save as collection"
            }))
            .style(theme::filled)
            .padding([8, 14])
            .on_press_maybe(ready.then(|| message(Message::Save))),
        ]
        .spacing(10)
        .align_y(Center);
        if !self.saved {
            return save;
        }
        save.push(if self.confirm_delete {
            Element::from(
                row![
                    text("Delete this collection?").size(13),
                    button(text("Delete").size(13))
                        .style(theme::destructive)
                        .padding([6, 12])
                        .on_press(message(Message::ConfirmDelete(true))),
                    button(text("Cancel").size(13))
                        .style(theme::tonal)
                        .padding([6, 12])
                        .on_press(message(Message::ConfirmDelete(false))),
                ]
                .spacing(8)
                .align_y(Center),
            )
        } else {
            button(text("Delete"))
                .style(theme::destructive)
                .padding([8, 14])
                .on_press(message(Message::Delete))
                .into()
        })
    }

    /// What the filter finds: songs with Play and Shuffle, or album tiles.
    fn results<'a>(&'a self, view: &View<'a>) -> Element<'a, AppMessage> {
        let key = view.key;
        match self.filter.kind {
            Kind::Songs => match &self.songs {
                Load::Loading => widgets::loading(),
                Load::Failed(error) => view.failure(error),
                Load::Ready(songs) if songs.is_empty() => {
                    widgets::empty("Nothing matches this filter.")
                }
                Load::Ready(songs) => column![
                    widgets::play_buttons(songs, format!("{key}:collection"), PlaySource::Unknown),
                    widgets::track_list(
                        songs,
                        view,
                        widgets::ListStyle {
                            numbered: false,
                            show_album: true,
                        },
                        PlaySource::Unknown,
                    ),
                ]
                .spacing(12)
                .into(),
            },
            Kind::Albums => match &self.albums {
                Load::Loading => widgets::loading(),
                Load::Failed(error) => view.failure(error),
                Load::Ready(tiles) if tiles.is_empty() => {
                    widgets::empty("Nothing matches this filter.")
                }
                Load::Ready(tiles) => {
                    widgets::tile_grid(tiles, view.artwork, crate::screens::home::open_album)
                }
            },
        }
    }
}

/// A filter's chip: filled while picked.
fn chip<'a>(key: u64, label: String, on: bool, m: Message) -> Element<'a, AppMessage> {
    button(text(label).size(13))
        .style(if on { theme::filled } else { theme::tonal })
        .padding([5, 12])
        .on_press(AppMessage::Page(key, PageMsg::Collections(m)))
        .into()
}
