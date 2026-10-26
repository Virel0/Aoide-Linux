//! The queue, under its headings: Played, Now Playing, Next Up, Then. Rows
//! jump when clicked, move up and down within their section, and come out.

use aoide_audio::EntryId;
use iced::widget::{Column, button, column, row, scrollable, text};
use iced::{Center, Element, Fill, Length};

use crate::app::{App, Message, Signed};
use crate::artwork::ArtKey;
use crate::icons::{Icon, Tone, icon};
use crate::playback::queue::{self, Heading};
use crate::{theme, widgets};

pub const SCROLL: &str = "queue-scroll";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueAction {
    Jump(EntryId),
    Remove(EntryId),
    /// Move the entry at `index` to `destination` (an offset before removal).
    Move {
        index: usize,
        destination: usize,
    },
    /// Drop everything after the current track.
    Clear,
}

fn small(glyph: Icon, message: Option<Message>) -> Element<'static, Message> {
    button(icon(glyph, 14.0, Tone::Secondary))
        .padding(4)
        .style(theme::row)
        .on_press_maybe(message)
        .into()
}

pub fn view<'a>(app: &'a App, _signed: &'a Signed) -> Element<'a, Message> {
    let Some(queue) = app.playback.queue().filter(|q| !q.is_empty()) else {
        return widgets::empty("Nothing queued. Play an album or a song to start.");
    };
    let mut list = Column::new().spacing(4);
    for section in queue::sections(queue) {
        // Clear empties what is after the current track.
        let is_next = matches!(section.heading, Heading::NextUp | Heading::Then);
        let clear = is_next && section.range.start == queue.position().unwrap_or(0) + 1;
        list = list.push(heading(&section, clear));
        for (offset, entry) in section.entries.iter().enumerate() {
            if let Some(line) = entry_row(app, &section, offset, *entry) {
                list = list.push(line);
            }
        }
    }
    scrollable(list.padding(iced::Padding::ZERO.right(12)))
        .id(SCROLL)
        .height(Fill)
        .into()
}

/// A section's title, with Clear for the first one after the current track.
fn heading<'a>(section: &queue::Section, clear: bool) -> Element<'a, Message> {
    let mut heading = row![
        text(section.heading.title())
            .size(13)
            .font(widgets::SEMIBOLD)
            .width(Fill)
    ]
    .align_y(Center);
    if clear {
        heading = heading.push(
            button(text("Clear").size(12))
                .style(theme::tonal)
                .padding([2, 8])
                .on_press(Message::Queue(QueueAction::Clear)),
        );
    }
    heading.into()
}

/// One queued track: its cover, title, artist and why Infinity added it,
/// and (except the one playing) Up, Down and Remove.
fn entry_row<'a>(
    app: &'a App,
    section: &queue::Section,
    offset: usize,
    entry: EntryId,
) -> Option<Element<'a, Message>> {
    let index = section.range.start + offset;
    let queued = app.playback.queued_at(index)?;
    let now = section.heading == Heading::NowPlaying;
    let played = section.heading == Heading::Played;
    let mut lines = column![
        text(queued.meta.title.as_str())
            .size(13)
            .wrapping(text::Wrapping::None)
            .style(move |t| text::Style {
                color: if now {
                    Some(theme::accent(t))
                } else if played {
                    Some(theme::secondary(t))
                } else {
                    None
                },
            }),
        widgets::secondary(queued.meta.artist.as_str())
            .size(11)
            .wrapping(text::Wrapping::None),
    ]
    .spacing(1)
    .width(Fill)
    .clip(true);
    if let Some(reason) = &queued.reason {
        lines = lines.push(
            text(reason.as_str())
                .size(10)
                .wrapping(text::Wrapping::None)
                .style(|t| text::Style {
                    color: Some(theme::accent(t)),
                }),
        );
    }
    let art = widgets::art(
        queued
            .meta
            .artwork
            .as_ref()
            .map(|(i, t)| ArtKey::new(i, t))
            .as_ref(),
        &app.artwork,
        Length::Fixed(36.0),
        4.0,
    );
    let step = |up: bool| {
        queue::step_destination(&section.range, index, up)
            .map(|destination| Message::Queue(QueueAction::Move { index, destination }))
    };
    let mut line = row![
        button(row![art, lines].spacing(8).align_y(Center))
            .padding([3, 4])
            .width(Fill)
            .style(if now { theme::row_selected } else { theme::row })
            .on_press(Message::Queue(QueueAction::Jump(entry))),
    ]
    .spacing(2)
    .align_y(Center);
    if !now {
        line = line
            .push(small(Icon::Up, step(true)))
            .push(small(Icon::Down, step(false)))
            .push(small(
                Icon::Remove,
                Some(Message::Queue(QueueAction::Remove(entry))),
            ));
    }
    Some(line.into())
}
