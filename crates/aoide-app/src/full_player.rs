//! The full-screen player: the cover blurred across the whole window (or a
//! ground moving in its colours), and a frosted card holding the cover
//! (still or moving), the song, the seek bar and the transport. Three
//! layouts, which V goes through: the card beside the synced lyrics, the card
//! alone, and a wide card with the song large; B goes through the grounds.
//! Both are remembered. The controls and the pointer hide after a few still
//! seconds while the music plays. It is always dark, whatever the app's
//! mode, as its words sit on the darkened cover. The window goes full
//! screen with it; Esc leaves.

mod backdrop;
mod clock;
mod glass;
mod ground;
mod lyrics;
mod palette;

use std::time::Instant;

use iced::theme::Mode;
use iced::widget::{
    Row, button, column, container, image, mouse_area, row, shader, space, stack, text, themer,
};
use iced::{Center, ContentFit, Element, Fill, Length, Task, mouse, window};
use serde::{Deserialize, Serialize};

use crate::app::{App, Message, Signed};
use crate::artwork::ArtKey;
use crate::icons::{Icon, Tone, icon};
use crate::listening::Msg;
use crate::{theme, widgets};

pub use backdrop::Made;

/// How the player is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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

/// What lies behind the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Ground {
    /// The blurred cover, still.
    Still,
    /// The cover's colours folding slowly over each other.
    #[default]
    Silk,
    /// Curtains of its colours drifting over its darkest.
    Aurora,
    /// Clouds of its colours spreading in dark water.
    Ink,
}

impl Ground {
    /// The ground B moves to.
    pub fn next(self) -> Self {
        match self {
            Self::Still => Self::Silk,
            Self::Silk => Self::Aurora,
            Self::Aurora => Self::Ink,
            Self::Ink => Self::Still,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Still => "Still",
            Self::Silk => "Silk",
            Self::Aurora => "Aurora",
            Self::Ink => "Ink",
        }
    }

    pub fn moves(self) -> bool {
        self != Self::Still
    }
}

#[derive(Debug)]
pub struct FullPlayer {
    /// The window went full screen for this player (it wasn't already), so
    /// leaving puts it back.
    pub made_fullscreen: bool,
    /// The cover the ground was last asked for.
    backdrop_for: Option<ArtKey>,
    /// That cover, blurred, and its colours, once made.
    backdrop: Option<backdrop::Made>,
    /// Seconds the moving ground has moved: only while the music plays.
    clock: f32,
    last_tick: Option<Instant>,
    /// When the pointer last moved or a key was pressed.
    last_stirred: Instant,
    /// The controls and the pointer are hidden (only while playing).
    still: bool,
    focused: bool,
}

impl FullPlayer {
    fn new(now: Instant) -> Self {
        Self {
            made_fullscreen: false,
            backdrop_for: None,
            backdrop: None,
            clock: 0.0,
            last_tick: None,
            last_stirred: now,
            still: false,
            focused: true,
        }
    }
}

impl App {
    pub(crate) fn open_full_player(&mut self) -> Task<Message> {
        self.listening.menu = None;
        if self.listening.full.is_some() {
            return Task::none();
        }
        self.listening.full = Some(FullPlayer::new(Instant::now()));
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

    /// A layout chosen, kept for next time.
    pub(crate) fn set_full_layout(&mut self, layout: Layout) -> Task<Message> {
        self.config.interface.full_layout = layout;
        self.save_config();
        Task::none()
    }

    /// A ground chosen, kept for next time.
    pub(crate) fn set_full_ground(&mut self, ground: Ground) -> Task<Message> {
        self.config.interface.full_ground = ground;
        self.save_config();
        Task::none()
    }
}

pub fn view<'a>(app: &'a App, signed: &'a Signed, full: &'a FullPlayer) -> Element<'a, Message> {
    let current = app.playback.current().map(|(_, q)| &q.meta);
    let controls = !full.still || !app.playback.is_playing();
    let interface = &app.config.interface;
    let middle: Element<'a, Message> = match interface.full_layout {
        Layout::Lyrics => match lyrics::side(app, signed, current) {
            Some(side) => row![
                glass::card(app, signed, current, controls),
                container(side).width(Fill).max_width(820.0),
            ]
            .spacing(72)
            .padding([0, 48])
            .align_y(Center)
            .into(),
            None => glass::card(app, signed, current, controls),
        },
        Layout::Card => glass::card(app, signed, current, controls),
        Layout::Wide => glass::wide(app, signed, current, controls),
    };
    let top: Element<'a, Message> = if controls {
        top_bar(interface.full_layout, interface.full_ground)
    } else {
        space().height(TOP_BAR).into()
    };
    let page = stack![
        ground(full, interface.full_ground),
        column![top, container(middle).center(Fill)]
            .spacing(10)
            .padding(24),
    ];
    let page = mouse_area(page).interaction(if controls {
        mouse::Interaction::None
    } else {
        mouse::Interaction::Hidden
    });
    let dark = crate::appearance::in_mode(Mode::Dark, interface);
    themer(Some(dark), page).into()
}

/// The top bar's height, kept while it is hidden so nothing moves.
const TOP_BAR: f32 = 34.0;

/// The blurred cover across the window, with the moving ground over it when
/// one is chosen; the plain dark page until the cover is made.
fn ground(full: &FullPlayer, chosen: Ground) -> Element<'_, Message> {
    let Some(made) = &full.backdrop else {
        return container(space().width(Fill).height(Fill))
            .style(theme::page)
            .into();
    };
    let still = image(made.still.clone())
        .width(Fill)
        .height(Fill)
        .content_fit(ContentFit::Cover)
        .filter_method(image::FilterMethod::Linear);
    if !chosen.moves() {
        return still.into();
    }
    let moving = shader(ground::Program {
        ground: chosen,
        palette: made.palette,
        clock: full.clock,
    })
    .width(Fill)
    .height(Fill);
    stack![still, moving].into()
}

/// Close, the hint, the three layouts and the ground.
fn top_bar<'a>(layout: Layout, chosen: Ground) -> Element<'a, Message> {
    let switch = |each: Layout| {
        Element::from(
            button(text(each.label()).size(13))
                .padding([6, 12])
                .style(if each == layout {
                    theme::row_selected
                } else {
                    theme::row
                })
                .on_press(Message::Listening(Msg::FullLayout(each))),
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
        widgets::secondary("V layout \u{b7} B background \u{b7} Esc leaves").size(11),
        Row::with_children(Layout::ALL.map(switch)).spacing(6),
        button(text(format!("Background: {}", chosen.label())).size(13))
            .padding([6, 12])
            .style(theme::row)
            .on_press(Message::Listening(Msg::FullGround(chosen.next()))),
    ]
    .spacing(12)
    .height(Length::Fixed(TOP_BAR))
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
    fn b_goes_through_the_grounds_and_only_still_stands_still() {
        let mut ground = Ground::Still;
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(ground);
            ground = ground.next();
        }
        assert_eq!(ground, Ground::Still);
        assert_eq!(
            seen,
            [Ground::Still, Ground::Silk, Ground::Aurora, Ground::Ink]
        );
        assert_eq!(
            seen.iter().filter(|g| g.moves()).count(),
            3,
            "every ground but Still moves"
        );
    }

    #[test]
    fn the_choices_are_kept_by_name() {
        assert_eq!(serde_json::to_string(&Layout::Wide).unwrap(), "\"wide\"");
        assert_eq!(
            serde_json::to_string(&Ground::Aurora).unwrap(),
            "\"aurora\""
        );
        let back: Ground = serde_json::from_str("\"ink\"").unwrap();
        assert_eq!(back, Ground::Ink);
    }
}
