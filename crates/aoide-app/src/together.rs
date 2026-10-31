//! Listening together: playing in step with other devices in a session Melete
//! keeps (`aoide_melete::listen`). Each device plays its own stream.
//!
//! **Following.** While in a session the app asks for it every
//! [`POLL_EVERY`], as often as the server looks for changes itself, and moves
//! the player to where it says ([`plan`]): the same queue is jumped, sought,
//! played or paused within; another is loaded in its place.
//!
//! **Leading.** Every player change already passes through `save_queue`; from
//! there [`App::share_change`] sends the player's state, made against the
//! version last seen, unless the player is where the session says. That rule
//! is what keeps a change from echoing: one arrived from the session leaves
//! the player in step, so nothing goes back. A change refused for an old
//! version means someone else acted first: the session is fetched and
//! followed.
//!
//! Infinity is off while in a session: every device would add its own songs.

use std::time::Duration;

use aoide_melete::ListenSession;
use iced::{Subscription, Task};

use crate::app::{App, Message};
use crate::playback::tracks::TrackMeta;

mod ask;
mod follow;
pub mod plan;
mod request;
mod share;
mod view;

pub use plan::Local;
pub use request::{Failed, Timed};
pub use view::view;

/// How often a session is asked about while in one.
pub const POLL_EVERY: Duration = Duration::from_secs(2);

/// Listening together, while signed in.
#[derive(Debug, Default)]
pub struct Together {
    /// The session this device is in.
    pub session: Option<ListenSession>,
    /// The server's clock minus this one's, in milliseconds.
    offset_ms: i64,
    /// The code being typed, to join with.
    pub code: String,
    /// A start, join or leave is waiting on the server.
    pub busy: bool,
    /// What went wrong last, in a sentence.
    pub problem: Option<String>,
    /// A change is on its way to the server.
    pushing: bool,
    /// The player changed again meanwhile: send again once it's answered.
    dirty: bool,
    /// A poll is on its way.
    polling: bool,
    /// The session's queue is being loaded: what the player does meanwhile
    /// is the session's doing, not a change to send.
    applying: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Code(String),
    Start,
    Join,
    Leave,
    /// End it for everyone.
    End,
    Poll,
    /// A start or join answered.
    Joined(Result<Timed, Failed>),
    Polled(Result<Timed, Failed>),
    Pushed(Result<Timed, Failed>),
    Left(Result<(), Failed>),
    /// The session's queue, fetched to be loaded.
    Loaded(Result<(Vec<TrackMeta>, usize), Failed>),
}

impl Together {
    /// Takes the server's answer: the clock, and the session.
    fn adopt(&mut self, timed: Timed) {
        self.offset_ms = timed.listening.server_time - timed.local_mid;
        self.session = Some(timed.listening.session);
    }

    fn server_now(&self) -> i64 {
        aoide_store::now_millis() + self.offset_ms
    }

    /// The session's queue is loaded (or the load was overtaken): what the
    /// player does from here is ours.
    pub(crate) fn applying_ended(&mut self) {
        self.applying = false;
    }

    fn clear(&mut self) {
        *self = Self {
            code: std::mem::take(&mut self.code),
            ..Self::default()
        };
    }
}

impl App {
    /// Asks for the session while in one.
    pub(crate) fn together_subscription(&self) -> Subscription<Message> {
        let joined = self
            .session
            .as_ref()
            .is_some_and(|s| s.together.session.is_some());
        if joined {
            iced::time::every(POLL_EVERY).map(|_| Message::Together(Msg::Poll))
        } else {
            Subscription::none()
        }
    }

    /// Whether this device is in a session, which holds Infinity back.
    pub(crate) fn listening_together(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| s.together.session.is_some())
    }

    /// The player as it stands.
    pub(super) fn local(&self) -> Local {
        let (ids, index) = self.playback.saved_form();
        Local {
            ids,
            index: usize::try_from(index).unwrap_or(0),
            position_ms: self.playback.position_ms(),
            playing: self.playback.is_playing(),
        }
    }

    pub(crate) fn together(&mut self, message: Msg) -> Task<Message> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let t = &mut signed.together;
        match message {
            Msg::Code(code) => {
                t.code = code;
                Task::none()
            }
            Msg::Start | Msg::Join | Msg::Poll | Msg::Leave | Msg::End => self.ask(message),
            Msg::Joined(result) => {
                t.busy = false;
                match result {
                    Ok(timed) => {
                        t.code.clear();
                        t.adopt(timed);
                        self.follow()
                    }
                    Err(Failed::Ended) => {
                        t.problem = Some("No session has that code.".into());
                        Task::none()
                    }
                    Err(why) => self.together_failed(why),
                }
            }
            Msg::Polled(result) => {
                t.polling = false;
                match result {
                    Ok(timed) => {
                        t.adopt(timed);
                        self.follow()
                    }
                    Err(why) => self.together_failed(why),
                }
            }
            Msg::Pushed(result) => {
                t.pushing = false;
                match result {
                    Ok(timed) => {
                        t.adopt(timed);
                        if std::mem::take(&mut t.dirty) {
                            return self.share_change();
                        }
                        Task::none()
                    }
                    // Someone else acted first: theirs stands.
                    Err(Failed::Stale) => {
                        t.dirty = false;
                        Task::done(Message::Together(Msg::Poll))
                    }
                    Err(why) => self.together_failed(why),
                }
            }
            Msg::Left(result) => match result {
                // Gone already is as good as left.
                Ok(()) | Err(Failed::Ended) => {
                    t.clear();
                    Task::none()
                }
                Err(why) => {
                    t.busy = false;
                    self.together_failed(why)
                }
            },
            Msg::Loaded(result) => match result {
                Ok((tracks, position)) => self.load_session_queue(tracks, position),
                Err(why) => {
                    t.applying = false;
                    self.together_failed(why)
                }
            },
        }
    }

    fn together_failed(&mut self, why: Failed) -> Task<Message> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let t = &mut signed.together;
        t.busy = false;
        match why {
            Failed::SignedOut => Task::done(Message::TokenRejected),
            Failed::Ended => {
                let was_in = t.session.is_some();
                t.clear();
                if was_in {
                    t.problem = Some("Listening together has ended.".into());
                }
                Task::none()
            }
            Failed::Stale => Task::done(Message::Together(Msg::Poll)),
            Failed::Other(words) => {
                t.problem = Some(words);
                Task::none()
            }
        }
    }
}
