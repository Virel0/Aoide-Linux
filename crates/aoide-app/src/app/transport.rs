//! The player's controls, from the player bar, the queue panel and the
//! desktop (MPRIS and the tray).

use iced::Task;

use super::{App, Message, Panel, Transport};
use crate::integration;
use crate::queue_panel::QueueAction;
use crate::screens;

impl App {
    pub(crate) fn transport(&mut self, control: Transport) -> Task<Message> {
        let result = match control {
            Transport::Toggle => self.playback.toggle(),
            Transport::Next => self.playback.next(),
            Transport::Previous => self.playback.previous(),
            Transport::Scrub(at) => {
                if let Some(signed) = self.session.as_mut() {
                    signed.scrub = Some(at.max(0.0) as u64);
                }
                Ok(())
            }
            Transport::Seek(ms) => self.playback.seek(ms),
            Transport::ScrubEnd => match self.session.as_mut().and_then(|s| s.scrub.take()) {
                Some(ms) => self.playback.seek(ms),
                None => Ok(()),
            },
            Transport::Volume(volume) => {
                self.config.volume = volume.clamp(0.0, 1.0);
                self.playback.set_volume(volume)
            }
            Transport::Shuffle => {
                let on = !self.playback.is_shuffled();
                self.playback.set_shuffled(on)
            }
            Transport::Repeat => self.playback.cycle_repeat(),
            Transport::Crossfade => {
                self.config.settings.crossfade = !self.config.settings.crossfade;
                return self.settings_changed(screens::settings::Toggle::Crossfade);
            }
            Transport::Infinity => {
                self.config.settings.infinity = !self.config.settings.infinity;
                return self.settings_changed(screens::settings::Toggle::Infinity);
            }
            Transport::Panel(panel) => return self.toggle_panel(panel),
            Transport::ClosePanel => {
                if let Some(signed) = self.session.as_mut() {
                    signed.panel = None;
                }
                Ok(())
            }
        };
        if let Err(error) = result {
            self.notice = Some(error);
        }
        match control {
            Transport::Toggle => self.save_queue(),
            Transport::Next | Transport::Previous | Transport::Shuffle | Transport::Repeat => {
                Task::batch([self.check_infinity(), self.save_queue()])
            }
            Transport::Seek(_) | Transport::ScrubEnd => self.save_queue(),
            _ => Task::none(),
        }
    }

    /// Opens the right-hand panel, or closes it if it is the one open.
    fn toggle_panel(&mut self, panel: Panel) -> Task<Message> {
        if let Some(signed) = self.session.as_mut() {
            signed.panel = if signed.panel == Some(panel) {
                None
            } else {
                Some(panel)
            };
            signed.lyric_line = None;
        }
        self.track_changed()
    }

    pub(super) fn queue_action(&mut self, action: QueueAction) -> Task<Message> {
        let result = match action {
            QueueAction::Jump(entry) => self.playback.jump(entry),
            QueueAction::Remove(entry) => self.playback.remove(entry),
            QueueAction::Move { index, destination } => {
                self.playback.move_entry(index, destination)
            }
            QueueAction::Clear => self.playback.clear_up_next(),
        };
        if let Err(error) = result {
            self.notice = Some(error);
        }
        Task::batch([self.save_queue(), self.check_infinity()])
    }

    pub(crate) fn desktop(&mut self, command: integration::Command) -> Task<Message> {
        use integration::Command as C;
        let result = match command {
            C::Play => self.playback.play(),
            C::Pause | C::Stop => self.playback.pause(),
            C::PlayPause => return self.transport(Transport::Toggle),
            C::Next => return self.transport(Transport::Next),
            C::Previous => return self.transport(Transport::Previous),
            C::SeekBy(ms) => {
                let at = (self.playback.position_ms() as i64 + ms).max(0) as u64;
                let length = self
                    .playback
                    .current()
                    .and_then(|(_, q)| q.meta.duration_ms)
                    .map(|ms| ms.max(0) as u64);
                // MPRIS: a seek past the end acts as Next.
                if length.is_some_and(|length| at >= length) {
                    return self.transport(Transport::Next);
                }
                self.playback.seek(at)
            }
            C::SetPosition(ms) => self.playback.seek(ms),
            C::Volume(volume) => return self.transport(Transport::Volume(volume)),
            C::Shuffle(on) => self.playback.set_shuffled(on),
            C::Loop(mode) => {
                // Off, all, one: at most two steps to any of them.
                let mut result = Ok(());
                for _ in 0..2 {
                    if self.playback.repeat() == mode {
                        break;
                    }
                    result = self.playback.cycle_repeat();
                }
                result
            }
            // Also out of the tray, if the window is hidden there.
            C::Raise => return self.show_window(),
            C::Quit => return self.quit(),
        };
        Task::batch([self.result(result), self.save_queue()])
    }
}
