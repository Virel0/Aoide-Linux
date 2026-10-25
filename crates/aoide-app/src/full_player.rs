//! The full-screen player: the cover large (or the visualizer in its place),
//! the song, the seek bar and the transport, over the whole window, which
//! goes full screen with it. Esc leaves.
//!
//! The visualizer reads the engine's tap (`aoide_audio::Visualizer`) about
//! 30 times a second, and only while it is on screen, something is playing
//! and the window isn't minimised: otherwise nothing ticks at all.

use std::time::Duration;

use aoide_audio::{Visualizer, VisualizerConfig};
use iced::widget::{button, canvas, column, container, row, slider, text};
use iced::{
    Center, Element, Fill, Length, Point, Rectangle, Renderer, Size, Subscription, Task, Theme,
    keyboard, mouse, window,
};

use crate::app::{App, Message, Signed, Transport};
use crate::artwork::ArtKey;
use crate::icons::{Icon, Tone, icon};
use crate::listening::Msg;
use crate::{theme, widgets};

/// About 30 frames a second.
pub const FRAME: Duration = Duration::from_millis(33);
/// How many bars the visualizer draws.
pub const BARS: usize = 48;

/// What fills the middle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Show {
    #[default]
    Artwork,
    Visualizer,
}

#[derive(Debug, Default)]
pub struct FullPlayer {
    pub show: Show,
    /// The bars as last read, 0 to 1, low to high.
    pub bars: Vec<f32>,
    /// The window is minimised (asked while it is out of focus).
    pub hidden: bool,
    pub focused: bool,
    /// The window went full screen for this player (it wasn't already), so
    /// leaving puts it back.
    pub made_fullscreen: bool,
    reader: Option<Reader>,
}

/// The visualizer, made the first time it is shown.
struct Reader(Visualizer);

impl std::fmt::Debug for Reader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Visualizer")
    }
}

impl FullPlayer {
    pub fn new() -> Self {
        Self {
            focused: true,
            ..Self::default()
        }
    }

    /// Whether the visualizer should be read now: it is showing, the
    /// window can be seen, and the music is moving.
    pub fn animates(&self, playing: bool) -> bool {
        self.show == Show::Visualizer && !self.hidden && playing
    }
}

/// The bars' rectangles in a `size` canvas: `levels` side by side with a gap
/// a third of a bar, each rising from the bottom by its level (a sliver for
/// silence, so the shape stays).
pub fn bar_rects(levels: &[f32], size: Size) -> Vec<Rectangle> {
    if levels.is_empty() || size.width <= 0.0 || size.height <= 0.0 {
        return Vec::new();
    }
    let n = levels.len() as f32;
    // n bars and n − 1 gaps of a third: width = n·w + (n − 1)·w/3.
    let bar = size.width / (n + (n - 1.0) / 3.0);
    let gap = bar / 3.0;
    levels
        .iter()
        .enumerate()
        .map(|(i, level)| {
            let height = (level.clamp(0.0, 1.0) * size.height).max(2.0);
            Rectangle {
                x: i as f32 * (bar + gap),
                y: size.height - height,
                width: bar,
                height,
            }
        })
        .collect()
}

impl App {
    pub(crate) fn open_full_player(&mut self) -> Task<Message> {
        self.listening.menu = None;
        if self.listening.full.is_some() {
            return Task::none();
        }
        self.listening.full = Some(FullPlayer::new());
        // Full screen only from a window: one the listener already made full
        // screen is left as it is, and stays so afterwards.
        window::latest().and_then(|id| {
            window::mode(id).then(move |mode| {
                if mode == window::Mode::Windowed {
                    window::set_mode::<Message>(id, window::Mode::Fullscreen)
                        .chain(Task::done(Message::Listening(Msg::MadeFullscreen)))
                } else {
                    Task::none()
                }
            })
        })
    }

    pub(crate) fn close_full_player(&mut self) -> Task<Message> {
        match self.listening.full.take() {
            Some(full) if full.made_fullscreen => {
                window::latest().and_then(|id| window::set_mode(id, window::Mode::Windowed))
            }
            _ => Task::none(),
        }
    }

    /// One visualizer frame.
    pub(crate) fn visual_frame(&mut self) {
        let tap = self.playback.visualizer_tap();
        let Some(full) = self.listening.full.as_mut() else {
            return;
        };
        if full.reader.is_none() {
            let Some(tap) = tap else {
                return;
            };
            full.reader = Some(Reader(Visualizer::new(
                tap,
                VisualizerConfig {
                    bands: BARS,
                    ..VisualizerConfig::default()
                },
            )));
        }
        if let Some(Reader(visualizer)) = full.reader.as_mut() {
            let frame = visualizer.read();
            full.bars.clear();
            full.bars.extend_from_slice(frame.levels);
        }
    }

    /// The full player's clock, keys and window watch, while it is open.
    pub(crate) fn full_player_subscription(&self) -> Subscription<Message> {
        let Some(full) = &self.listening.full else {
            return Subscription::none();
        };
        let mut subscriptions = vec![
            keyboard::listen().filter_map(|event| match event {
                keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                } => Some(Message::Listening(Msg::CloseFull)),
                _ => None,
            }),
            window::events().filter_map(|(_, event)| match event {
                window::Event::Focused => Some(Message::Listening(Msg::WindowFocus(true))),
                window::Event::Unfocused => Some(Message::Listening(Msg::WindowFocus(false))),
                _ => None,
            }),
        ];
        if full.animates(self.playback.is_playing()) {
            subscriptions
                .push(iced::time::every(FRAME).map(|_| Message::Listening(Msg::VisualFrame)));
        }
        // Out of focus it may be minimised, which no event says: ask now
        // and then, at a rate that costs nothing.
        if full.show == Show::Visualizer && !full.focused {
            subscriptions.push(
                iced::time::every(Duration::from_secs(1))
                    .map(|_| Message::Listening(Msg::CheckHidden)),
            );
        }
        Subscription::batch(subscriptions)
    }

    pub(crate) fn check_hidden(&self) -> Task<Message> {
        window::latest().and_then(|id| {
            window::is_minimized(id)
                .map(|minimised| Message::Listening(Msg::WindowHidden(minimised.unwrap_or(false))))
        })
    }
}

pub fn view<'a>(app: &'a App, signed: &'a Signed, full: &'a FullPlayer) -> Element<'a, Message> {
    let playback = &app.playback;
    let current = playback.current().map(|(_, q)| &q.meta);
    let listen = |m: Msg| Message::Listening(m);

    let switch = |label: &'a str, glyph: Icon, which: Show| {
        let on = full.show == which;
        button(
            row![
                icon(glyph, 16.0, if on { Tone::Accent } else { Tone::Secondary }),
                text(label).size(13)
            ]
            .spacing(6)
            .align_y(Center),
        )
        .padding([6, 12])
        .style(if on { theme::row_selected } else { theme::row })
        .on_press(listen(Msg::FullShow(which)))
    };
    let top = row![
        button(
            row![icon(Icon::Down, 18.0, Tone::Normal), text("Close").size(13)]
                .spacing(6)
                .align_y(Center)
        )
        .padding([6, 12])
        .style(theme::row)
        .on_press(listen(Msg::CloseFull)),
        crate::shell::gap(),
        switch("Artwork", Icon::Artwork, Show::Artwork),
        switch("Visualizer", Icon::Visualizer, Show::Visualizer),
    ]
    .spacing(6)
    .align_y(Center);

    let middle: Element<'a, Message> = match full.show {
        Show::Artwork => widgets::art(
            current
                .and_then(|m| m.artwork.as_ref())
                .map(|(i, t)| ArtKey::new(i, t))
                .as_ref(),
            &app.artwork,
            Length::Fixed(420.0),
            14.0,
        ),
        Show::Visualizer => canvas(Bars {
            levels: if full.bars.is_empty() {
                vec![0.0; BARS]
            } else {
                full.bars.clone()
            },
            dimmed: !playback.is_playing(),
        })
        .width(Length::Fixed(760.0))
        .height(Length::Fixed(420.0))
        .into(),
    };

    let subtitle = current.map_or_else(String::new, |m| {
        if m.album.is_empty() {
            m.artist.clone()
        } else {
            format!("{} \u{2014} {}", m.artist, m.album)
        }
    });
    let words = column![
        text(current.map_or("Not Playing", |m| m.title.as_str()))
            .size(30)
            .font(widgets::BOLD),
        widgets::secondary(subtitle).size(17),
    ]
    .spacing(6)
    .align_x(Center);

    let length = current.and_then(|m| m.duration_ms).unwrap_or(0).max(0) as u64;
    let at = signed
        .scrub
        .unwrap_or(playback.position_ms())
        .min(length.max(1));
    let seek: Element<'a, Message> = if length > 0 {
        slider(0.0..=length as f32, at as f32, |v| {
            Message::Player(Transport::Scrub(v))
        })
        .on_release(Message::Player(Transport::ScrubEnd))
        .step(250.0_f32)
        .style(theme::scrubber)
        .into()
    } else {
        slider(0.0..=1.0, 0.0, |_| Message::Ignore)
            .style(theme::scrubber)
            .into()
    };
    let clock = |s: String| widgets::secondary(s).size(12).width(48);
    let scrubber = row![
        clock(widgets::duration(at)),
        seek,
        clock(format!("-{}", widgets::duration(length.saturating_sub(at))))
    ]
    .spacing(10)
    .align_y(Center)
    .width(Length::Fixed(640.0));

    let control = |glyph: Icon, size: f32, on: bool, message: Transport| {
        button(icon(
            glyph,
            size,
            if on { Tone::Accent } else { Tone::Normal },
        ))
        .padding(10)
        .style(if on { theme::icon_on } else { theme::row })
        .on_press(Message::Player(message))
    };
    let repeat = playback.repeat();
    let transport = row![
        control(
            Icon::Shuffle,
            20.0,
            playback.is_shuffled(),
            Transport::Shuffle
        ),
        control(Icon::Previous, 28.0, false, Transport::Previous),
        control(
            if playback.is_playing() {
                Icon::Pause
            } else {
                Icon::Play
            },
            40.0,
            false,
            Transport::Toggle
        ),
        control(Icon::Next, 28.0, false, Transport::Next),
        control(
            if repeat == aoide_audio::RepeatMode::One {
                Icon::RepeatOne
            } else {
                Icon::Repeat
            },
            20.0,
            repeat != aoide_audio::RepeatMode::Off,
            Transport::Repeat
        ),
    ]
    .spacing(18)
    .align_y(Center);

    let body = column![
        container(middle).center_x(Fill),
        words,
        container(scrubber).center_x(Fill),
        transport,
        widgets::secondary("Esc to leave").size(11),
    ]
    .spacing(26)
    .align_x(Center)
    .width(Fill);

    container(
        column![top, container(body).center(Fill)]
            .spacing(10)
            .padding(24),
    )
    .width(Fill)
    .height(Fill)
    .style(theme::page)
    .into()
}

/// The visualizer's bars.
struct Bars {
    levels: Vec<f32>,
    dimmed: bool,
}

impl<Message> canvas::Program<Message> for Bars {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let accent = theme::accent(theme);
        for (rect, level) in bar_rects(&self.levels, bounds.size())
            .into_iter()
            .zip(&self.levels)
        {
            let alpha = if self.dimmed {
                0.35
            } else {
                0.45 + 0.55 * level.clamp(0.0, 1.0)
            };
            let radius = (rect.width / 2.0).min(4.0);
            frame.fill(
                &canvas::Path::rounded_rectangle(
                    Point::new(rect.x, rect.y),
                    Size::new(rect.width, rect.height),
                    radius.into(),
                ),
                accent.scale_alpha(alpha),
            );
        }
        vec![frame.into_geometry()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_fill_the_width_and_rise_by_level() {
        let levels = [0.0, 0.5, 1.0, 2.0];
        let rects = bar_rects(&levels, Size::new(130.0, 100.0));
        assert_eq!(rects.len(), 4);
        // 4 bars and 3 gaps of a third: the last bar ends at the right edge.
        let last = rects[3];
        assert!((last.x + last.width - 130.0).abs() < 1e-3);
        assert!((rects[1].x - (rects[0].width * 4.0 / 3.0)).abs() < 1e-3);
        // Silence keeps a sliver; a level over 1 is held to the top.
        assert_eq!(rects[0].height, 2.0);
        assert_eq!(rects[1].height, 50.0);
        assert_eq!(rects[2].height, 100.0);
        assert_eq!(rects[3].height, 100.0);
        for rect in &rects {
            assert!((rect.y + rect.height - 100.0).abs() < 1e-3);
        }
        assert!(bar_rects(&[], Size::new(100.0, 100.0)).is_empty());
        assert!(bar_rects(&levels, Size::new(0.0, 100.0)).is_empty());
    }

    #[test]
    fn the_visualizer_ticks_only_when_it_can_be_seen_moving() {
        let mut full = FullPlayer::new();
        assert!(!full.animates(true), "the artwork is showing");
        full.show = Show::Visualizer;
        assert!(full.animates(true));
        assert!(!full.animates(false), "paused: nothing moves");
        full.hidden = true;
        assert!(!full.animates(true), "minimised");
    }
}
