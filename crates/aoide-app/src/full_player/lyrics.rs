//! The lyrics beside the card. Synced: the line being sung large and white,
//! held in the middle, with two before and two after fading with distance; a
//! click on one seeks to it. Its reading and translation go under it, as the
//! lyrics panel has them switched on. Unsynced lyrics scroll as text.

use aoide_melete::{LyricLine, Lyrics};
use iced::widget::{Column, button, column, scrollable, text};
use iced::{Color, Element, Fill, Font, Theme};

use crate::app::{App, Message, Signed, Transport};
use crate::playback::lyrics::{current_line, seek_target};
use crate::playback::tracks::TrackMeta;
use crate::screens::Load;
use crate::{theme, widgets};

/// How many lines show on each side of the one being sung.
const REACH: usize = 2;

/// The lyrics side, or `None` when the song has none (the card then stands
/// alone). While they load, an empty side holds the layout still.
pub fn side<'a>(
    app: &'a App,
    signed: &'a Signed,
    current: Option<&TrackMeta>,
) -> Option<Element<'a, Message>> {
    let current = current?;
    let Some(state) = signed.lyrics.as_ref().filter(|s| s.item == current.id) else {
        return Some(iced::widget::space().width(Fill).into());
    };
    let lyrics = match &state.lyrics {
        Load::Loading => return Some(iced::widget::space().width(Fill).into()),
        Load::Ready(Some(lyrics)) if !lyrics.lines.is_empty() => lyrics,
        _ => return None,
    };
    let lines = if lyrics.is_synced() {
        synced(app, &state.item, lyrics)
    } else {
        plain(lyrics)
    };
    // Credited as the lyrics panel credits it.
    let credit = app
        .listening
        .lyrics_source
        .as_ref()
        .filter(|(item, _)| *item == state.item)
        .map(|(_, provider)| widgets::secondary(format!("Lyrics from {provider}")).size(12));
    Some(match credit {
        Some(credit) => column![lines, credit].spacing(24).into(),
        None => lines,
    })
}

/// The lines to show for `lit` among `count`: `REACH` slots each side of
/// it, `None` where there is no line (before the first, after the last).
/// Before the first line is sung, the first ones wait below the middle.
fn slots(lit: Option<usize>, count: usize) -> Vec<Option<usize>> {
    let middle = lit.map_or(-1, |line| line as isize);
    let reach = REACH as isize;
    (middle - reach..=middle + reach)
        .map(|index| usize::try_from(index).ok().filter(|&i| i < count))
        .collect()
}

fn synced<'a>(app: &'a App, item: &'a str, lyrics: &'a Lyrics) -> Element<'a, Message> {
    let offset = app.config.settings.lyric_offset_ms;
    let lit = current_line(lyrics, app.playback.position_ms(), offset);
    let mut lines = Column::new().spacing(18).width(Fill);
    for (place, slot) in slots(lit, lyrics.lines.len()).into_iter().enumerate() {
        let (size, alpha) = match place.abs_diff(REACH) {
            0 => (34.0, 1.0),
            1 => (22.0, 0.5),
            _ => (20.0, 0.28),
        };
        lines = lines.push(match slot {
            Some(index) => line(app, item, index, &lyrics.lines[index], size, alpha, offset),
            // An empty slot is as tall as a line would be, so the lit one
            // stays put.
            None => text(" ").size(size).into(),
        });
    }
    lines.into()
}

/// One synced line: lit when `alpha` is 1, with its reading over it and its
/// pronunciation and translation under it.
fn line<'a>(
    app: &'a App,
    item: &str,
    index: usize,
    line: &'a LyricLine,
    size: f32,
    alpha: f32,
    offset: i32,
) -> Element<'a, Message> {
    let lit = alpha >= 1.0;
    let font = if lit { widgets::BOLD } else { widgets::REGULAR };
    let colour = Color {
        a: alpha,
        ..Color::WHITE
    };
    let ruby = lit
        .then(|| crate::lyrics_extras::ruby_line(app, item, index, size, font, white))
        .flatten();
    let sung = ruby.unwrap_or_else(|| words(line, size, font, colour));
    let mut body = column![sung].spacing(6);
    if lit {
        for extra in crate::lyrics_extras::under_text(app, item, index) {
            body = body.push(text(extra).size(18).style(|_| text::Style {
                color: Some(Color {
                    a: 0.72,
                    ..Color::WHITE
                }),
            }));
        }
    }
    button(body)
        .padding([2, 6])
        .width(Fill)
        .style(theme::row)
        .on_press_maybe(seek_target(line, offset).map(|ms| Message::Player(Transport::Seek(ms))))
        .into()
}

fn words<'a>(line: &'a LyricLine, size: f32, font: Font, colour: Color) -> Element<'a, Message> {
    let said = if line.text.trim().is_empty() {
        "\u{266a}"
    } else {
        line.text.as_str()
    };
    text(said)
        .size(size)
        .font(font)
        .style(move |_| text::Style {
            color: Some(colour),
        })
        .into()
}

fn white(_theme: &Theme) -> Color {
    Color::WHITE
}

/// Unsynced lyrics: all of them, to scroll through.
fn plain(lyrics: &Lyrics) -> Element<'_, Message> {
    let lines = lyrics.lines.iter().map(|line| {
        text(line.text.as_str())
            .size(20)
            .style(|_| text::Style {
                color: Some(Color {
                    a: 0.82,
                    ..Color::WHITE
                }),
            })
            .into()
    });
    scrollable(Column::with_children(lines).spacing(10).width(Fill))
        .height(iced::Length::Fixed(600.0))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sung_line_sits_in_the_middle_with_two_each_side() {
        assert_eq!(
            slots(Some(5), 10),
            [Some(3), Some(4), Some(5), Some(6), Some(7)]
        );
    }

    #[test]
    fn missing_lines_at_either_end_are_empty_slots() {
        assert_eq!(slots(Some(0), 10), [None, None, Some(0), Some(1), Some(2)]);
        assert_eq!(slots(Some(9), 10), [Some(7), Some(8), Some(9), None, None]);
        assert_eq!(slots(Some(0), 1), [None, None, Some(0), None, None]);
    }

    #[test]
    fn before_the_first_line_it_waits_below_the_middle() {
        assert_eq!(slots(None, 10), [None, None, None, Some(0), Some(1)]);
    }
}
