//! The app's icons: small SVGs drawn for Aoide, tinted from the theme so they
//! follow light and dark.

use std::sync::LazyLock;

use iced::widget::svg;
use iced::{Element, Length, Theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    Play,
    Pause,
    Next,
    Previous,
    Shuffle,
    Repeat,
    RepeatOne,
    Volume,
    Queue,
    Lyrics,
    Crossfade,
    Infinity,
    Heart,
    HeartFilled,
    More,
    Back,
    Search,
    Home,
    Album,
    Artist,
    Song,
    Genre,
    Playlist,
    Settings,
    Close,
    Up,
    Down,
    Remove,
    Plus,
    Sparkle,
    Radio,
    Folder,
}

/// How an icon is tinted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    Accent,
    Secondary,
}

const STROKE: &str =
    r#"fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round""#;

fn source(icon: Icon) -> String {
    let body = match icon {
        Icon::Play => r#"<path d="M7 4.5v15l12.5-7.5z" fill="black"/>"#.to_owned(),
        Icon::Pause => {
            r#"<rect x="6" y="4.5" width="4" height="15" rx="1" fill="black"/><rect x="14" y="4.5" width="4" height="15" rx="1" fill="black"/>"#.to_owned()
        }
        Icon::Next => {
            r#"<path d="M4.5 5.5v13l9.5-6.5z" fill="black"/><rect x="15.5" y="5.5" width="3" height="13" rx="1" fill="black"/>"#.to_owned()
        }
        Icon::Previous => {
            r#"<rect x="5.5" y="5.5" width="3" height="13" rx="1" fill="black"/><path d="M19.5 5.5v13L10 12z" fill="black"/>"#.to_owned()
        }
        Icon::Shuffle => format!(
            r#"<path d="M3 7h3.5c2 0 3 1 4.5 3l2 3c1.5 2 2.5 3 4.5 3H21M3 17h3.5c1.4 0 2.3-.5 3.2-1.5M13.5 8.5c.9-1 1.8-1.5 3.2-1.5H21M18 4l3 3-3 3M18 13l3 3-3 3" {STROKE}/>"#
        ),
        Icon::Repeat => format!(
            r#"<path d="M4 12V9.5A3.5 3.5 0 0 1 7.5 6H20M17 3l3 3-3 3M20 12v2.5a3.5 3.5 0 0 1-3.5 3.5H4M7 21l-3-3 3-3" {STROKE}/>"#
        ),
        Icon::RepeatOne => format!(
            r#"<path d="M4 12V9.5A3.5 3.5 0 0 1 7.5 6H20M17 3l3 3-3 3M20 12v2.5a3.5 3.5 0 0 1-3.5 3.5H4M7 21l-3-3 3-3M11 10.5l1.5-1v5" {STROKE}/>"#
        ),
        Icon::Volume => format!(
            r#"<path d="M4 9.5h3.5L12 6v12l-4.5-3.5H4z" fill="black"/><path d="M15.5 9a4 4 0 0 1 0 6M18 6.5a7.5 7.5 0 0 1 0 11" {STROKE}/>"#
        ),
        Icon::Queue => format!(
            r#"<path d="M4 6h13M4 11h13M4 16h7" {STROKE}/><path d="M15 14.5v6l5-3z" fill="black"/>"#
        ),
        Icon::Lyrics => format!(
            r#"<path d="M5 5h14a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1h-8l-4.5 3.5V16H5a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1zM8 9h8M8 12h5" {STROKE}/>"#
        ),
        Icon::Crossfade => format!(
            r#"<path d="M3 17c6 0 9-10 18-10M3 7c9 0 12 10 18 10" {STROKE}/>"#
        ),
        Icon::Infinity => format!(
            r#"<path d="M12 12c-2-2.7-3.5-4-5.5-4a4 4 0 0 0 0 8c2 0 3.5-1.3 5.5-4zm0 0c2 2.7 3.5 4 5.5 4a4 4 0 0 0 0-8c-2 0-3.5 1.3-5.5 4z" {STROKE}/>"#
        ),
        Icon::Heart => format!(
            r#"<path d="M12 19.5S4.5 15 4.5 9.5A3.8 3.8 0 0 1 12 7.6a3.8 3.8 0 0 1 7.5 1.9c0 5.5-7.5 10-7.5 10z" {STROKE}/>"#
        ),
        Icon::HeartFilled => {
            r#"<path d="M12 19.5S4.5 15 4.5 9.5A3.8 3.8 0 0 1 12 7.6a3.8 3.8 0 0 1 7.5 1.9c0 5.5-7.5 10-7.5 10z" fill="black"/>"#.to_owned()
        }
        Icon::More => {
            r#"<circle cx="6" cy="12" r="1.7" fill="black"/><circle cx="12" cy="12" r="1.7" fill="black"/><circle cx="18" cy="12" r="1.7" fill="black"/>"#.to_owned()
        }
        Icon::Back => format!(r#"<path d="M15 5l-7 7 7 7" {STROKE}/>"#),
        Icon::Search => format!(r#"<circle cx="10.5" cy="10.5" r="6" {STROKE}/><path d="M15 15l5 5" {STROKE}/>"#),
        Icon::Home => format!(r#"<path d="M4 11l8-6.5 8 6.5V19a1 1 0 0 1-1 1h-4.5v-5h-5v5H5a1 1 0 0 1-1-1z" {STROKE}/>"#),
        Icon::Album => format!(
            r#"<rect x="4" y="4" width="16" height="16" rx="2" {STROKE}/><circle cx="12" cy="12" r="3.5" {STROKE}/>"#
        ),
        Icon::Artist => format!(
            r#"<circle cx="12" cy="8.5" r="3.5" {STROKE}/><path d="M5 20c.8-3.8 3.6-6 7-6s6.2 2.2 7 6" {STROKE}/>"#
        ),
        Icon::Song => format!(
            r#"<path d="M9 18V6l10-2v12" {STROKE}/><circle cx="6.5" cy="18" r="2.5" {STROKE}/><circle cx="16.5" cy="16" r="2.5" {STROKE}/>"#
        ),
        Icon::Genre => format!(
            r#"<path d="M4 12.5V5a1 1 0 0 1 1-1h7.5L20 11.5 12.5 19z" {STROKE}/><circle cx="8.5" cy="8.5" r="1.3" fill="black"/>"#
        ),
        Icon::Playlist => format!(
            r#"<path d="M4 6h11M4 11h11M4 16h6" {STROKE}/><path d="M17 18V9l3.5-1" {STROKE}/><circle cx="15.5" cy="18" r="1.8" {STROKE}/>"#
        ),
        Icon::Settings => format!(
            r#"<path d="M4 7h9M17 7h3M4 17h3M11 17h9" {STROKE}/><circle cx="15" cy="7" r="2" {STROKE}/><circle cx="9" cy="17" r="2" {STROKE}/>"#
        ),
        Icon::Close => format!(r#"<path d="M6 6l12 12M18 6L6 18" {STROKE}/>"#),
        Icon::Up => format!(r#"<path d="M6 15l6-6 6 6" {STROKE}/>"#),
        Icon::Down => format!(r#"<path d="M6 9l6 6 6-6" {STROKE}/>"#),
        Icon::Remove => format!(r#"<path d="M6 12h12" {STROKE}/>"#),
        Icon::Plus => format!(r#"<path d="M12 6v12M6 12h12" {STROKE}/>"#),
        Icon::Sparkle => r#"<path d="M10 3l1.9 5.6L17.5 10.5l-5.6 1.9L10 18l-1.9-5.6L2.5 10.5l5.6-1.9z" fill="black"/><path d="M18 14l.9 2.6 2.6.9-2.6.9L18 21l-.9-2.6-2.6-.9 2.6-.9z" fill="black"/>"#.to_owned(),
        Icon::Folder => format!(
            r#"<path d="M3.5 7.5a2 2 0 0 1 2-2h4l2 2h7a2 2 0 0 1 2 2v7.5a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z" {STROKE}/>"#
        ),
        Icon::Radio => format!(
            r#"<circle cx="12" cy="12" r="2" fill="black"/><path d="M8.5 8.5a5 5 0 0 0 0 7M15.5 8.5a5 5 0 0 1 0 7M5.5 5.5a9.2 9.2 0 0 0 0 13M18.5 5.5a9.2 9.2 0 0 1 0 13" {STROKE}/>"#
        ),
    };
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">{body}</svg>"#)
}

const ALL: [Icon; 32] = [
    Icon::Play,
    Icon::Pause,
    Icon::Next,
    Icon::Previous,
    Icon::Shuffle,
    Icon::Repeat,
    Icon::RepeatOne,
    Icon::Volume,
    Icon::Queue,
    Icon::Lyrics,
    Icon::Crossfade,
    Icon::Infinity,
    Icon::Heart,
    Icon::HeartFilled,
    Icon::More,
    Icon::Back,
    Icon::Search,
    Icon::Home,
    Icon::Album,
    Icon::Artist,
    Icon::Song,
    Icon::Genre,
    Icon::Playlist,
    Icon::Settings,
    Icon::Close,
    Icon::Up,
    Icon::Down,
    Icon::Remove,
    Icon::Plus,
    Icon::Sparkle,
    Icon::Radio,
    Icon::Folder,
];

/// One handle per icon, made once: a handle's id is what the renderer caches on.
static HANDLES: LazyLock<Vec<(Icon, svg::Handle)>> = LazyLock::new(|| {
    ALL.iter()
        .map(|&icon| (icon, svg::Handle::from_memory(source(icon).into_bytes())))
        .collect()
});

fn handle(icon: Icon) -> svg::Handle {
    HANDLES
        .iter()
        .find(|(i, _)| *i == icon)
        .map(|(_, h)| h.clone())
        .unwrap_or_else(|| svg::Handle::from_memory(source(icon).into_bytes()))
}

/// The icon at `size` pixels, tinted.
pub fn icon<'a, Message: 'a>(icon: Icon, size: f32, tone: Tone) -> Element<'a, Message> {
    svg(handle(icon))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(move |theme: &Theme, _status| svg::Style {
            color: Some(match tone {
                Tone::Normal => theme.palette().text,
                Tone::Accent => crate::theme::accent(theme),
                Tone::Secondary => crate::theme::secondary(theme),
            }),
        })
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_a_whole_svg() {
        for icon in ALL {
            let svg = source(icon);
            assert!(
                svg.starts_with("<svg") && svg.ends_with("</svg>"),
                "{icon:?}"
            );
            assert_eq!(
                svg.matches('<').count(),
                svg.matches('>').count(),
                "{icon:?}"
            );
        }
    }
}
