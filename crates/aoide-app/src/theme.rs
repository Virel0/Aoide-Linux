//! Light and dark, following the system, with Aoide's one accent colour (the
//! phone's: a violet, lighter on dark so it stays legible). Calm surfaces;
//! the artwork carries the colour.

use iced::theme::{Mode, Palette};
use iced::widget::{button, container, slider, text_input};
use iced::{Background, Border, Color, Shadow, Theme, color};

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

/// Text that says less than the title beside it.
pub fn secondary(theme: &Theme) -> Color {
    theme.palette().text.scale_alpha(0.6)
}

pub fn faint(theme: &Theme) -> Color {
    theme.palette().text.scale_alpha(0.35)
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

/// The player bar along the bottom.
pub fn bar(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(palette.background.weaker.color)),
        text_color: Some(palette.background.base.text),
        border: Border {
            color: palette.background.strong.color,
            width: 0.0,
            radius: 0.0.into(),
        },
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

/// The inline action strip under a row.
pub fn menu(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(Background::Color(palette.background.weak.color)),
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
    let background = match status {
        button::Status::Hovered => Some(Background::Color(palette.background.weak.color)),
        button::Status::Pressed => Some(Background::Color(palette.background.strong.color)),
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

/// A quieter button beside it: Shuffle, Cancel.
pub fn tonal(theme: &Theme, status: button::Status) -> button::Style {
    let palette = theme.extended_palette();
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => palette.background.strong.color,
        _ => palette.background.weak.color,
    };
    button::Style {
        background: Some(Background::Color(background)),
        text_color: palette.primary.base.color,
        border: Border {
            radius: 8.0.into(),
            ..Border::default()
        },
        shadow: Shadow::default(),
        snap: true,
    }
}

pub fn destructive(theme: &Theme, status: button::Status) -> button::Style {
    let mut style = tonal(theme, status);
    style.text_color = theme.palette().danger;
    style
}

pub fn field(theme: &Theme, status: text_input::Status) -> text_input::Style {
    let mut style = text_input::default(theme, status);
    style.border.radius = 8.0.into();
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
