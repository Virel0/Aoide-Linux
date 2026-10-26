//! The import page as drawn: the link and the CSV button, the progress,
//! and what was matched.

use aoide_sidecar::{ImportMatch, ImportResult, ImportedPlaylist};
use iced::widget::{Column, Row, button, column, container, row, text, text_input};
use iced::{Center, Element, Fill};

use super::{Message, State, Step, line, summary};
use crate::app::Message as App;
use crate::screens::{PageMsg, View};
use crate::{theme, widgets};

impl State {
    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let mut body = column![
            widgets::title_bar("Import a playlist", view.can_go_back),
            widgets::secondary(
                "Bring a playlist from Spotify or another service. Aoide looks for each song \
                 in your library and saves the ones it finds as an Aoide playlist."
            )
            .size(13),
        ]
        .spacing(16);
        match &self.step {
            Step::Start => body = start(body, &self.link, key),
            Step::Reading => body = body.push(widgets::secondary("Reading the playlist…")),
            Step::Matching { done, total } => {
                body = body.push(widgets::secondary(format!(
                    "Looking for the songs in your library: {done} of {total}…"
                )));
            }
            Step::Saving => body = body.push(widgets::secondary("Saving…")),
            Step::Matched(found) => body = body.push(self.matched(&found.0, &found.1, view)),
        }
        if let Some(error) = &self.error {
            body = body.push(widgets::plain_failure(error));
        }
        if let Some(note) = &self.note {
            body = body.push(widgets::secondary(note.as_str()).size(13));
        }
        widgets::page(body.max_width(860))
    }

    fn matched<'a>(
        &'a self,
        playlist: &'a ImportedPlaylist,
        result: &'a ImportResult,
        view: &View<'a>,
    ) -> Element<'a, App> {
        let key = view.key;
        let message = move |m: Message| App::Page(key, PageMsg::Import(m));
        let found = result.found_ids().len();
        let ready = found > 0 && !self.name.trim().is_empty();
        let mut column = column![text(summary(result)).size(16)].spacing(14);
        if playlist.is_truncated {
            column = column.push(
                widgets::secondary(
                    "Spotify only shares the first 100 songs through a link. Export the \
                     playlist to CSV for all of it.",
                )
                .size(13),
            );
        }
        column = column.push(
            row![
                text_input("Playlist name", &self.name)
                    .on_input(move |s| message(Message::Name(s)))
                    .padding(8)
                    .style(theme::field),
                button(text(match found {
                    1 => "Save 1 song".to_owned(),
                    n => format!("Save {n} songs"),
                }))
                .style(theme::filled)
                .padding([8, 14])
                .on_press_maybe(ready.then(|| message(Message::Save))),
            ]
            .spacing(10)
            .align_y(Center),
        );
        column = column.push(tools(result, key));
        let (have, lack): (Vec<&ImportMatch>, Vec<&ImportMatch>) =
            result.matches.iter().partition(|m| m.found.is_some());
        if !lack.is_empty() {
            column = column
                .push(
                    text(format!("Not in your library ({})", lack.len()))
                        .size(17)
                        .font(widgets::BOLD),
                )
                .push(match_rows(lack, false));
        }
        if !have.is_empty() {
            column = column
                .push(
                    text(format!("Found ({})", have.len()))
                        .size(17)
                        .font(widgets::BOLD),
                )
                .push(match_rows(have, true));
        }
        column.into()
    }
}

/// Start Over, and Copy Missing Songs when some are.
fn tools(result: &ImportResult, key: u64) -> Row<'_, App> {
    let message = move |m: Message| App::Page(key, PageMsg::Import(m));
    let mut tools = row![
        button(text("Start over").size(13))
            .style(theme::tonal)
            .padding([6, 12])
            .on_press(message(Message::StartOver)),
    ]
    .spacing(8);
    if !result.missing().is_empty() {
        tools = tools.push(
            button(text("Copy missing songs").size(13))
                .style(theme::tonal)
                .padding([6, 12])
                .on_press(message(Message::CopyMissing)),
        );
    }
    tools
}

/// The imported songs, each with the library's song it matched when `found`.
fn match_rows(matches: Vec<&ImportMatch>, found: bool) -> Element<'_, App> {
    let mut list = Column::new().spacing(4);
    for m in matches {
        let mut line = column![text(line(&m.track)).size(14)];
        if found && let Some(chosen) = &m.found {
            let artist = chosen
                .artists
                .first()
                .map(String::as_str)
                .unwrap_or_default();
            line = line.push(
                widgets::secondary(format!(
                    "In your library: {} \u{2014} {artist}",
                    chosen.title.as_deref().unwrap_or("this song")
                ))
                .size(12),
            );
        }
        list = list.push(container(line).padding([4, 8]));
    }
    container(list)
        .padding(10)
        .width(Fill)
        .style(theme::menu)
        .into()
}

/// Waiting for a link or a file: the link field and Read, the CSV button,
/// and what each gives.
fn start<'a>(body: Column<'a, App>, link: &'a str, key: u64) -> Column<'a, App> {
    let message = move |m: Message| App::Page(key, PageMsg::Import(m));
    let ready = !link.trim().is_empty();
    body.push(
        row![
            text_input("Paste a Spotify playlist link", link)
                .on_input(move |s| message(Message::Link(s)))
                .on_submit_maybe(ready.then(|| message(Message::ReadLink)))
                .padding(8)
                .style(theme::field),
            button(text("Read"))
                .style(theme::filled)
                .padding([8, 14])
                .on_press_maybe(ready.then(|| message(Message::ReadLink))),
        ]
        .spacing(10)
        .align_y(Center),
    )
    .push(
        row![
            widgets::secondary("Or a CSV export (Exportify and similar):").size(13),
            button(text("Choose a CSV file…").size(13))
                .style(theme::tonal)
                .padding([6, 12])
                .on_press(message(Message::ChooseCsv)),
        ]
        .spacing(10)
        .align_y(Center),
    )
    .push(
        widgets::secondary(
            "A Spotify link gives the first 100 songs; export the playlist to \
             CSV for all of it. Only the link's public page is read: no account.",
        )
        .size(12),
    )
}
