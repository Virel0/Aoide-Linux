//! The sync panel as drawn: the overview's lines, Sync Now, Retry and
//! Repair Covers.

use aoide_sidecar::SyncOverview;
use iced::widget::{Column, Row, button, column, container, row, text};
use iced::{Center, Element, Fill};

use super::{Line, Msg, State, describe};
use crate::app::Message as AppMessage;
use crate::screens::{Load, View};
use crate::{theme, widgets};

impl State {
    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, AppMessage> {
        let key = view.key;
        let mut body = column![widgets::title_bar("Sync", view.can_go_back)].spacing(20);
        let snapshot = match &self.snapshot {
            Load::Loading => return widgets::page(body.push(widgets::loading())),
            Load::Failed(error) => return widgets::page(body.push(view.failure(error))),
            Load::Ready(None) => {
                return widgets::page(body.push(widgets::empty(
                    "This server has no Aoide sidecar; playlists and history stay on this computer.",
                )));
            }
            Load::Ready(Some(snapshot)) => snapshot,
        };
        let overview = &snapshot.overview;
        let lines = describe(overview, &snapshot.names, aoide_store::now_millis());
        body = body
            .push(
                container(line_list(lines))
                    .padding(16)
                    .width(Fill)
                    .style(theme::menu),
            )
            .push(actions(overview, key))
            .push(
                widgets::secondary(
                    "Repair covers gives each playlist that has no picture the cover of the \
                     one Jellyfin playlist with exactly its name, and leaves the rest alone.",
                )
                .size(12),
            );
        if let Some(note) = &self.note {
            body = body.push(widgets::secondary(note.as_str()).size(13));
        }
        widgets::page(body.max_width(720))
    }
}

/// Each line, a problem in the danger colour, its explanation under it.
fn line_list<'a>(lines: Vec<Line>) -> Column<'a, AppMessage> {
    let mut list = Column::new().spacing(14);
    for line in lines {
        let mut block = column![text(line.text).size(15).style(move |t: &iced::Theme| {
            text::Style {
                color: line.warning.then(|| t.extended_palette().danger.base.color),
            }
        })]
        .spacing(3);
        if let Some(detail) = line.detail {
            block = block.push(widgets::secondary(detail).size(13));
        }
        list = list.push(block);
    }
    list
}

/// Sync Now (Syncing… while it runs), Retry when some can be, and Repair
/// Covers.
fn actions<'a>(overview: &SyncOverview, key: u64) -> Row<'a, AppMessage> {
    let running = overview.running;
    let mut actions = row![
        button(text(if running { "Syncing…" } else { "Sync now" }))
            .style(theme::filled)
            .padding([8, 16])
            .on_press_maybe((!running).then_some(AppMessage::SyncPanel(Msg::SyncNow(key)))),
    ]
    .spacing(10)
    .align_y(Center);
    if overview.retryable > 0 {
        actions = actions.push(
            button(text(match overview.retryable {
                1 => "Retry 1 change".to_owned(),
                n => format!("Retry {n} changes"),
            }))
            .style(theme::tonal)
            .padding([8, 16])
            .on_press_maybe((!running).then_some(AppMessage::SyncPanel(Msg::Retry(key)))),
        );
    }
    actions.push(
        button(text("Repair covers"))
            .style(theme::tonal)
            .padding([8, 16])
            .on_press(AppMessage::Covers(crate::covers::Msg::Repair { key })),
    )
}
