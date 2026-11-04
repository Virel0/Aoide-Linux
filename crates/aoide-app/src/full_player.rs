//! The full-screen player: the cover blurred across the whole window, and a
//! frosted card holding the cover (still or moving), the song, the seek bar
//! and the transport. Three layouts, which V goes through: the card beside
//! the synced lyrics, the card alone, and a wide card with the song large.
//! It is always dark, whatever the app's mode, as its words sit on the
//! darkened cover. The window goes full screen with it; Esc leaves.

mod backdrop;
mod glass;
mod lyrics;

use iced::keyboard::{self, Key, key::Named};
use iced::theme::Mode;
use iced::widget::{Row, button, column, container, image, row, space, stack, text, themer};
use iced::{Center, ContentFit, Element, Fill, Subscription, Task, window};

use crate::app::{App, Message, Signed};
use crate::artwork::ArtKey;
use crate::icons::{Icon, Tone, icon};
use crate::listening::Msg;
use crate::{theme, widgets};

/// How the player is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    /// The card beside the lyrics; the card alone when the song has none.
    #[default]
    Lyrics,
    Card,
    Wide,
}

impl Layout {
    const ALL: [Self; 3] = [Self::Lyrics, Self::Card, Self::Wide];

    /// The layout V moves to.
    pub fn next(self) -> Self {
        match self {
            Self::Lyrics => Self::Card,
            Self::Card => Self::Wide,
            Self::Wide => Self::Lyrics,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Lyrics => "Lyrics",
            Self::Card => "Card",
            Self::Wide => "Wide",
        }
    }
}

#[derive(Debug, Default)]
pub struct FullPlayer {
    pub layout: Layout,
    /// The window went full screen for this player (it wasn't already), so
    /// leaving puts it back.
    pub made_fullscreen: bool,
    /// The cover the blurred ground was last asked for.
    backdrop_for: Option<ArtKey>,
    /// That cover, blurred, once made.
    backdrop: Option<image::Handle>,
}

impl App {
    pub(crate) fn open_full_player(&mut self) -> Task<Message> {
        self.listening.menu = None;
        if self.listening.full.is_some() {
            return Task::none();
        }
        self.listening.full = Some(FullPlayer::default());
        // Full screen only from a window: one the listener already made full
        // screen is left as it is, and stays so afterwards.
        let fullscreen = window::latest().and_then(|id| {
            window::mode(id).then(move |mode| {
                if mode == window::Mode::Windowed {
                    window::set_mode::<Message>(id, window::Mode::Fullscreen)
                        .chain(Task::done(Message::Listening(Msg::MadeFullscreen)))
                } else {
                    Task::none()
                }
            })
        });
        // The lyrics layout needs them, whether or not the panel is open.
        Task::batch([fullscreen, self.track_changed()])
    }

    pub(crate) fn close_full_player(&mut self) -> Task<Message> {
        match self.listening.full.take() {
            Some(full) if full.made_fullscreen => {
                window::latest().and_then(|id| window::set_mode(id, window::Mode::Windowed))
            }
            _ => Task::none(),
        }
    }

    /// Esc and V, while the full player is open.
    pub(crate) fn full_player_subscription(&self) -> Subscription<Message> {
        if self.listening.full.is_none() {
            return Subscription::none();
        }
        keyboard::listen().filter_map(|event| match event {
            keyboard::Event::KeyPressed { key, modifiers, .. } => key_message(&key, modifiers),
            _ => None,
        })
    }
}

/// What a key does in the full player: Esc leaves, V (alone) changes the
/// layout.
fn key_message(key: &Key, modifiers: keyboard::Modifiers) -> Option<Message> {
    match key.as_ref() {
        Key::Named(Named::Escape) => Some(Message::Listening(Msg::CloseFull)),
        Key::Character(c)
            if c.eq_ignore_ascii_case("v")
                && !modifiers.command()
                && !modifiers.control()
                && !modifiers.alt() =>
        {
            Some(Message::Listening(Msg::NextLayout))
        }
        _ => None,
    }
}

pub fn view<'a>(app: &'a App, signed: &'a Signed, full: &'a FullPlayer) -> Element<'a, Message> {
    let current = app.playback.current().map(|(_, q)| &q.meta);
    let middle: Element<'a, Message> = match full.layout {
        Layout::Lyrics => match lyrics::side(app, signed, current) {
            Some(side) => row![
                glass::card(app, signed, current),
                container(side).width(Fill).max_width(820.0),
            ]
            .spacing(72)
            .padding([0, 48])
            .align_y(Center)
            .into(),
            None => glass::card(app, signed, current),
        },
        Layout::Card => glass::card(app, signed, current),
        Layout::Wide => glass::wide(app, signed, current),
    };
    let page = stack![
        ground(full),
        column![top_bar(full), container(middle).center(Fill)]
            .spacing(10)
            .padding(24),
    ];
    let dark = crate::appearance::in_mode(Mode::Dark, &app.config.interface);
    themer(Some(dark), page).into()
}

/// The blurred cover across the window, or the plain dark page until it is
/// made.
fn ground(full: &FullPlayer) -> Element<'_, Message> {
    let picture: Element<'_, Message> = match &full.backdrop {
        Some(blurred) => image(blurred.clone())
            .width(Fill)
            .height(Fill)
            .content_fit(ContentFit::Cover)
            .filter_method(image::FilterMethod::Linear)
            .into(),
        None => space().width(Fill).height(Fill).into(),
    };
    container(picture)
        .width(Fill)
        .height(Fill)
        .style(theme::page)
        .into()
}

/// Close, the hint, and the three layouts.
fn top_bar(full: &FullPlayer) -> Element<'_, Message> {
    let switch = |layout: Layout| {
        let on = full.layout == layout;
        Element::from(
            button(text(layout.label()).size(13))
                .padding([6, 12])
                .style(if on { theme::row_selected } else { theme::row })
                .on_press(Message::Listening(Msg::FullLayout(layout))),
        )
    };
    row![
        button(
            row![icon(Icon::Down, 18.0, Tone::Normal), text("Close").size(13)]
                .spacing(6)
                .align_y(Center)
        )
        .padding([6, 12])
        .style(theme::row)
        .on_press(Message::Listening(Msg::CloseFull)),
        crate::shell::gap(),
        widgets::secondary("V changes the layout \u{b7} Esc leaves").size(11),
        Row::with_children(Layout::ALL.map(switch)).spacing(6),
    ]
    .spacing(12)
    .align_y(Center)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v_goes_through_the_three_layouts_and_back() {
        let mut layout = Layout::default();
        let mut seen = vec![layout];
        for _ in 0..3 {
            layout = layout.next();
            seen.push(layout);
        }
        assert_eq!(
            seen,
            [Layout::Lyrics, Layout::Card, Layout::Wide, Layout::Lyrics]
        );
    }

    #[test]
    fn v_alone_changes_the_layout_and_esc_leaves() {
        let none = keyboard::Modifiers::empty();
        let v = Key::Character("v".into());
        assert!(matches!(
            key_message(&v, none),
            Some(Message::Listening(Msg::NextLayout))
        ));
        assert!(matches!(
            key_message(&Key::Character("V".into()), keyboard::Modifiers::SHIFT),
            Some(Message::Listening(Msg::NextLayout))
        ));
        // Ctrl+V and the like are someone else's.
        assert!(key_message(&v, keyboard::Modifiers::CTRL).is_none());
        assert!(key_message(&v, keyboard::Modifiers::ALT).is_none());
        assert!(matches!(
            key_message(&Key::Named(Named::Escape), none),
            Some(Message::Listening(Msg::CloseFull))
        ));
        assert!(key_message(&Key::Character("x".into()), none).is_none());
    }
}
