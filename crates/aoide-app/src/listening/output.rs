//! The output device: listing the devices and choosing one.

use aoide_audio::DeviceChoice;
use iced::Task;

use super::Msg;
use crate::app::{App, Message};

/// cpal's listing blocks (ALSA can take a moment), so off the UI thread.
pub(super) fn list_devices() -> Task<Message> {
    Task::perform(
        async {
            tokio::task::spawn_blocking(aoide_audio::output_devices)
                .await
                .map_err(|e| e.to_string())
                .and_then(|listed| listed.map_err(|e| e.to_string()))
        },
        |result| Message::Listening(Msg::Devices(result)),
    )
}

impl App {
    pub(super) fn choose_device(&mut self, choice: DeviceChoice) -> Task<Message> {
        self.listening.menu = None;
        if choice == self.config.sound.device {
            return Task::none();
        }
        self.config.sound.device = choice.clone();
        self.save_config();
        if let Err(error) = self.playback.set_device(choice) {
            self.notice = Some(error);
        }
        Task::none()
    }
}
