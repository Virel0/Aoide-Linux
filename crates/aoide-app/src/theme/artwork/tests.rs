//! The artwork's look as the styles draw it, for a spread of covers in both
//! modes: the header on its wash, and the shell in its tint.

use iced::theme::Mode;
use iced::widget::{button, container};
use iced::{Background, Color, Theme};

use super::*;
use crate::appearance::{self, Accent, Appearance, InterfaceSettings};
use crate::cover_colour::{COLOURFUL, Look, contrast, over};

/// Each cover's window theme (as while it plays), its page's wash, and
/// its look.
fn washed() -> Vec<(String, Theme, PageWash, Look)> {
    let mut all = Vec::new();
    for appearance in [Appearance::Light, Appearance::Dark] {
        let interface = InterfaceSettings {
            appearance,
            accent: Accent::Artwork,
            ..InterfaceSettings::default()
        };
        for (name, cover) in COLOURFUL {
            let look = Look::of(&[cover]).expect("a colour");
            let window = appearance::theme(Mode::Light, &interface, Some(&look));
            let wash = PageWash::new(&window, &look);
            all.push((format!("{name}, {appearance:?}"), window, wash, look));
        }
    }
    all
}

fn plain(background: Option<Background>) -> Color {
    match background {
        Some(Background::Color(colour)) => colour,
        other => panic!("not a plain colour: {other:?}"),
    }
}

/// The wash at its strongest over the page.
fn wash_top(theme: &Theme, wash: &PageWash) -> Color {
    let page = plain(crate::theme::page(theme).background);
    wash.derived.wash_over(page, WASH_TOP)
}

#[test]
fn the_headers_quiet_buttons_read_on_their_veils() {
    for (name, _, wash, _) in washed() {
        let top = wash_top(&wash.header, &wash);
        for status in [button::Status::Active, button::Status::Hovered] {
            let style = crate::theme::tonal(&wash.header, status);
            let veil = plain(style.background);
            assert!(veil.a < 1.0, "{name} {status:?}: a veil");
            let ratio = contrast(style.text_color, over(veil, top));
            assert!(ratio >= 4.5, "{name} {status:?}: {ratio}");
        }
        // Delete keeps a ground of its own.
        let style = crate::theme::destructive(&wash.header, button::Status::Active);
        let ground = plain(style.background);
        assert!((ground.a - 1.0).abs() < f32::EPSILON, "{name}: solid");
        let ratio = contrast(style.text_color, ground);
        assert!(ratio >= 4.5, "{name} destructive: {ratio}");
    }
}

#[test]
fn the_headers_words_and_accent_read_on_the_wash() {
    for (name, _, wash, _) in washed() {
        let top = wash_top(&wash.header, &wash);
        let text = wash.header.palette().text;
        assert!(contrast(text, top) >= 4.5, "{name} text");
        let secondary = crate::theme::secondary(&wash.header);
        assert!(contrast(secondary, top) >= 4.5, "{name} secondary");
        // The artist's name under an album's title.
        let accent = crate::theme::accent(&wash.header);
        assert_eq!(accent, wash.derived.accent);
        assert!(contrast(accent, top) >= 4.5, "{name} accent");
    }
}

#[test]
fn play_is_filled_in_the_covers_accent_with_a_label_that_reads() {
    for (name, _, wash, _) in washed() {
        for theme in [&wash.page, &wash.header] {
            for status in [button::Status::Active, button::Status::Hovered] {
                let style = crate::theme::filled(theme, status);
                let ground = plain(style.background);
                let ratio = contrast(style.text_color, ground);
                assert!(ratio >= 4.5, "{name} {status:?}: {ratio}");
            }
            let resting = plain(crate::theme::filled(theme, button::Status::Active).background);
            assert_eq!(resting, wash.derived.accent, "{name}");
        }
    }
}

#[test]
fn the_shell_reads_in_its_tint() {
    for (name, window, _, look) in washed() {
        for style in [crate::theme::side, crate::theme::bar] {
            let plain_ground = plain(style(&window).background);
            let ground = plain(tinted(style, Some(look))(&window).background);
            assert_ne!(ground, plain_ground, "{name}: tinted");
            for (part, colour) in [
                ("text", window.palette().text),
                ("secondary", crate::theme::secondary(&window)),
                ("accent", crate::theme::accent(&window)),
            ] {
                let ratio = contrast(colour, ground);
                assert!(ratio >= 4.5, "{name} {part}: {ratio}");
            }
            // Nothing playing: the ground as it was.
            assert_eq!(plain(tinted(style, None)(&window).background), plain_ground);
        }
        // The selected sidebar item, lit in the accent.
        let selected = crate::theme::row_selected(&window, button::Status::Active);
        let ratio = contrast(selected.text_color, plain(selected.background));
        assert!(ratio >= 4.5, "{name} selected: {ratio}");
    }
}

#[test]
fn no_wash_draws_nothing_behind_the_header() {
    let theme = crate::theme::for_mode(Mode::Dark);
    assert_eq!(wash(None)(&theme).background, None);
    let look = Look::of(&[[220, 20, 30]]).expect("a red");
    let style: container::Style = wash(Some(look.dark))(&theme);
    assert!(matches!(style.background, Some(Background::Gradient(_))));
}
