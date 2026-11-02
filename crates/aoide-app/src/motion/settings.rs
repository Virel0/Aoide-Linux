//! Settings → General → Motion covers: this computer's switch, and, while
//! the server makes them, whether to see the ones it made from still covers
//! (the server keeps that choice, for every device).

use iced::widget::{column, text};
use iced::{Element, Theme};

use super::{Motion, Msg};
use crate::app::Message;
use crate::widgets;

const FOOTER: &str = "Albums with a looping video play it, muted, over the cover on the \
                      album's page and in the full-screen player. Your server can also make \
                      them from still covers when its admin turns that on. Where this \
                      computer can't play one, the cover shows as usual.";

pub fn section<'a>(motion: &'a Motion, enabled: bool) -> Element<'a, Message> {
    let set = |m: Msg| Message::Motion(m);
    let mut rows = column![widgets::switch("Motion covers", enabled, move |on| {
        set(Msg::Enabled(on))
    })];
    if let Some(choice) = motion.choice.filter(|c| enabled && c.on) {
        rows = rows.push(widgets::switch(
            "Ones your server made",
            choice.show_generated,
            move |on| set(Msg::ShowGenerated(on)),
        ));
    }
    if let Some(problem) = &motion.problem {
        rows = rows.push(
            text(problem.as_str())
                .size(12)
                .style(|t: &Theme| text::Style {
                    color: Some(t.palette().danger),
                }),
        );
    }
    crate::screens::settings::section("Motion covers", rows, FOOTER)
}
