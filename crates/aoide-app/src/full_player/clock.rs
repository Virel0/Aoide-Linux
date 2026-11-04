//! What the full player listens for: its keys, the pointer and the window's
//! focus, and a tick. The tick runs about 30 times a second while a moving
//! ground can be seen moving (the music plays and the window has focus),
//! twice a second while only the controls wait to hide, and not at all
//! otherwise.

use std::time::{Duration, Instant};

use iced::keyboard::{self, Key, key::Named};
use iced::{Event, Subscription, Task, mouse, window};

use super::FullPlayer;
use crate::app::{App, Message};
use crate::listening::Msg;

/// About 30 frames a second: the grounds move slowly.
const FRAME: Duration = Duration::from_millis(33);
/// How often to look whether the controls should hide, when nothing moves.
const IDLE_CHECK: Duration = Duration::from_millis(500);
/// How long the pointer and keys stay still before the controls hide.
pub const HIDE_AFTER: Duration = Duration::from_secs(3);
/// The longest step the ground takes in one tick, so a stall or a pause
/// doesn't jump it.
const LONGEST_STEP: f32 = 0.1;

impl App {
    pub(crate) fn full_player_subscription(&self) -> Subscription<Message> {
        let Some(full) = &self.listening.full else {
            return Subscription::none();
        };
        let mut listening = vec![
            keyboard::listen().filter_map(|event| match event {
                keyboard::Event::KeyPressed { key, modifiers, .. } => {
                    Some(key_message(&key, modifiers))
                }
                _ => None,
            }),
            iced::event::listen_with(stirred),
        ];
        let playing = self.playback.is_playing();
        if self.ground_moves(full) {
            listening.push(iced::time::every(FRAME).map(tick));
        } else if playing && !full.still {
            listening.push(iced::time::every(IDLE_CHECK).map(tick));
        }
        Subscription::batch(listening)
    }

    /// Whether the moving ground is on screen and should move now.
    fn ground_moves(&self, full: &FullPlayer) -> bool {
        self.config.interface.full_ground.moves()
            && full.backdrop.is_some()
            && full.focused
            && self.playback.is_playing()
    }

    pub(crate) fn full_tick(&mut self, now: Instant) -> Task<Message> {
        let playing = self.playback.is_playing();
        let Some(moves) = self.listening.full.as_ref().map(|f| self.ground_moves(f)) else {
            return Task::none();
        };
        if let Some(full) = self.listening.full.as_mut() {
            if moves {
                if let Some(last) = full.last_tick {
                    full.clock += now.duration_since(last).as_secs_f32().min(LONGEST_STEP);
                }
                full.last_tick = Some(now);
            } else {
                full.last_tick = None;
            }
            if hides(playing, now.duration_since(full.last_stirred)) {
                full.still = true;
            }
        }
        Task::none()
    }

    /// The pointer moved or a key was pressed: the controls come back.
    pub(crate) fn full_stirred(&mut self) -> Task<Message> {
        if let Some(full) = self.listening.full.as_mut() {
            full.last_stirred = Instant::now();
            full.still = false;
        }
        Task::none()
    }

    pub(crate) fn full_focus(&mut self, focused: bool) -> Task<Message> {
        if let Some(full) = self.listening.full.as_mut() {
            full.focused = focused;
        }
        Task::none()
    }
}

fn tick(now: Instant) -> Message {
    Message::Listening(Msg::FullTick(now))
}

/// Whether the controls hide: only while the music plays, after
/// `HIDE_AFTER` with nothing stirring.
pub fn hides(playing: bool, since_stirred: Duration) -> bool {
    playing && since_stirred >= HIDE_AFTER
}

/// The pointer moving and the window's focus.
fn stirred(event: Event, _status: iced::event::Status, _window: window::Id) -> Option<Message> {
    match event {
        Event::Mouse(mouse::Event::CursorMoved { .. }) => {
            Some(Message::Listening(Msg::FullStirred))
        }
        Event::Window(window::Event::Focused) => Some(Message::Listening(Msg::FullFocus(true))),
        Event::Window(window::Event::Unfocused) => Some(Message::Listening(Msg::FullFocus(false))),
        _ => None,
    }
}

/// What a key does in the full player: Esc leaves, V (alone) changes the
/// layout, B (alone) the ground; any other key just brings the controls
/// back.
fn key_message(key: &Key, modifiers: keyboard::Modifiers) -> Message {
    let alone = !modifiers.command() && !modifiers.control() && !modifiers.alt();
    match key.as_ref() {
        Key::Named(Named::Escape) => Message::Listening(Msg::CloseFull),
        Key::Character(c) if alone && c.eq_ignore_ascii_case("v") => {
            Message::Listening(Msg::NextLayout)
        }
        Key::Character(c) if alone && c.eq_ignore_ascii_case("b") => {
            Message::Listening(Msg::NextGround)
        }
        _ => Message::Listening(Msg::FullStirred),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is(message: Message, expected: fn(&Msg) -> bool) -> bool {
        matches!(message, Message::Listening(ref m) if expected(m))
    }

    #[test]
    fn the_controls_hide_after_three_still_seconds_while_playing() {
        assert!(!hides(true, Duration::from_millis(2_900)));
        assert!(hides(true, HIDE_AFTER));
        // Paused, they stay, however long.
        assert!(!hides(false, Duration::from_secs(600)));
    }

    #[test]
    fn v_and_b_alone_change_the_layout_and_ground_and_esc_leaves() {
        let none = keyboard::Modifiers::empty();
        let v = Key::Character("v".into());
        assert!(is(key_message(&v, none), |m| matches!(m, Msg::NextLayout)));
        assert!(is(
            key_message(&Key::Character("V".into()), keyboard::Modifiers::SHIFT),
            |m| matches!(m, Msg::NextLayout)
        ));
        assert!(is(
            key_message(&Key::Character("b".into()), none),
            |m| matches!(m, Msg::NextGround)
        ));
        assert!(is(
            key_message(&Key::Named(Named::Escape), none),
            |m| matches!(m, Msg::CloseFull)
        ));
        // Ctrl+V and other keys only bring the controls back.
        assert!(is(key_message(&v, keyboard::Modifiers::CTRL), |m| {
            matches!(m, Msg::FullStirred)
        }));
        assert!(is(
            key_message(&Key::Character("x".into()), none),
            |m| matches!(m, Msg::FullStirred)
        ));
    }
}
