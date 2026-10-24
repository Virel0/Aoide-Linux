//! The queue, under its headings.

use iced::Element;

use crate::app::{App, Message, Signed};
use crate::widgets;

pub fn view<'a>(app: &'a App, _signed: &'a Signed) -> Element<'a, Message> {
    match app.playback.queue() {
        Some(queue) if !queue.is_empty() => widgets::empty("The queue."),
        _ => widgets::empty("Nothing queued."),
    }
}
