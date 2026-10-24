//! Lyrics for what is playing.

use iced::Element;

use crate::app::{App, Message, Signed};
use crate::widgets;

pub fn view<'a>(_app: &'a App, _signed: &'a Signed) -> Element<'a, Message> {
    widgets::empty("No lyrics.")
}
