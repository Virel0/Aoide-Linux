//! Lyrics for what is playing. Synced lines follow the heard position (with
//! the listener's timing offset), the current one lit; a click on a line
//! seeks to it. Unsynced lyrics are shown as text.

use aoide_jellyfin::Lyrics;
use iced::widget::{Column, button, scrollable, text};
use iced::{Element, Fill};

use crate::app::{App, Message, Signed, Transport};
use crate::playback::lyrics;
use crate::screens::Load;
use crate::{theme, widgets};

pub const SCROLL: &str = "lyrics-scroll";

/// The lyrics the panel holds, and for which track.
#[derive(Debug, Clone, PartialEq)]
pub struct LyricsState {
    pub item: String,
    pub lyrics: Load<Option<Lyrics>>,
}

pub fn view<'a>(app: &'a App, signed: &'a Signed) -> Element<'a, Message> {
    let Some((_, current)) = app.playback.current() else {
        return widgets::empty("Nothing is playing.");
    };
    let Some(state) = signed.lyrics.as_ref().filter(|s| s.item == current.meta.id) else {
        return widgets::loading();
    };
    let lyrics = match &state.lyrics {
        Load::Loading => return widgets::loading(),
        Load::Failed(error) => return widgets::failure(error),
        Load::Ready(None) => return widgets::empty("No lyrics for this song."),
        Load::Ready(Some(lyrics)) => lyrics,
    };
    let offset = app.config.settings.lyric_offset_ms;
    let mut column = Column::new().spacing(6).width(Fill);
    if lyrics.is_synced() {
        let lit = lyrics::current_line(lyrics, app.playback.position_ms(), offset);
        for (index, line) in lyrics.lines.iter().enumerate() {
            let is_lit = lit == Some(index);
            let label = text(if line.text.trim().is_empty() {
                "♪"
            } else {
                line.text.as_str()
            })
            .size(if is_lit { 18 } else { 16 })
            .font(if is_lit {
                widgets::BOLD
            } else {
                widgets::REGULAR
            })
            .style(move |t| text::Style {
                color: Some(if is_lit {
                    theme::accent(t)
                } else {
                    theme::secondary(t)
                }),
            });
            column = column.push(
                button(label)
                    .padding([2, 4])
                    .width(Fill)
                    .style(theme::row)
                    .on_press_maybe(
                        lyrics::seek_target(line, offset)
                            .map(|ms| Message::Player(Transport::Seek(ms))),
                    ),
            );
        }
    } else {
        for line in &lyrics.lines {
            column = column.push(text(line.text.as_str()).size(15));
        }
    }
    scrollable(column.padding(iced::Padding::ZERO.right(12)))
        .id(SCROLL)
        .height(Fill)
        .into()
}
