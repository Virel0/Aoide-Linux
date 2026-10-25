//! The player bar's popovers: drawn over the window just above the bar, at
//! its right-hand end; a click anywhere else closes them.

use iced::widget::{button, column, container, mouse_area, opaque, row, text};
use iced::{Center, Element, Fill, Length, Padding};

use crate::app::{App, Message};
use crate::icons::{Icon, Tone, icon};
use crate::listening::{BarMenu, Msg};
use crate::theme;

/// How far above the window's bottom edge a popover sits: the bar and a gap.
const ABOVE_BAR: f32 = 92.0;

/// The open popover over the whole window, or `None`.
pub fn overlay(app: &App) -> Option<Element<'_, Message>> {
    let menu = app.listening.menu?;
    let card = match menu {
        BarMenu::More => more(app),
    };
    let card = container(card)
        .padding(8)
        .width(Length::Fixed(280.0))
        .style(theme::menu);
    let placed = container(opaque(card))
        .width(Fill)
        .height(Fill)
        .align_right(Fill)
        .align_bottom(Fill)
        .padding(Padding::ZERO.bottom(ABOVE_BAR).right(16.0));
    Some(opaque(
        mouse_area(placed).on_press(Message::Listening(Msg::Menu(None))),
    ))
}

/// One line of a popover.
pub fn item<'a>(
    glyph: Option<Icon>,
    label: impl Into<String>,
    on: Message,
) -> Element<'a, Message> {
    let mut line = row![].spacing(10).align_y(Center);
    if let Some(glyph) = glyph {
        line = line.push(icon(glyph, 16.0, Tone::Secondary));
    }
    button(line.push(text(label.into()).size(14)))
        .width(Fill)
        .padding([6, 10])
        .style(theme::row)
        .on_press(on)
        .into()
}

fn more(_app: &App) -> Element<'_, Message> {
    column![item(
        Some(Icon::Settings),
        "Equaliser…",
        Message::Listening(Msg::OpenEqualizer)
    ),]
    .spacing(2)
    .into()
}

/// A bar button that opens one of its popovers.
pub fn menu_button<'a>(glyph: Icon, open: bool, menu: BarMenu) -> Element<'a, Message> {
    button(icon(
        glyph,
        18.0,
        if open { Tone::Accent } else { Tone::Normal },
    ))
    .padding(6)
    .style(if open { theme::icon_on } else { theme::row })
    .on_press(Message::Listening(Msg::Menu(Some(menu))))
    .into()
}
