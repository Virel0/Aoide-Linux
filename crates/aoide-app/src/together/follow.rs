//! Following the session: moving the player to where it says.

use iced::Task;

use super::Msg;
use super::plan::{self, Plan};
use super::request::failed;
use crate::app::{App, Message};
use crate::playback::tracks::TrackMeta;
use crate::playing::{Destination, Pending};

impl App {
    /// Moves the player to where the session is now.
    pub(super) fn follow(&mut self) -> Task<Message> {
        let local = self.local();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let t = &mut signed.together;
        let Some(session) = t.session.clone() else {
            return Task::none();
        };
        let server_now = t.server_now();
        match plan::plan(&session, &local, server_now) {
            Plan::Nothing => Task::none(),
            Plan::Rebuild => {
                t.applying = true;
                let client = signed.connection.client.clone();
                let ids: Vec<String> = session.queue.iter().map(ToString::to_string).collect();
                let current = ids.get(session.index).cloned();
                Task::perform(
                    async move {
                        let items = client.items_by_ids(&ids).await.map_err(failed)?;
                        let position = items
                            .iter()
                            .position(|item| Some(&item.id) == current.as_ref())
                            .unwrap_or(0);
                        Ok((items.iter().map(TrackMeta::from_item).collect(), position))
                    },
                    |r| Message::Together(Msg::Loaded(r)),
                )
            }
            Plan::Adjust { jump, seek, play } => {
                let entry = jump.and_then(|index| {
                    self.playback
                        .queue()
                        .and_then(|q| q.entries().get(index))
                        .map(|e| e.id)
                });
                let mut result = entry.map_or(Ok(()), |entry| self.playback.jump(entry));
                if let (Ok(()), Some(ms)) = (&result, seek) {
                    result = self.playback.seek(ms);
                }
                if let (Ok(()), Some(play)) = (&result, play) {
                    result = if play {
                        self.playback.play()
                    } else {
                        self.playback.pause()
                    };
                }
                if let Err(error) = result {
                    self.notice = Some(error);
                }
                Task::none()
            }
        }
    }

    /// Loads the session's queue in place of this one, at its track.
    pub(super) fn load_session_queue(
        &mut self,
        tracks: Vec<TrackMeta>,
        position: usize,
    ) -> Task<Message> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let Some(session) = signed.together.session.clone() else {
            signed.together.applying = false;
            return Task::none();
        };
        signed.play_generation += 1;
        let elapsed_ms = session.position_at(signed.together.server_now());
        Task::done(Message::Restore(Box::new(Pending {
            tracks,
            how: Destination::Together {
                position,
                elapsed_ms,
                playing: session.playing,
            },
            generation: signed.play_generation,
        })))
    }
}
