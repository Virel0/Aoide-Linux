//! Listen Together at the top of the queue panel: start a session with this
//! queue or join one by its code; once in one, its code, who's in it, and
//! Leave (and End for whoever started it).

use iced::widget::{button, column, row, text, text_input};
use iced::{Center, Element, Fill};

use super::{Msg, Together};
use crate::app::Message;
use crate::{theme, widgets};

fn send(msg: Msg) -> Message {
    Message::Together(msg)
}

pub fn view(together: &Together) -> Element<'_, Message> {
    let body = match &together.session {
        None => outside(together),
        Some(session) => {
            let me = session.members.iter().find(|m| m.you);
            let hosting = me.is_some_and(|m| m.username == session.host);
            let others: Vec<String> = session
                .members
                .iter()
                .filter(|m| !m.you)
                .map(|m| format!("{} on {}", m.username, m.device))
                .collect();
            let who = if others.is_empty() {
                "Nobody else yet. Share the code.".to_owned()
            } else {
                format!("With {}.", others.join(", "))
            };
            let mut actions = row![
                button(text("Leave").size(12))
                    .style(theme::tonal)
                    .padding([4, 10])
                    .on_press_maybe((!together.busy).then(|| send(Msg::Leave))),
            ]
            .spacing(6);
            if hosting {
                actions = actions.push(
                    button(text("End for Everyone").size(12))
                        .style(theme::tonal)
                        .padding([4, 10])
                        .on_press_maybe((!together.busy).then(|| send(Msg::End))),
                );
            }
            column![
                row![
                    text("Listening together")
                        .size(13)
                        .font(widgets::SEMIBOLD)
                        .width(Fill),
                    text(session.code.as_str()).size(15).font(widgets::BOLD),
                ]
                .align_y(Center),
                widgets::secondary(who).size(11),
                actions,
            ]
            .spacing(6)
            .into()
        }
    };
    let mut strip = column![body].spacing(4);
    if let Some(problem) = &together.problem {
        strip = strip.push(widgets::secondary(problem.as_str()).size(11));
    }
    strip.padding(iced::Padding::ZERO.bottom(8)).into()
}

/// Start, or a code to join with.
fn outside(together: &Together) -> Element<'_, Message> {
    let joinable = !together.busy && !together.code.trim().is_empty();
    column![
        row![
            text("Listen Together")
                .size(13)
                .font(widgets::SEMIBOLD)
                .width(Fill),
            button(text("Start").size(12))
                .style(theme::tonal)
                .padding([4, 10])
                .on_press_maybe((!together.busy).then(|| send(Msg::Start))),
        ]
        .align_y(Center),
        row![
            text_input("Code", &together.code)
                .on_input(|code| send(Msg::Code(code)))
                .on_submit_maybe(joinable.then(|| send(Msg::Join)))
                .size(13)
                .padding(6)
                .style(theme::field),
            button(text("Join").size(12))
                .style(theme::tonal)
                .padding([4, 10])
                .on_press_maybe(joinable.then(|| send(Msg::Join))),
        ]
        .spacing(6)
        .align_y(Center),
    ]
    .spacing(6)
    .into()
}
