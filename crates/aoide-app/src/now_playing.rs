//! The Now Playing panel on the right: the cover, and the queue or the lyrics.

use iced::widget::{button, column, container, row, text};
use iced::{Center, Element, Fill, Length};

use crate::app::{App, Message, Panel, Signed, Transport};
use crate::artwork::ArtKey;
use crate::icons::{Icon, Tone, icon};
use crate::{theme, widgets};

pub const WIDTH: f32 = 340.0;

pub fn view<'a>(app: &'a App, signed: &'a Signed, panel: Panel) -> Element<'a, Message> {
    let current = app.playback.current().map(|(_, q)| &q.meta);
    let tab = |label: &'a str, which: Panel| {
        button(text(label).size(13))
            .padding([5, 12])
            .style(if panel == which {
                theme::row_selected
            } else {
                theme::row
            })
            .on_press(Message::Player(Transport::Panel(which)))
    };
    let header = row![
        tab("Queue", Panel::Queue),
        tab("Lyrics", Panel::Lyrics),
        crate::shell::gap(),
        button(icon(Icon::Close, 14.0, Tone::Secondary))
            .style(theme::row)
            .padding(6)
            .on_press(Message::Player(Transport::ClosePanel)),
    ]
    .spacing(4)
    .align_y(Center);
    let cover = column![
        widgets::art(
            current
                .and_then(|m| m.artwork.as_ref())
                .map(|(i, t)| ArtKey::new(i, t))
                .as_ref(),
            &app.artwork,
            Length::Fixed(220.0),
            10.0,
        ),
        text(current.map_or("Not playing", |m| m.title.as_str()))
            .size(16)
            .font(widgets::BOLD),
        widgets::secondary(current.map_or("", |m| m.artist.as_str())).size(13),
    ]
    .spacing(6)
    .align_x(Center);

    let body: Element<'a, Message> = match panel {
        Panel::Queue => crate::queue_panel::view(app, signed),
        Panel::Lyrics => crate::lyrics_panel::view(app, signed),
    };
    container(
        column![header, container(cover).center_x(Fill), body]
            .spacing(14)
            .padding(14),
    )
    .width(Length::Fixed(WIDTH))
    .height(Fill)
    .style(theme::side)
    .into()
}
