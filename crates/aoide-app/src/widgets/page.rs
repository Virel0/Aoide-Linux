//! A page's frame and its states: the title bar, the scrolled body,
//! loading, empty and failed.

use iced::widget::{button, column, container, row, scrollable, text};
use iced::{Center, Element, Fill};

use super::{BOLD, secondary};
use crate::app::Message;
use crate::failure::Failure;
use crate::icons::{Icon, Tone, icon};
use crate::theme;

/// A screen's title, with a back button when there is somewhere to go back to.
pub fn title_bar<'a>(title: &'a str, back: bool) -> Element<'a, Message> {
    let mut bar = row![].spacing(8).align_y(Center);
    if back {
        bar = bar.push(
            button(icon(Icon::Back, 20.0, Tone::Accent))
                .style(theme::row)
                .padding(4)
                .on_press(Message::Back),
        );
    }
    bar.push(text(title).size(28).font(BOLD)).into()
}

/// The scrollable every page is drawn in, put back at its top when another
/// page opens.
pub const PAGE: &str = "page-scroll";

/// A page's whole body: scrolled, padded.
pub fn page<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    scrollable(container(content).padding([24, 28]).width(Fill))
        .id(PAGE)
        .height(Fill)
        .into()
}

pub fn loading<'a>() -> Element<'a, Message> {
    container(secondary("Loading…")).padding(20).into()
}

/// A failure whose words are all there is: the local store's, the app's own.
pub fn plain_failure<'a>(error: &'a str) -> Element<'a, Message> {
    container(text(error))
        .padding(12)
        .style(theme::error_box)
        .into()
}

/// Something that failed, in words: Retry runs it again (when there is
/// something to run), and Details shows the technical text under it.
pub fn failure<'a>(
    failure: &'a Failure,
    retry: Option<Message>,
    details_open: bool,
) -> Element<'a, Message> {
    let has_actions = retry.is_some() || failure.details.is_some();
    let mut actions = row![].spacing(8).align_y(Center);
    if let Some(retry) = retry {
        actions = actions.push(
            button(text("Retry").size(13))
                .style(theme::tonal)
                .padding([4, 12])
                .on_press(retry),
        );
    }
    if failure.details.is_some() {
        actions = actions.push(
            button(
                row![
                    text("Details").size(13),
                    icon(
                        if details_open { Icon::Up } else { Icon::Down },
                        14.0,
                        Tone::Secondary
                    ),
                ]
                .spacing(4)
                .align_y(Center),
            )
            .style(theme::row)
            .padding([4, 8])
            .on_press(Message::FailureDetails(!details_open)),
        );
    }
    let mut body = column![text(failure.summary.as_str())].spacing(10);
    if has_actions {
        body = body.push(actions);
    }
    if let (true, Some(details)) = (details_open, &failure.details) {
        body = body.push(secondary(details.as_str()).size(12));
    }
    container(body)
        .padding(12)
        .width(Fill)
        .style(theme::error_box)
        .into()
}

pub fn empty<'a>(message: &'a str) -> Element<'a, Message> {
    container(secondary(message)).padding(20).into()
}
