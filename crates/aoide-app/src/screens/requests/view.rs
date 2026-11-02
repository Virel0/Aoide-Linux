//! The Requests page as drawn: each request and where it stands, then the
//! playlists asked for, each with its songs when opened.

use aoide_melete::{MusicRequest, PlaylistImport, PlaylistImportSummary, RequestState};
use iced::widget::{Column, button, column, container, row, text};
use iced::{Center, Element, Fill};

use super::words::{detail, import_tally, kind_word, standing, track_standing};
use super::{Message, State};
use crate::app::Message as App;
use crate::screens::{Load, PageMsg, View};
use crate::{theme, widgets};

fn small_button(label: &str, message: Option<App>) -> Element<'_, App> {
    button(text(label).size(12))
        .style(theme::tonal)
        .padding([4, 10])
        .on_press_maybe(message)
        .into()
}

impl State {
    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let refresh = button(text("Refresh").size(13))
            .style(theme::tonal)
            .padding([6, 12])
            .on_press(App::Page(key, PageMsg::Requests(Message::Refresh)));
        let mut body = column![
            widgets::title_bar("Requests", view.can_go_back),
            row![
                widgets::secondary(
                    "Music you've asked your server for, kept up to date while this page is \
                     open. Ask for more from Search, or for a playlist's missing songs from Import."
                )
                .size(13)
                .width(Fill),
                refresh,
            ]
            .align_y(Center),
        ]
        .spacing(16);
        if let Some(problem) = &self.problem {
            body = body.push(widgets::secondary(problem.as_str()).size(13));
        }
        let now = aoide_store::now_millis() / 1000;
        body = body.push(match &self.requests {
            Load::Loading => widgets::loading(),
            Load::Failed(error) => view.failure(error),
            Load::Ready(requests) if requests.is_empty() => widgets::empty(
                "Nothing asked for yet. Search for an artist, album or song that isn't in your library, then ask for it.",
            ),
            Load::Ready(requests) => column(requests.iter().map(|r| self.row(r, key, now)))
                .spacing(10)
                .into(),
        });
        if let Load::Ready(imports) = &self.imports
            && !imports.is_empty()
        {
            body = body
                .push(text("Playlists").size(17).font(widgets::BOLD))
                .push(column(imports.iter().map(|i| self.import(i, key))).spacing(12));
        }
        widgets::page(body)
    }

    /// One request: what it is, where it stands, and the finer points.
    fn row<'a>(&self, request: &'a MusicRequest, key: u64, now: i64) -> Element<'a, App> {
        let mut what = vec![kind_word(request.kind).to_owned()];
        if let Some(artist) = &request.artist {
            what.push(artist.clone());
        }
        if let Some(year) = request.year {
            what.push(year.to_string());
        }
        let mut about = column![
            text(request.title.as_str())
                .size(14)
                .font(widgets::SEMIBOLD),
            widgets::secondary(what.join(" \u{b7} ")).size(12),
            text(standing(request)).size(12).style(|t| text::Style {
                color: Some(theme::accent(t)),
            }),
        ]
        .spacing(2)
        .width(Fill);
        if let Some(detail) = detail(request, now) {
            about = about.push(widgets::secondary(detail).size(11));
        }
        let mut line = row![about].align_y(Center);
        if request.state == RequestState::Requested {
            let busy = self.withdrawing == Some(request.id);
            line = line.push(small_button(
                if busy { "Withdrawing…" } else { "Withdraw" },
                (!busy).then_some(App::Page(
                    key,
                    PageMsg::Requests(Message::Withdraw(request.id)),
                )),
            ));
        }
        line.into()
    }

    /// One playlist asked for: its tally, Songs and Remove, and its songs when open.
    fn import<'a>(&'a self, import: &'a PlaylistImportSummary, key: u64) -> Element<'a, App> {
        let message = move |m: Message| Some(App::Page(key, PageMsg::Requests(m)));
        let opened = self
            .open_import
            .as_ref()
            .filter(|(id, _)| *id == import.id)
            .map(|(_, songs)| songs);
        let mut tools = row![small_button(
            if opened.is_some() {
                "Hide songs"
            } else {
                "Songs"
            },
            message(Message::ShowImport(import.id)),
        )]
        .spacing(6);
        if self.removing == Some(import.id) {
            tools = tools.push(small_button("Removing…", None));
        } else if self.confirm_remove != Some(import.id) {
            tools = tools.push(small_button("Remove", message(Message::Remove(import.id))));
        }
        let mut block = column![
            row![
                column![
                    text(import.name.as_str()).size(14).font(widgets::SEMIBOLD),
                    widgets::secondary(import_tally(import)).size(12),
                ]
                .spacing(2)
                .width(Fill),
                tools,
            ]
            .spacing(8)
            .align_y(Center)
        ]
        .spacing(8);
        if self.confirm_remove == Some(import.id) {
            block = block.push(
                row![
                    widgets::secondary(
                        "Remove it? Its songs still waiting for approval are taken back; \
                         approved ones still come."
                    )
                    .size(12)
                    .width(Fill),
                    small_button("Keep", message(Message::KeepImport)),
                    small_button("Remove", message(Message::ConfirmRemove(import.id))),
                ]
                .spacing(6)
                .align_y(Center),
            );
        }
        if let Some(songs) = opened {
            block = block.push(match songs {
                Load::Loading => widgets::secondary("Reading its songs…").size(12).into(),
                Load::Failed(error) => widgets::secondary(error.to_string()).size(12).into(),
                Load::Ready(read) => songs_list(read),
            });
        }
        block.into()
    }
}

/// An import's songs, each with what became of it.
fn songs_list(import: &PlaylistImport) -> Element<'_, App> {
    let mut list = Column::new().spacing(4);
    for track in &import.items {
        let name = match &track.artist {
            Some(artist) => format!("{}. {} \u{2014} {artist}", track.position, track.title),
            None => format!("{}. {}", track.position, track.title),
        };
        list = list.push(
            row![
                text(name).size(13).width(Fill),
                widgets::secondary(track_standing(track)).size(12),
            ]
            .spacing(8),
        );
    }
    container(list)
        .padding(10)
        .width(Fill)
        .style(theme::menu)
        .into()
}
