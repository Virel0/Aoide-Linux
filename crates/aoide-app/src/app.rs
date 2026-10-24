//! The application: one state, one message type, and the long-lived services
//! it owns — the audio output and player, the store thread, the artwork
//! cache and the sync loop.

use std::time::Instant;

use aoide_audio::EngineEvent;
use iced::theme::Mode;
use iced::widget::{center, column, text};
use iced::{Element, Subscription, Task, Theme, window};

use crate::artwork::{self, ArtKey, Artwork};
use crate::config::{Config, Paths};
use crate::integration;
use crate::playback::Playback;
use crate::store_worker::StoreHandle;
use crate::theme;

pub struct App {
    paths: Option<Paths>,
    config: Config,
    store: Option<StoreHandle>,
    store_error: Option<String>,
    artwork: Artwork,
    playback: Playback,
    mode: Mode,
    window: Option<window::Id>,
    /// The app's own monotonic clock, for the reporter's and the sync's timing.
    clock: Instant,
}

#[derive(Debug, Clone)]
pub enum Message {
    ThemeChanged(Mode),
    Window(window::Id, window::Event),
    Engine(EngineEvent),
    Artwork(ArtKey, Option<Vec<u8>>),
    Desktop(integration::Command),
    Saved,
}

impl App {
    pub fn boot(paths: Option<Paths>, config: Config) -> (Self, Task<Message>) {
        let (store, store_error) = match &paths {
            Some(paths) => match StoreHandle::open(&paths.store_file, &config.device_id) {
                Ok(store) => (Some(store), None),
                Err(error) => (
                    None,
                    Some(format!("The local library couldn't be opened: {error}")),
                ),
            },
            None => (
                None,
                Some("There is no home directory to keep the library in.".into()),
            ),
        };
        let artwork = Artwork::new(paths.as_ref().map(|p| p.artwork_dir.clone()));
        let playback = Playback::start(config.volume);
        let app = Self {
            paths,
            config,
            store,
            store_error,
            artwork,
            playback,
            mode: Mode::None,
            window: None,
            clock: Instant::now(),
        };
        let theme = iced::system::theme().map(Message::ThemeChanged);
        (app, theme)
    }

    pub fn title(&self) -> String {
        "Aoide".to_owned()
    }

    pub fn theme(&self) -> Theme {
        theme::for_mode(self.mode)
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            self.playback.subscription().map(Message::Engine),
            iced::system::theme_changes().map(Message::ThemeChanged),
            window::events().map(|(id, event)| Message::Window(id, event)),
            integration::commands().map(Message::Desktop),
        ])
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ThemeChanged(mode) => {
                self.mode = mode;
                Task::none()
            }
            Message::Window(id, event) => self.window_event(id, event),
            Message::Engine(event) => {
                let _ = self.playback.handle(&event);
                Task::none()
            }
            Message::Artwork(key, bytes) => {
                self.artwork.arrived(key, bytes);
                Task::none()
            }
            Message::Desktop(_command) => Task::none(),
            Message::Saved => Task::none(),
        }
    }

    fn window_event(&mut self, id: window::Id, event: window::Event) -> Task<Message> {
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
            window::Event::CloseRequested => self.quit(),
            _ => Task::none(),
        }
    }

    /// Saves what should outlive the window, waits for the store thread to
    /// finish what it was given, then exits.
    fn quit(&mut self) -> Task<Message> {
        self.save_config();
        let store = self.store.clone();
        Task::future(async move {
            if let Some(store) = store {
                store.call(|_| ()).await;
            }
        })
        .then(|()| iced::exit())
    }

    fn save_config(&self) {
        if let Some(paths) = &self.paths {
            let _ = self.config.save(&paths.config_file);
        }
    }

    /// Starts downloads for covers asked for and not yet fetched.
    #[allow(dead_code)]
    fn pump_artwork(&mut self, url: impl Fn(&ArtKey) -> Option<String>) -> Task<Message> {
        let dir = self.artwork.dir().map(ToOwned::to_owned);
        Task::batch(self.artwork.next_batch().into_iter().map(|key| {
            let Some(address) = url(&key) else {
                return Task::done(Message::Artwork(key, None));
            };
            let dir = dir.clone();
            Task::perform(artwork::fetch(address, dir, key.clone()), move |bytes| {
                Message::Artwork(key.clone(), bytes)
            })
        }))
    }

    pub fn view(&self) -> Element<'_, Message> {
        let mut lines = column![text("Aoide").size(32)].spacing(8);
        if let Some(error) = &self.store_error {
            lines = lines.push(text(error.as_str()));
        }
        if let Some(reason) = self.playback.unavailable() {
            lines = lines.push(text(format!("No sound output: {reason}")));
        }
        let _ = self.clock;
        center(lines).into()
    }
}
