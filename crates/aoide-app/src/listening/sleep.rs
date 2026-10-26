//! The sleep timer: arming it, its clock, and pausing at its end or at the
//! boundary it waits for.

use std::time::Duration;

use aoide_audio::{EngineEvent, EntryId};
use iced::Task;

use super::Msg;
use crate::app::{App, Message};
use crate::playback::sleep_timer::{Choice, SleepTimer, Verdict};

impl App {
    /// An engine event, for the sleep timer: a boundary to pause on, or the
    /// end of the queue.
    pub(crate) fn sleep_on_event(&mut self, event: &EngineEvent) -> Task<Message> {
        let Some(timer) = &self.listening.sleep else {
            return Task::none();
        };
        let verdict = match event {
            EngineEvent::TrackStarted { id } => {
                let album = self
                    .playback
                    .queued(*id)
                    .and_then(|q| q.meta.album_id.clone());
                timer.track_started(*id, album.as_deref())
            }
            EngineEvent::Finished => timer.finished(),
            _ => Verdict::Keep,
        };
        match (verdict, event) {
            (Verdict::Pause, EngineEvent::TrackStarted { id }) => {
                let entry = *id;
                let paused = self.sleep_pause();
                // Once the pause's fade is over, back to the track's start, so
                // Play begins it rather than a fraction in.
                let rewind =
                    Task::perform(tokio::time::sleep(Duration::from_millis(400)), move |()| {
                        Message::Listening(Msg::SleepRewind(entry))
                    });
                Task::batch([paused, rewind])
            }
            (Verdict::Pause, _) => self.sleep_pause(),
            (Verdict::Drop, _) => self.sleep_set(None),
            (Verdict::Keep, _) => Task::none(),
        }
    }

    /// A sleep timer chosen from the menu, from what is playing now.
    pub(super) fn arm_sleep(&mut self, choice: Choice) -> Task<Message> {
        self.listening.menu = None;
        let current = self
            .playback
            .current()
            .map(|(entry, q)| (entry, q.meta.album_id.clone()));
        let timer = SleepTimer::arm(
            choice,
            current.as_ref().map(|(e, a)| (*e, a.as_deref())),
            self.now(),
        );
        if timer.is_none() {
            self.notice = Some("Nothing is playing, so there is no end to wait for.".into());
        }
        self.sleep_set(timer)
    }

    /// The minutes typed for a custom timer, if they read as minutes.
    pub(super) fn sleep_custom(&mut self) -> Task<Message> {
        match crate::playback::sleep_timer::parse_minutes(&self.listening.sleep_minutes) {
            Some(minutes) => {
                self.listening.sleep_minutes.clear();
                self.listening(Msg::Sleep(Choice::Minutes(minutes)))
            }
            None => Task::none(),
        }
    }

    pub(super) fn sleep_tick(&mut self) -> Task<Message> {
        match &self.listening.sleep {
            Some(timer) if timer.tick(self.now()) == Verdict::Pause => self.sleep_pause(),
            _ => Task::none(),
        }
    }

    /// Back to the start of the track paused at a boundary, if it is still
    /// the one held.
    pub(super) fn sleep_rewind(&mut self, entry: EntryId) -> Task<Message> {
        let held =
            self.playback.current().is_some_and(|(e, _)| e == entry) && !self.playback.is_playing();
        if held && let Err(error) = self.playback.seek(0) {
            self.notice = Some(error);
        }
        Task::none()
    }

    /// Pauses for the sleep timer and forgets it.
    fn sleep_pause(&mut self) -> Task<Message> {
        if let Err(error) = self.playback.pause() {
            self.notice = Some(error);
        }
        let report = self.report_pause_state();
        Task::batch([self.sleep_set(None), report])
    }

    /// Sets or clears the timer; the player's settings follow it.
    pub(super) fn sleep_set(&mut self, timer: Option<SleepTimer>) -> Task<Message> {
        self.listening.sleep = timer;
        let settings = self.player_settings();
        if let Err(error) = self.playback.set_settings(settings) {
            self.notice = Some(error);
        }
        Task::none()
    }
}
