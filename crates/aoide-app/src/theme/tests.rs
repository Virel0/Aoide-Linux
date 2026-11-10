//! The theme's contrast, in every mode and accent.

use super::*;
use crate::appearance::{self, Accent, Appearance, InterfaceSettings};

/// `fg` drawn over `bg`.
fn over(fg: Color, bg: Color) -> Color {
    let mix = |f: f32, b: f32| f * fg.a + b * (1.0 - fg.a);
    Color::from_rgb(mix(fg.r, bg.r), mix(fg.g, bg.g), mix(fg.b, bg.b))
}

/// WCAG 2's contrast ratio of `fg`, drawn over `bg`.
fn contrast(fg: Color, bg: Color) -> f32 {
    let fg = over(fg, bg);
    fn luminance(c: Color) -> f32 {
        let channel = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b)
    }
    let (a, b) = (luminance(fg), luminance(bg));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Every mode in every accent, and in the colours of a spread of covers:
/// the window's theme while each plays, and a page's in its cover's.
fn themes() -> Vec<Theme> {
    let mut all = Vec::new();
    for appearance in [Appearance::Light, Appearance::Dark] {
        for accent in Accent::ALL {
            let interface = InterfaceSettings {
                appearance,
                accent,
                ..InterfaceSettings::default()
            };
            all.push(appearance::theme(Mode::Light, &interface, None));
        }
        let interface = InterfaceSettings {
            appearance,
            accent: Accent::Artwork,
            ..InterfaceSettings::default()
        };
        for (_, cover) in crate::cover_colour::COLOURFUL {
            let look = crate::cover_colour::Look::of(&[cover]).expect("a colour");
            let window = appearance::theme(Mode::Light, &interface, Some(&look));
            all.push(artwork::PageWash::new(&window, &look).page);
            all.push(window);
        }
    }
    all
}

fn button_ground(style: button::Style) -> Color {
    match style.background {
        Some(Background::Color(colour)) => colour,
        other => panic!("not a plain colour: {other:?}"),
    }
}

#[test]
fn tonal_buttons_keep_their_text_legible() {
    for theme in themes() {
        for status in [button::Status::Active, button::Status::Hovered] {
            let style = tonal(&theme, status);
            let ratio = contrast(style.text_color, button_ground(style));
            assert!(ratio >= 4.5, "{theme} {status:?}: {ratio}");
        }
        let style = destructive(&theme, button::Status::Active);
        let ratio = contrast(style.text_color, button_ground(style));
        assert!(ratio >= 4.5, "{theme} destructive: {ratio}");
    }
}

#[test]
fn fields_and_switches_show_their_edges() {
    for theme in themes() {
        let page_ground = ground(page(&theme));
        let card = ground(menu(&theme));
        let field = field(&theme, text_input::Status::Active);
        let off = switch(&theme, toggler::Status::Active { is_toggled: false });
        let Background::Color(track) = off.background else {
            panic!("not a plain colour")
        };
        let Background::Color(knob) = off.foreground else {
            panic!("not a plain colour")
        };
        for (name, surface) in [("page", page_ground), ("card", card)] {
            let ratio = contrast(field.border.color, surface);
            assert!(ratio >= 3.0, "{theme} field edge on {name}: {ratio}");
            let ratio = contrast(track, surface);
            assert!(ratio >= 3.0, "{theme} switch track on {name}: {ratio}");
            let ratio = contrast(knob, over(track, surface));
            assert!(ratio >= 3.0, "{theme} switch knob on {name}: {ratio}");
        }
        // The placeholder is drawn on the field's own ground.
        let ratio = contrast(field.placeholder, page_ground);
        assert!(ratio >= 4.5, "{theme} placeholder: {ratio}");
    }
}

fn ground(style: container::Style) -> Color {
    match style.background {
        Some(Background::Color(colour)) => colour,
        other => panic!("not a plain colour: {other:?}"),
    }
}

#[test]
fn secondary_text_reads_on_every_surface() {
    for theme in themes() {
        for (name, surface) in [
            ("page", ground(page(&theme))),
            ("sidebar", ground(side(&theme))),
            ("bar", ground(bar(&theme))),
            ("card", ground(menu(&theme))),
        ] {
            let ratio = contrast(secondary(&theme), surface);
            assert!(ratio >= 4.5, "{theme} {name}: {ratio}");
        }
    }
}

#[test]
fn secondary_text_reads_on_a_hovered_row() {
    for theme in themes() {
        let hovered = button_ground(row(&theme, button::Status::Hovered));
        let ratio = contrast(secondary(&theme), hovered);
        assert!(ratio >= 4.5, "{theme} hovered row: {ratio}");
    }
}

/// Lighter first: the page, a card, a tonal button, the same hovered.
/// A tonal button on a card or on the page is always a shade darker.
#[test]
fn light_surfaces_step_down_from_the_page() {
    fn luminance(c: Color) -> f32 {
        0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b
    }
    for theme in themes()
        .into_iter()
        .filter(|t| !t.extended_palette().is_dark)
    {
        let steps = [
            ground(page(&theme)),
            ground(menu(&theme)),
            button_ground(tonal(&theme, button::Status::Active)),
            button_ground(tonal(&theme, button::Status::Hovered)),
        ];
        for pair in steps.windows(2) {
            assert!(
                luminance(pair[0]) > luminance(pair[1]),
                "{theme}: {:?} then {:?}",
                pair[0],
                pair[1]
            );
        }
        // Lighter than the shade iced gives a card (#dedede).
        let iced_card = theme.extended_palette().background.weak.color;
        assert!(luminance(ground(menu(&theme))) > luminance(iced_card));
    }
}

#[test]
fn a_failure_and_its_details_read_on_their_box() {
    for theme in themes() {
        let style = error_box(&theme);
        let surface = over(ground(style), ground(page(&theme)));
        let words = style.text_color.expect("the box sets its text colour");
        let ratio = contrast(words, surface);
        assert!(ratio >= 4.5, "{theme} failure: {ratio}");
        let ratio = contrast(secondary(&theme), surface);
        assert!(ratio >= 4.5, "{theme} details: {ratio}");
    }
}

/// A dark tonal button's grounds are still iced's own shades of the page,
/// worked out again so a veiled theme's are never taken for them.
#[test]
fn a_dark_tonal_button_keeps_iceds_shades() {
    for theme in themes()
        .into_iter()
        .filter(|t| t.extended_palette().is_dark)
    {
        let shades = theme.extended_palette().background;
        let resting = button_ground(tonal(&theme, button::Status::Active));
        let hovered = button_ground(tonal(&theme, button::Status::Hovered));
        assert_eq!(resting, shades.weaker.color, "{theme}");
        assert_eq!(hovered, shades.weakest.color, "{theme}");
    }
}
