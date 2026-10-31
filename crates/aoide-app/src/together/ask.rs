//! Asking the server: starting, joining, polling and leaving.

use iced::Task;

use super::Msg;
use super::request::{failed, timed};
use crate::app::{App, Message};

impl App {
    /// Sends what `message` asks for; its answer comes back as another [`Msg`].
    pub(super) fn ask(&mut self, message: Msg) -> Task<Message> {
        let local = self.local();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let client = signed.connection.client.clone();
        let t = &mut signed.together;
        match message {
            Msg::Start => {
                let Some(state) = local.state() else {
                    t.problem = Some("This queue has songs that aren't on the server.".into());
                    return Task::none();
                };
                t.busy = true;
                t.problem = None;
                Task::perform(
                    async move { timed(client.start_listening(&state)).await },
                    |r| Message::Together(Msg::Joined(r)),
                )
            }
            Msg::Join => {
                let code = t.code.trim().to_owned();
                if code.is_empty() {
                    return Task::none();
                }
                t.busy = true;
                t.problem = None;
                Task::perform(
                    async move { timed(client.join_listening(&code)).await },
                    |r| Message::Together(Msg::Joined(r)),
                )
            }
            Msg::Poll => {
                let Some(id) = t.session.as_ref().map(|s| s.id) else {
                    return Task::none();
                };
                if t.polling || t.pushing || t.applying {
                    return Task::none();
                }
                t.polling = true;
                Task::perform(async move { timed(client.listening(id)).await }, |r| {
                    Message::Together(Msg::Polled(r))
                })
            }
            Msg::Leave | Msg::End => {
                let Some(id) = t.session.as_ref().map(|s| s.id) else {
                    return Task::none();
                };
                t.busy = true;
                let end = matches!(message, Msg::End);
                Task::perform(
                    async move {
                        if end {
                            client.end_listening(id).await
                        } else {
                            client.leave_listening(id).await
                        }
                        .map_err(failed)
                    },
                    |r| Message::Together(Msg::Left(r)),
                )
            }
            _ => Task::none(),
        }
    }
}
