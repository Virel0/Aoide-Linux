//! Light and dark, following the system, with Aoide's one accent colour (the
//! phone's: a violet, lighter on dark so it stays legible) or the artwork's
//! (`artwork`). Calm surfaces; the artwork carries the colour.

pub mod artwork;

use iced::theme::palette::deviate;
use iced::theme::{Mode, Palette};
use iced::widget::{button, container, slider, text_input, toggler};
use iced::{Background, Border, Color, Shadow, Theme, color};

/// Light mode's surfaces. iced derives its greys from white in steps of 3 to
/// 15%, and its 10% card (#dedede) and 7% bar read heavy on a white page;
/// these are lighter, close to macOS's own. Dark mode keeps iced's shades.
/// (Contrast: secondary text at least 4.5:1 on the card, the bar and a
/// hovered row; a tonal button's accent text at least 4.5:1; tested.)
mod light {
    use iced::{Color, color};

    /// A card: Settings' groups, the action strip, a sheet.
    pub const CARD: Color = color!(0xf2f2f5);
    /// The player bar.
    pub const BAR: Color = color!(0xf5f5f7);
    /// A row under the pointer, and pressed.
    pub const HOVER: Color = color!(0xebebef);
    pub const PRESSED: Color = color!(0xe1e1e6);
    /// A tonal button's ground: darker than a card, so it shows on one as
    /// well as on the page.
    pub const TONAL: Color = color!(0xececf0);
    pub const TONAL_HOVER: Color = color!(0xe7e7eb);
}

/// The accent on a light ground.
pub const ACCENT_LIGHT: Color = color!(0x6244e5);
/// The same accent on a dark ground.
pub const ACCENT_DARK: Color = color!(0xa593ff);

pub fn for_mode(mode: Mode) -> Theme {
    match mode {
        Mode::Dark => Theme::custom(
            "Aoide Dark",
            Palette {
                background: color!(0x1c1c1e),
                text: color!(0xf2f2f7),
                primary: ACCENT_DARK,
                success: color!(0x30d158),
                warning: color!(0xffd60a),
                danger: color!(0xff6961),
            },
        ),
        Mode::Light | Mode::None => Theme::custom(
            "Aoide Light",
            Palette {
                background: Color::WHITE,
                text: color!(0x1c1c1e),
                primary: ACCENT_LIGHT,
                success: color!(0x248a3d),
                warning: color!(0xb25000),
                danger: color!(0xd70015),
            },
        ),
    }
}

pub fn accent(theme: &Theme) -> Color {
    theme.palette().primary
}

/// Text that says less than the title beside it: the text colour, faded
/// less on a light ground, where 60% falls under 4.5:1 on the cards
/// (contrast: at least 4.5:1 on the page, the sidebar, the bar and a card,
/// in both modes; tested).
pub fn secondary(theme: &Theme) -> Color {
    let fade = if theme.extended_palette().is_dark {
        0.6
    } else {
        0.68
    };
    theme.palette().text.scale_alpha(fade)
}

/// The sidebar and the side panel: a shade off the page.
pub fn side(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(palette.background.weakest.color)),
        text_color: Some(palette.background.base.text),
        ..container::Style::default()
    }
}

pub fn page(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(palette.background.base.color)),
        text_color: Some(palette.background.base.text),
        ..container::Style::default()
    }
}

/// The full player's frosted card: a pale veil over the blurred cover,
/// edged in light, floating on a soft shadow.
pub fn glass(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::WHITE.scale_alpha(0.08))),
        border: Border {
            color: Color::WHITE.scale_alpha(0.2),
            width: 1.0,
            radius: 24.0.into(),
        },
        shadow: Shadow {
            color: Color::BLACK.scale_alpha(0.35),
            offset: iced::Vector::new(0.0, 16.0),
            blur_radius: 48.0,
        },
        ..container::Style::default()
    }
}

pub fn bar(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    let ground = if palette.is_dark {
        palette.background.weaker.color
    } else {
        light::BAR
    };
    container::Style {
        background: Some(Background::Color(ground)),
        text_color: Some(palette.background.base.text),
        border: Border {
            color: palette.background.strong.color,
            width: 0.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    }
}

/// The dimmed window behind a sheet or a tile's actions.
pub fn scrim(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::BLACK.scale_alpha(0.45))),
        ..container::Style::default()
    }
}

/// Where artwork would be, before it arrives or when there is none.
pub fn placeholder(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(palette.background.strong.color)),
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// A card's ground.
fn card(theme: &Theme) -> Color {
    let palette = theme.extended_palette();
    if palette.is_dark {
        palette.background.weak.color
    } else {
        light::CARD
    }
}

/// The inline action strip under a row, and every card.
pub fn menu(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(card(theme))),
        border: Border {
            radius: 8.0.into(),
            ..Border::default()
        },
        text_color: Some(palette.background.base.text),
        shadow: Shadow::default(),
        snap: true,
    }
}

pub fn error_box(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(
            palette.danger.weak.color.scale_alpha(0.25),
        )),
        text_color: Some(palette.danger.base.color),
        border: Border {
            radius: 8.0.into(),
            ..Border::default()
        },
        ..container::Style::default()
    }
}

/// A row or a plain icon: no chrome until hovered.
pub fn row(theme: &Theme, status: button::Status) -> button::Style {
    let palette = theme.extended_palette();
    let (hovered, pressed) = if palette.is_dark {
        (
            palette.background.weak.color,
            palette.background.strong.color,
        )
    } else {
        (light::HOVER, light::PRESSED)
    };
    let background = match status {
        button::Status::Hovered => Some(Background::Color(hovered)),
        button::Status::Pressed => Some(Background::Color(pressed)),
        _ => None,
    };
    button::Style {
        background,
        text_color: palette.background.base.text,
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// The row that is playing, or the sidebar item that is open.
pub fn row_selected(theme: &Theme, status: button::Status) -> button::Style {
    let palette = theme.extended_palette();
    let mut style = row(theme, status);
    if style.background.is_none() {
        style.background = Some(Background::Color(palette.background.weaker.color));
    }
    style.text_color = palette.primary.base.color;
    style
}

/// An icon that is switched on (shuffle, repeat, crossfade, infinity).
pub fn icon_on(theme: &Theme, status: button::Status) -> button::Style {
    let mut style = row(theme, status);
    style.text_color = accent(theme);
    style
}

/// The one filled button on a screen: Play, Connect.
pub fn filled(theme: &Theme, status: button::Status) -> button::Style {
    let mut style = button::primary(theme, status);
    style.border.radius = 8.0.into();
    style
}

/// A quieter button beside it: Shuffle, Cancel. Its ground is a shade off
/// the page, a different one from a card's so it shows on either, and pale
/// enough for its accent text (contrast: accent text at least 4.5:1, resting
/// and hovered, in every accent and both modes; tested).
///
/// On a page's wash its ground is a veil instead (`artwork::with_accent`).
pub fn tonal(theme: &Theme, status: button::Status) -> button::Style {
    let palette = theme.extended_palette();
    let shades = &palette.background;
    // Only a veiled theme has translucent shades.
    let veiled = shades.weaker.color.a < 1.0;
    let grounds = if veiled {
        (shades.weaker.color, shades.weakest.color)
    } else {
        solid_tonal(theme)
    };
    tonal_on(theme, status, grounds)
}

/// A tonal button's own grounds, resting and hovered: in dark mode, iced's
/// weaker and weakest shades of the page, worked out again here as iced does
/// so a veiled theme's are not taken for them.
fn solid_tonal(theme: &Theme) -> (Color, Color) {
    let palette = theme.extended_palette();
    if palette.is_dark {
        let page = palette.background.base.color;
        (deviate(page, 0.07), deviate(page, 0.03))
    } else {
        (light::TONAL, light::TONAL_HOVER)
    }
}

fn tonal_on(
    theme: &Theme,
    status: button::Status,
    (resting, hovered): (Color, Color),
) -> button::Style {
    let palette = theme.extended_palette();
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => hovered,
        _ => resting,
    };
    button::Style {
        background: Some(Background::Color(background)),
        text_color: unless_disabled(palette.primary.base.color, status),
        border: Border {
            radius: 8.0.into(),
            ..Border::default()
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Delete and the like: on its own ground even on the wash, where a veil
/// would leave the danger colour under 4.5:1.
pub fn destructive(theme: &Theme, status: button::Status) -> button::Style {
    let mut style = tonal_on(theme, status, solid_tonal(theme));
    style.text_color = unless_disabled(theme.palette().danger, status);
    style
}

/// A label's colour, faded by half when its button can't be pressed, as
/// iced fades its own buttons.
fn unless_disabled(colour: Color, status: button::Status) -> Color {
    if let button::Status::Disabled = status {
        colour.scale_alpha(0.5)
    } else {
        colour
    }
}

/// The edge of a control that has no fill of its own to show where it is:
/// a field, a switch turned off (contrast: at least 3:1 on the page and on
/// a card, in both modes; tested).
pub fn edge(theme: &Theme) -> Color {
    let fade = if theme.extended_palette().is_dark {
        0.4
    } else {
        0.5
    };
    theme.palette().text.scale_alpha(fade)
}

pub fn field(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let mut style = text_input::default(theme, status);
    style.border.radius = 8.0.into();
    style.placeholder = secondary(theme);
    if let text_input::Status::Active = status {
        style.border.color = edge(theme);
    }
    style
}

/// A switch: off, a knob on the edge colour's track, so it is seen at all
/// on a card; on, as iced draws it, in the accent.
pub fn switch(theme: &Theme, status: toggler::Status) -> toggler::Style {
    let mut style = toggler::default(theme, status);
    if let toggler::Status::Active { is_toggled: false }
    | toggler::Status::Hovered { is_toggled: false } = status
    {
        style.background = Background::Color(edge(theme));
        style.foreground = Background::Color(theme.extended_palette().background.base.color);
    }
    style
}

pub fn scrubber(theme: &Theme, status: slider::Status) -> slider::Style {
    let mut style = slider::default(theme, status);
    let palette = theme.extended_palette();
    style.rail.backgrounds = (
        Background::Color(accent(theme)),
        Background::Color(palette.background.strong.color),
    );
    style.rail.width = 4.0;
    style.handle.shape = slider::HandleShape::Circle { radius: 6.0 };
    style.handle.background = Background::Color(palette.background.base.text);
    style.handle.border_width = 0.0;
    style
}

#[cfg(test)]
mod tests;
