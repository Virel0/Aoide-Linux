//! Listen Together at the top of the queue panel: start a session with this
//! queue or join one by its code; once in one, its code, who's in it (with
//! their pictures), and Leave (and End for whoever started it).

use aoide_melete::ListenSession;
use iced::widget::{Row, button, column, row, text, text_input};
use iced::{Center, Element, Fill};

use super::{Msg, Together};
use crate::app::Message;
use crate::artwork::Artwork;
use crate::{theme, widgets};

fn send(msg: Msg) -> Message {
    Message::Together(msg)
}

/// How big a member's picture is, beside the line naming them.
const FACE: f32 = 18.0;

pub fn view<'a>(together: &'a Together, artwork: &'a Artwork) -> Element<'a, Message> {
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
            let who: Element<'_, Message> = if others.is_empty() {
                widgets::secondary("Nobody else yet. Share the code.")
                    .size(11)
                    .into()
            } else {
                row![
                    faces(session, artwork),
                    widgets::secondary(format!("With {}.", others.join(", ")))
                        .size(11)
                        .width(Fill),
                ]
                .spacing(6)
                .align_y(Center)
                .into()
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
                who,
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

/// One picture (or initial) for each other person in the session, however
/// many of their devices are in it.
fn faces<'a>(session: &'a ListenSession, artwork: &'a Artwork) -> Row<'a, Message> {
    let mut named: Vec<&str> = Vec::new();
    let mut faces = Row::new().spacing(4);
    for member in session.members.iter().filter(|m| !m.you) {
        if named.contains(&member.username.as_str()) {
            continue;
        }
        named.push(&member.username);
        let path = member.avatar_url.as_deref();
        faces = faces.push(widgets::avatar(&member.username, path, artwork, FACE));
    }
    faces
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
