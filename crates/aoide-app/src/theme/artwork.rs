//! The look taken from the artwork (`cover_colour`): the window in the
//! playing song's accent with its shell tinted, and a page washed in its
//! cover's colour behind the header, its quiet buttons veiled on the wash
//! and its accent the cover's. One bold thing per page, the wash; the rest
//! stays as calm as the plain theme.

use std::f32::consts::PI;

use iced::gradient::Linear;
use iced::theme::Palette;
use iced::theme::palette::Extended;
use iced::widget::{button, container};
use iced::{Background, Border, Color, Gradient, Shadow, Theme, Vector};

use crate::cover_colour::{Derived, Look, Surfaces, WASH_MIDDLE, WASH_TOP};

#[cfg(test)]
mod tests;

/// A quiet button's veil on the wash, resting and hovered: white, faint on
/// a dark wash and stronger on a light one (a black veil would pull the
/// accent's text on a pale wash under 4.5:1).
const VEILS_DARK: [Color; 2] = [
    Color::from_rgba(1.0, 1.0, 1.0, 0.10),
    Color::from_rgba(1.0, 1.0, 1.0, 0.16),
];
const VEILS_LIGHT: [Color; 2] = [
    Color::from_rgba(1.0, 1.0, 1.0, 0.50),
    Color::from_rgba(1.0, 1.0, 1.0, 0.65),
];

/// One mode's grounds, from the plain theme's own styles, for the
/// derivation to read its colours on.
pub fn surfaces(dark: bool) -> Surfaces {
    let theme = super::for_mode(if dark {
        iced::theme::Mode::Dark
    } else {
        iced::theme::Mode::Light
    });
    let colour = |style: container::Style| match style.background {
        Some(Background::Color(colour)) => colour,
        _ => theme.palette().background,
    };
    let button = |style: button::Style| match style.background {
        Some(Background::Color(colour)) => colour,
        _ => theme.palette().background,
    };
    Surfaces {
        dark,
        page: colour(super::page(&theme)),
        text: theme.palette().text,
        secondary: super::secondary(&theme),
        shell: vec![colour(super::side(&theme)), colour(super::bar(&theme))],
        grounds: vec![
            colour(super::menu(&theme)),
            button(super::tonal(&theme, button::Status::Active)),
            button(super::tonal(&theme, button::Status::Hovered)),
            button(super::row(&theme, button::Status::Hovered)),
            button(super::row_selected(&theme, button::Status::Active)),
        ],
        veils: if dark { VEILS_DARK } else { VEILS_LIGHT },
    }
}

/// `base` in `derived`'s accent, a filled button labelled in its label.
/// `veiled`: the quiet buttons' grounds are the veils, for a page's header
/// on the wash (`super::tonal` knows them by their being translucent, which
/// no plain ground is).
pub fn with_accent(base: &Theme, derived: Derived, veiled: bool) -> Theme {
    let palette = Palette {
        primary: derived.accent,
        ..base.palette()
    };
    let name = format!("{base} from the artwork");
    Theme::custom_with_fn(name, palette, move |palette| {
        let mut extended = Extended::generate(palette);
        extended.primary.base.text = derived.label;
        extended.primary.strong.text = derived.label;
        if veiled {
            let [resting, hovered] = if extended.is_dark {
                VEILS_DARK
            } else {
                VEILS_LIGHT
            };
            extended.background.weaker.color = resting;
            extended.background.weakest.color = hovered;
        }
        extended
    })
}

/// A page washed in its cover's colour.
#[derive(Debug, Clone)]
pub struct PageWash {
    /// The colours for the window's mode.
    pub derived: Derived,
    /// The page's theme: the cover's accent.
    pub page: Theme,
    /// The header's: the same, with its quiet buttons veiled.
    pub header: Theme,
}

impl PageWash {
    /// The wash for a page in `window`'s mode, from its cover's `look`.
    pub fn new(window: &Theme, look: &Look) -> Self {
        let derived = look.for_theme(window);
        Self {
            derived,
            page: with_accent(window, derived, false),
            header: with_accent(window, derived, true),
        }
    }
}

/// Behind a page's header: the wash, strongest at the top and clear at the
/// header's foot, where the page's own ground takes over; nothing without one.
pub fn wash(derived: Option<Derived>) -> impl Fn(&Theme) -> container::Style {
    move |_theme| {
        let Some(derived) = derived else {
            return container::Style::default();
        };
        let gradient = Linear::new(PI)
            .add_stop(0.0, derived.wash.scale_alpha(WASH_TOP))
            .add_stop(0.5, derived.wash.scale_alpha(WASH_MIDDLE))
            .add_stop(1.0, derived.wash.scale_alpha(0.0));
        container::Style {
            background: Some(Background::Gradient(Gradient::Linear(gradient))),
            ..container::Style::default()
        }
    }
}

/// A cover on the wash, lifted on a soft shadow; `radius` is the cover's.
pub fn lifted(radius: f32) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let strength = if theme.extended_palette().is_dark {
            0.45
        } else {
            0.22
        };
        container::Style {
            border: Border {
                radius: radius.into(),
                ..Border::default()
            },
            shadow: Shadow {
                color: Color::BLACK.scale_alpha(strength),
                offset: Vector::new(0.0, 10.0),
                blur_radius: 30.0,
            },
            ..container::Style::default()
        }
    }
}

/// `style` (the sidebar's, the side panel's, the player bar's) with the
/// playing song's tint in its ground; as it is with nothing playing.
pub fn tinted(
    style: fn(&Theme) -> container::Style,
    playing: Option<Look>,
) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let mut style = style(theme);
        if let (Some(look), Some(Background::Color(ground))) = (playing, style.background) {
            let dark = theme.extended_palette().is_dark;
            let tinted = look.in_mode(dark).tinted(ground, dark);
            style.background = Some(Background::Color(tinted));
        }
        style
    }
}
