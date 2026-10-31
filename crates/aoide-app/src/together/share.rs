//! Leading: sending the player's state to the session when it leaves step.

use iced::Task;

use super::Msg;
use super::plan;
use super::request::timed;
use crate::app::{App, Message};

impl App {
    /// Sends the player's state to the session, unless it's already where the
    /// session says. Called with every player change.
    pub(crate) fn share_change(&mut self) -> Task<Message> {
        let local = self.local();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let client = signed.connection.client.clone();
        let t = &mut signed.together;
        let Some(session) = t.session.as_ref() else {
            return Task::none();
        };
        if t.applying || plan::in_step(session, &local, t.server_now()) {
            return Task::none();
        }
        if t.pushing {
            t.dirty = true;
            return Task::none();
        }
        let Some(state) = local.state() else {
            return Task::none();
        };
        let (id, version) = (session.id, session.version);
        t.pushing = true;
        Task::perform(
            async move { timed(client.change_listening(id, version, &state)).await },
            |r| Message::Together(Msg::Pushed(r)),
        )
    }
}
