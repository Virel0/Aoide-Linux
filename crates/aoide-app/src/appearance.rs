//! Settings → General: how the window looks and what its title says.

use iced::widget::{column, text, toggler};
use iced::{Element, Fill};
use serde::{Deserialize, Serialize};

use crate::app::{App, Message};
use crate::listening::{Msg, SettingsExtras};
use crate::{theme, widgets};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct InterfaceSettings {
    /// "Title — Artist · Aoide" in the window title while playing.
    pub track_in_title: bool,
}

impl Default for InterfaceSettings {
    fn default() -> Self {
        Self {
            track_in_title: true,
        }
    }
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

    pub(crate) fn interface_changed(&mut self, interface: InterfaceSettings) {
        self.config.interface = interface;
        self.save_config();
    }
}

/// Settings → General.
pub fn general_section<'a>(extras: &SettingsExtras<'a>) -> Element<'a, Message> {
    let interface = extras.config.interface.clone();
    let set = move |change: InterfaceSettings| Message::Listening(Msg::Interface(change));
    let title = {
        let interface = interface.clone();
        toggler(interface.track_in_title)
            .label("Show the Song in the Window Title")
            .width(Fill)
            .on_toggle(move |on| {
                let mut change = interface.clone();
                change.track_in_title = on;
                set(change)
            })
    };
    column![
        text("General")
            .size(13)
            .font(widgets::SEMIBOLD)
            .style(|t| text::Style {
                color: Some(theme::secondary(t)),
            }),
        iced::widget::container(column![title].spacing(12))
            .padding(14)
            .width(Fill)
            .style(theme::menu),
        widgets::secondary(
            "While a song plays, the window title reads \u{201c}Song \u{2014} Artist \u{b7} \
             Aoide\u{201d}, which is what task bars and window switchers show."
        )
        .size(12),
    ]
    .spacing(8)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let json = serde_json::to_string(&InterfaceSettings {
            track_in_title: false,
        })
        .unwrap();
        let back: InterfaceSettings = serde_json::from_str(&json).unwrap();
        assert!(!back.track_in_title);
        let empty: InterfaceSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, InterfaceSettings::default());
    }
}
