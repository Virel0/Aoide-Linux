//! Settings → General: how the window looks and what its title says.

use iced::theme::{Mode, Palette};
use iced::widget::{button, column, container, row, space, text, toggler};
use iced::{Background, Border, Center, Color, Element, Fill, Length, Theme, color};
use serde::{Deserialize, Serialize};

use crate::app::{App, Message};
use crate::listening::{Msg, SettingsExtras};
use crate::{theme, widgets};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct InterfaceSettings {
    /// "Title — Artist · Aoide" in the window title while playing.
    pub track_in_title: bool,
    pub appearance: Appearance,
    pub accent: Accent,
    /// LRCLIB for songs the server has no lyrics for.
    pub internet_lyrics: bool,
    /// Romaji under each line of Japanese lyrics.
    pub romaji: bool,
    /// Readings over the kanji of Japanese lyrics (the `furigana` feature).
    pub furigana: bool,
}

impl Default for InterfaceSettings {
    fn default() -> Self {
        Self {
            track_in_title: true,
            appearance: Appearance::System,
            accent: Accent::Violet,
            internet_lyrics: true,
            romaji: false,
            furigana: false,
        }
    }
}

/// Light, dark, or as the system has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Appearance; 3] = [Appearance::System, Appearance::Light, Appearance::Dark];

    pub fn title(self) -> &'static str {
        match self {
            Appearance::System => "System",
            Appearance::Light => "Light",
            Appearance::Dark => "Dark",
        }
    }

    /// The mode to draw in, given the system's.
    pub fn mode(self, system: Mode) -> Mode {
        match self {
            Appearance::System => system,
            Appearance::Light => Mode::Light,
            Appearance::Dark => Mode::Dark,
        }
    }
}

/// The accent colour: Aoide's violet, or one of five others. Each is a pair,
/// darker on a light ground and lighter on a dark one, so text in it stays
/// legible (4.5:1 against the page, tested).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Accent {
    #[default]
    Violet,
    Blue,
    Teal,
    Orange,
    Pink,
    Graphite,
}

impl Accent {
    pub const ALL: [Accent; 6] = [
        Accent::Violet,
        Accent::Blue,
        Accent::Teal,
        Accent::Orange,
        Accent::Pink,
        Accent::Graphite,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Accent::Violet => "Violet",
            Accent::Blue => "Blue",
            Accent::Teal => "Teal",
            Accent::Orange => "Orange",
            Accent::Pink => "Pink",
            Accent::Graphite => "Graphite",
        }
    }

    /// The colour on a light ground and on a dark one.
    pub fn colours(self) -> (Color, Color) {
        match self {
            Accent::Violet => (theme::ACCENT_LIGHT, theme::ACCENT_DARK),
            Accent::Blue => (color!(0x0a5fcc), color!(0x6cb2ff)),
            Accent::Teal => (color!(0x0b7470), color!(0x45c9c0)),
            Accent::Orange => (color!(0xad4a00), color!(0xffa25c)),
            Accent::Pink => (color!(0xbd1559), color!(0xff85b0)),
            Accent::Graphite => (color!(0x55555c), color!(0xb8b8c0)),
        }
    }

    pub fn for_mode(self, mode: Mode) -> Color {
        let (light, dark) = self.colours();
        if mode == Mode::Dark { dark } else { light }
    }
}

/// The app's theme: the system's mode unless one was chosen, in the chosen
/// accent.
pub fn theme(system: Mode, interface: &InterfaceSettings) -> Theme {
    let mode = interface.appearance.mode(system);
    let base = theme::for_mode(mode);
    if interface.accent == Accent::Violet {
        return base;
    }
    let palette = Palette {
        primary: interface.accent.for_mode(mode),
        ..base.palette()
    };
    Theme::custom(format!("{} {}", base, interface.accent.title()), palette)
}

/// The window's title: the song while one plays (and the setting allows),
/// else the app's name.
pub fn window_title(enabled: bool, playing: Option<(&str, &str)>) -> String {
    match playing.filter(|_| enabled) {
        Some((title, artist)) if !title.trim().is_empty() => {
            if artist.trim().is_empty() {
                format!("{title} \u{b7} Aoide")
            } else {
                format!("{title} \u{2014} {artist} \u{b7} Aoide")
            }
        }
        _ => "Aoide".to_owned(),
    }
}

impl App {
    pub(crate) fn window_title(&self) -> String {
        let playing = self
            .playback
            .current()
            .filter(|_| self.playback.is_playing())
            .map(|(_, q)| (q.meta.title.as_str(), q.meta.artist.as_str()));
        window_title(self.config.interface.track_in_title, playing)
    }

    pub(crate) fn interface_changed(
        &mut self,
        interface: InterfaceSettings,
    ) -> iced::Task<Message> {
        let lyrics_switched = interface.internet_lyrics != self.config.interface.internet_lyrics;
        self.config.interface = interface;
        self.save_config();
        if !lyrics_switched {
            return iced::Task::none();
        }
        // Look the song's lyrics up again, with or without the internet.
        if let Some(signed) = self.session.as_mut() {
            signed.lyrics = None;
        }
        self.track_changed()
    }
}

/// Settings → General.
pub fn general_section<'a>(extras: &SettingsExtras<'a>) -> Element<'a, Message> {
    let interface = extras.config.interface.clone();
    let set = move |change: InterfaceSettings| Message::Listening(Msg::Interface(change));
    let title = {
        let interface = interface.clone();
        toggler(interface.track_in_title)
            .style(theme::switch)
            .label("Show the Song in the Window Title")
            .width(Fill)
            .on_toggle(move |on| {
                let mut change = interface.clone();
                change.track_in_title = on;
                set(change)
            })
    };
    let appearance = {
        let interface = interface.clone();
        let mut choices = row![].spacing(4);
        for choice in Appearance::ALL {
            let on = interface.appearance == choice;
            let mut change = interface.clone();
            change.appearance = choice;
            choices = choices.push(
                button(text(choice.title()).size(13))
                    .padding([5, 14])
                    .style(if on {
                        theme::row_selected
                    } else {
                        theme::tonal
                    })
                    .on_press(set(change)),
            );
        }
        row![text("Appearance").width(Fill), choices].align_y(Center)
    };
    let accent = {
        let mut swatches = row![].spacing(10).align_y(Center);
        for choice in Accent::ALL {
            let on = interface.accent == choice;
            let mut change = interface.clone();
            change.accent = choice;
            swatches = swatches.push(
                button(
                    container(space())
                        .width(Length::Fixed(22.0))
                        .height(Length::Fixed(22.0))
                        .style(move |t: &Theme| swatch(t, choice, on)),
                )
                .padding(2)
                .style(theme::row)
                .on_press(set(change)),
            );
        }
        row![
            text("Accent Colour").width(Fill),
            swatches,
            widgets::secondary(interface.accent.title())
                .size(13)
                .width(Length::Fixed(70.0)),
        ]
        .spacing(10)
        .align_y(Center)
    };
    column![
        text("General")
            .size(13)
            .font(widgets::SEMIBOLD)
            .style(|t| text::Style {
                color: Some(theme::secondary(t)),
            }),
        iced::widget::container(column![appearance, accent, title].spacing(12))
            .padding(14)
            .width(Fill)
            .style(theme::menu),
        widgets::secondary(
            "System follows your desktop's light or dark setting as it changes. While a song \
             plays, the window title reads \u{201c}Song \u{2014} Artist \u{b7} Aoide\u{201d}, \
             which is what task bars and window switchers show."
        )
        .size(12),
    ]
    .spacing(8)
    .into()
}

/// A round swatch in the accent's colour for the page's mode, ringed when
/// chosen.
fn swatch(theme: &Theme, accent: Accent, chosen: bool) -> container::Style {
    let dark = theme.extended_palette().is_dark;
    let colour = accent.for_mode(if dark { Mode::Dark } else { Mode::Light });
    container::Style {
        background: Some(Background::Color(colour)),
        border: Border {
            color: if chosen {
                theme.palette().text
            } else {
                Color::TRANSPARENT
            },
            width: if chosen { 2.0 } else { 0.0 },
            radius: 11.0.into(),
        },
        ..container::Style::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG 2's contrast ratio.
    fn contrast(a: Color, b: Color) -> f32 {
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
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn every_accent_reads_as_text_on_its_own_ground() {
        let light = theme::for_mode(Mode::Light).palette().background;
        let dark = theme::for_mode(Mode::Dark).palette().background;
        for accent in Accent::ALL {
            let (on_light, on_dark) = accent.colours();
            assert!(
                contrast(on_light, light) >= 4.5,
                "{accent:?} on light: {}",
                contrast(on_light, light)
            );
            assert!(
                contrast(on_dark, dark) >= 4.5,
                "{accent:?} on dark: {}",
                contrast(on_dark, dark)
            );
        }
    }

    #[test]
    fn the_chosen_mode_wins_over_the_system() {
        assert_eq!(Appearance::System.mode(Mode::Dark), Mode::Dark);
        assert_eq!(Appearance::System.mode(Mode::Light), Mode::Light);
        assert_eq!(Appearance::Light.mode(Mode::Dark), Mode::Light);
        assert_eq!(Appearance::Dark.mode(Mode::Light), Mode::Dark);
    }

    #[test]
    fn the_theme_carries_the_accent_for_its_mode() {
        let mut interface = InterfaceSettings {
            appearance: Appearance::Dark,
            accent: Accent::Teal,
            ..InterfaceSettings::default()
        };
        let dark = theme(Mode::Light, &interface);
        assert_eq!(dark.palette().primary, Accent::Teal.colours().1);
        assert_eq!(
            dark.palette().background,
            theme::for_mode(Mode::Dark).palette().background
        );
        interface.appearance = Appearance::System;
        let light = theme(Mode::Light, &interface);
        assert_eq!(light.palette().primary, Accent::Teal.colours().0);
        interface.accent = Accent::Violet;
        assert_eq!(
            theme(Mode::Dark, &interface).palette().primary,
            theme::ACCENT_DARK
        );
    }

    #[test]
    fn the_title_names_the_song_while_it_plays() {
        assert_eq!(
            window_title(true, Some(("Harbour Lights", "Mara Vey"))),
            "Harbour Lights \u{2014} Mara Vey \u{b7} Aoide"
        );
        assert_eq!(
            window_title(true, Some(("Lantern", ""))),
            "Lantern \u{b7} Aoide"
        );
        assert_eq!(window_title(true, None), "Aoide");
        assert_eq!(window_title(true, Some((" ", "Mara Vey"))), "Aoide");
    }

    #[test]
    fn the_setting_keeps_the_title_plain() {
        assert_eq!(
            window_title(false, Some(("Harbour Lights", "Mara Vey"))),
            "Aoide"
        );
    }

    #[test]
    fn it_is_on_by_default_and_round_trips() {
        let interface = InterfaceSettings::default();
        assert!(interface.track_in_title);
        assert!(interface.internet_lyrics);
        assert!(!interface.romaji && !interface.furigana);
        let changed = InterfaceSettings {
            track_in_title: false,
            appearance: Appearance::Light,
            accent: Accent::Orange,
            internet_lyrics: false,
            romaji: true,
            furigana: true,
        };
        let json = serde_json::to_string(&changed).unwrap();
        let back: InterfaceSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, changed);
        let empty: InterfaceSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, InterfaceSettings::default());
    }
}
