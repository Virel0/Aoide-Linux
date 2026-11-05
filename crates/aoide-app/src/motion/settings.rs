//! Settings → General → Motion covers: this computer's switch.

use iced::Element;
use iced::widget::column;

use super::Msg;
use crate::app::Message;
use crate::widgets;

const FOOTER: &str = "Albums with a looping video in their folder play it, muted, over the \
                      cover on the album's page and in the full-screen player. Where this \
                      computer can't play one, the cover shows as usual.";

pub fn section<'a>(enabled: bool) -> Element<'a, Message> {
    let rows = column![widgets::switch("Motion covers", enabled, |on| {
        Message::Motion(Msg::Enabled(on))
    })];
    crate::screens::settings::section("Motion covers", rows, FOOTER)
}
