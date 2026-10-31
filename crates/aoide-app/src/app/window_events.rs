//! The window: where it is and how big, focus, closing, and quitting.

use iced::{Task, window};

use super::{App, Message};

impl App {
    pub(super) fn window_event(&mut self, id: window::Id, event: window::Event) -> Task<Message> {
        self.window = Some(id);
        match event {
            window::Event::Opened { position, size } => {
                if let Some(position) = position {
                    self.config.window.x = Some(position.x);
                    self.config.window.y = Some(position.y);
                }
                self.config.window.width = size.width;
                self.config.window.height = size.height;
                Task::none()
            }
            window::Event::Moved(position) => {
                self.config.window.x = Some(position.x);
                self.config.window.y = Some(position.y);
                Task::none()
            }
            window::Event::Resized(size) => {
                self.config.window.width = size.width;
                self.config.window.height = size.height;
                Task::none()
            }
            window::Event::Focused => self.request_sync(),
            window::Event::CloseRequested => self.close_requested(),
            window::Event::Unfocused => self.window_unfocused(),
            _ => Task::none(),
        }
    }

    /// Saves what should outlive the window, waits for the store thread to
    /// finish what it was given, then exits.
    pub(crate) fn quit(&mut self) -> Task<Message> {
        self.save_config();
        let _ = self.broadcast_queue();
        let store = self.store.clone();
        let now = aoide_store::now_millis();
        Task::future(async move {
            store
                .call(move |w| {
                    let _ = w.recorder.stopped(&w.store, now);
                    // The queue as it was left, for the other devices.
                    let _ = w.broadcast.flush_now(&w.store, now);
                })
                .await;
        })
        .then(|()| iced::exit())
    }
}
