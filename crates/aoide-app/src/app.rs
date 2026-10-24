//! The application: one state, one message type, and the long-lived services
//! it owns — the audio output and player, the store thread, the artwork
//! cache and the sync loop.

use std::sync::Arc;
use std::time::Instant;

use aoide_audio::EngineEvent;
use aoide_jellyfin::Device;
use aoide_sidecar::{SidecarClient, SyncEngine};
use aoide_store::{AccountClaim, SyncReport};
use iced::task::Handle;
use iced::theme::Mode;
use iced::widget::{button, center, column, text};
use iced::{Element, Subscription, Task, Theme, window};

use crate::artwork::{self, ArtKey, Artwork};
use crate::config::{Config, Paths};
use crate::credentials::{CredentialStore, KeyringStore};
use crate::integration;
use crate::playback::Playback;
use crate::screens::login;
use crate::session::{self, Connection, Effect, Event, Phase};
use crate::store_worker::StoreHandle;
use crate::sync::{self, SyncThrottle};
use crate::theme;

pub struct App {
    paths: Option<Paths>,
    config: Config,
    keyring: Arc<dyn CredentialStore>,
    store: StoreHandle,
    /// Something the listener should know that is not about one screen.
    notice: Option<String>,
    artwork: Artwork,
    playback: Playback,
    mode: Mode,
    window: Option<window::Id>,
    /// The app's own monotonic clock, for the reporter's and the sync's timing.
    clock: Instant,
    phase: Phase,
    login: login::State,
    session: Option<Signed>,
}

/// Everything that exists only while signed in.
struct Signed {
    connection: Connection,
    sidecar: Option<SidecarClient>,
    sync: Option<sync::Engine>,
    throttle: SyncThrottle,
    /// The sync in flight, aborted on sign-out.
    syncing: Option<Handle>,
    sync_status: Option<String>,
    /// The store held data from before accounts were claimed: only the
    /// listener can say whose it is.
    unknown_owner: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    ThemeChanged(Mode),
    Window(window::Id, window::Event),
    Engine(EngineEvent),
    Artwork(ArtKey, Option<Vec<u8>>),
    Desktop(integration::Command),
    Login(login::Message),
    Restored(Result<Option<Connection>, String>),
    SignedIn(Result<Connection, String>),
    Claimed(Result<AccountClaim, String>),
    ResolveOwner { keep: bool },
    SignOut,
    TokenRejected,
    Synced(Result<SyncReport, String>),
    Ignore,
}

impl App {
    pub fn boot(paths: Option<Paths>, config: Config) -> (Self, Task<Message>) {
        let mut notice = None;
        let store = match paths
            .as_ref()
            .map(|p| StoreHandle::open(&p.store_file, &config.device_id))
        {
            Some(Ok(store)) => store,
            failed => {
                let reason = match failed {
                    Some(Err(error)) => error,
                    _ => "there is no home directory".to_owned(),
                };
                notice = Some(format!(
                    "Your listening history couldn't be opened ({reason}), so nothing is kept after you quit."
                ));
                StoreHandle::in_memory(&config.device_id).expect("an in-memory store always opens")
            }
        };
        let artwork = Artwork::new(paths.as_ref().map(|p| p.artwork_dir.clone()));
        let playback = Playback::start(config.volume);
        let keyring: Arc<dyn CredentialStore> = Arc::new(KeyringStore);

        let (phase, restore) = match config.account.clone() {
            Some(account) => (
                Phase::Restoring,
                Task::perform(
                    session::restore(account, device(&config), Arc::clone(&keyring)),
                    Message::Restored,
                ),
            ),
            None => (Phase::SignedOut, Task::none()),
        };
        let app = Self {
            login: login::State::new(config.last_server.clone()),
            paths,
            config,
            keyring,
            store,
            notice,
            artwork,
            playback,
            mode: Mode::None,
            window: None,
            clock: Instant::now(),
            phase,
            session: None,
        };
        let theme = iced::system::theme().map(Message::ThemeChanged);
        (app, Task::batch([theme, restore]))
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

    fn now(&self) -> u64 {
        u64::try_from(self.clock.elapsed().as_millis()).unwrap_or(u64::MAX)
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
            Message::Login(message) => match self.login.update(message) {
                login::Action::None => Task::none(),
                login::Action::Focus(id) => iced::widget::operation::focus(id),
                login::Action::SignIn(form) => {
                    self.phase = session::transition(self.phase, Event::Submitted).0;
                    Task::perform(
                        session::sign_in(form, device(&self.config), Arc::clone(&self.keyring)),
                        Message::SignedIn,
                    )
                }
            },
            Message::Restored(result) => match result {
                Ok(Some(connection)) => self.enter(Event::Restored, connection),
                Ok(None) => {
                    self.phase = session::transition(self.phase, Event::NothingToRestore).0;
                    Task::none()
                }
                Err(error) => {
                    self.phase = session::transition(self.phase, Event::NothingToRestore).0;
                    self.login.error = Some(format!("Your saved login couldn't be read: {error}."));
                    Task::none()
                }
            },
            Message::SignedIn(result) => match result {
                Ok(connection) => {
                    self.login = login::State::new(Some(connection.account.server.clone()));
                    self.enter(Event::SignInSucceeded, connection)
                }
                Err(error) => {
                    self.phase = session::transition(self.phase, Event::SignInFailed).0;
                    self.login.failed(error);
                    Task::none()
                }
            },
            Message::Claimed(result) => {
                match result {
                    Ok(AccountClaim::UnknownOwner) => {
                        if let Some(signed) = self.session.as_mut() {
                            signed.unknown_owner = true;
                        }
                    }
                    Ok(AccountClaim::Erased) => {
                        self.notice = Some(
                            "This computer's library belonged to another account and has been cleared."
                                .to_owned(),
                        );
                    }
                    Ok(AccountClaim::Unchanged) => {}
                    Err(error) => self.notice = Some(error),
                }
                Task::none()
            }
            Message::ResolveOwner { keep } => {
                let Some(signed) = self.session.as_mut() else {
                    return Task::none();
                };
                signed.unknown_owner = false;
                let user = signed.connection.account.user_id.clone();
                self.store.send(move |w| {
                    let _ = if keep {
                        w.store.keep_and_claim(&user)
                    } else {
                        w.store.erase_and_claim(&user)
                    };
                });
                self.request_sync()
            }
            Message::SignOut => self.leave(Event::SignOut),
            Message::TokenRejected => {
                let task = self.leave(Event::TokenRejected);
                self.login.error =
                    Some("The server no longer accepts this login. Sign in again.".to_owned());
                task
            }
            Message::Synced(result) => {
                if let Some(signed) = self.session.as_mut() {
                    signed.throttle.finished();
                    signed.syncing = None;
                    signed.sync_status = result.err();
                }
                Task::none()
            }
            Message::Ignore => Task::none(),
        }
    }

    /// Signs in (or restores): records the account, then runs the effects.
    fn enter(&mut self, event: Event, connection: Connection) -> Task<Message> {
        let (phase, effects) = session::transition(self.phase, event);
        self.phase = phase;
        if phase != Phase::SignedIn {
            return Task::none();
        }
        self.config.account = Some(connection.account.clone());
        self.config.last_server = Some(connection.account.server.clone());
        self.save_config();

        let sidecar = SidecarClient::new(&connection.account.server, connection.token()).ok();
        let sync = sidecar
            .clone()
            .map(|client| Arc::new(SyncEngine::new(self.store.shared(), client)));
        self.session = Some(Signed {
            connection,
            sidecar,
            sync,
            throttle: SyncThrottle::default(),
            syncing: None,
            sync_status: None,
            unknown_owner: false,
        });

        let mut tasks = Vec::new();
        for effect in effects {
            match effect {
                Effect::AdoptAccount => tasks.push(self.adopt_account()),
                Effect::Connect => tasks.push(self.request_sync()),
                _ => {}
            }
        }
        Task::batch(tasks)
    }

    /// Claims the store for the signed-in account before anything reads it.
    fn adopt_account(&self) -> Task<Message> {
        let Some(signed) = &self.session else {
            return Task::none();
        };
        let user = signed.connection.account.user_id.clone();
        let store = self.store.clone();
        Task::perform(
            async move {
                store
                    .call(move |w| w.store.adopt_account(&user).map_err(|e| e.to_string()))
                    .await
                    .unwrap_or_else(|| Err("the library is closing".to_owned()))
            },
            Message::Claimed,
        )
    }

    /// Signs out, or drops a rejected login. The screen changes now; nothing
    /// here waits on the network.
    fn leave(&mut self, event: Event) -> Task<Message> {
        let (phase, effects) = session::transition(self.phase, event);
        if phase == self.phase {
            return Task::none();
        }
        self.phase = phase;
        let Some(signed) = self.session.take() else {
            return Task::none();
        };
        if let Some(syncing) = &signed.syncing {
            syncing.abort();
        }
        let mut tasks = Vec::new();
        for effect in effects {
            match effect {
                Effect::StopPlayback => {
                    self.playback.stop_and_clear();
                }
                Effect::ClearSavedQueue => {
                    let now = aoide_store::now_millis();
                    self.store.send(move |w| {
                        let _ = w.recorder.stopped(&w.store, now);
                        let _ = w.store.clear_queue(now);
                    });
                }
                Effect::ForgetCredentials => {
                    let account = signed.connection.account.clone();
                    self.config.account = None;
                    self.save_config();
                    tasks.push(Task::perform(
                        session::forget(account, Arc::clone(&self.keyring)),
                        |_| Message::Ignore,
                    ));
                }
                Effect::NotifyServer => {
                    let client = signed.connection.client.clone();
                    tasks.push(Task::perform(session::notify_server(client), |()| {
                        Message::Ignore
                    }));
                }
                Effect::AdoptAccount | Effect::Connect => {}
            }
        }
        self.artwork.clear();
        Task::batch(tasks)
    }

    /// A sync, if one is due: at launch, on focus, at most once a minute.
    fn request_sync(&mut self) -> Task<Message> {
        let now = self.now();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let Some(engine) = signed.sync.clone() else {
            return Task::none();
        };
        // Not before the store is known to be this account's.
        if signed.unknown_owner || !signed.throttle.request(now) {
            return Task::none();
        }
        let (task, handle) = Task::perform(sync::run(engine), Message::Synced).abortable();
        signed.syncing = Some(handle);
        task
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
            window::Event::Focused => self.request_sync(),
            window::Event::CloseRequested => self.quit(),
            _ => Task::none(),
        }
    }

    /// Saves what should outlive the window, waits for the store thread to
    /// finish what it was given, then exits.
    fn quit(&mut self) -> Task<Message> {
        self.save_config();
        let store = self.store.clone();
        let now = aoide_store::now_millis();
        Task::future(async move {
            store
                .call(move |w| {
                    let _ = w.recorder.stopped(&w.store, now);
                })
                .await;
        })
        .then(|()| iced::exit())
    }

    fn save_config(&self) {
        if let Some(paths) = &self.paths {
            let _ = self.config.save(&paths.config_file);
        }
    }

    /// Starts downloads for covers asked for and not yet fetched.
    fn pump_artwork(&mut self) -> Task<Message> {
        let Some(signed) = &self.session else {
            return Task::none();
        };
        let client = signed.connection.client.clone();
        let dir = self.artwork.dir().map(ToOwned::to_owned);
        Task::batch(self.artwork.next_batch().into_iter().map(|key| {
            let url = client
                .artwork_url(&key.item, Some(&key.tag), artwork::WIDTH)
                .to_string();
            let dir = dir.clone();
            let done = key.clone();
            Task::perform(artwork::fetch(url, dir, key), move |bytes| {
                Message::Artwork(done.clone(), bytes)
            })
        }))
    }

    pub fn view(&self) -> Element<'_, Message> {
        match (self.phase, &self.session) {
            (Phase::Restoring, _) => center(text("Aoide").size(32)).into(),
            (Phase::SignedIn, Some(signed)) => {
                let mut lines = column![
                    text(format!(
                        "Signed in as {} on {}",
                        signed.connection.account.username, signed.connection.account.server
                    )),
                    button("Sign Out").on_press(Message::SignOut),
                ]
                .spacing(12);
                if signed.unknown_owner {
                    lines =
                        lines.push(button("Keep").on_press(Message::ResolveOwner { keep: true }));
                }
                if let Some(notice) = &self.notice {
                    lines = lines.push(text(notice.as_str()));
                }
                center(lines).into()
            }
            _ => self.login.view().map(Message::Login),
        }
    }
}

/// This install, as the server's devices dashboard shows it.
fn device(config: &Config) -> Device {
    Device::new(device_name(), config.device_id.clone())
}

/// The machine's name, where the system says it.
fn device_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|name| name.trim().to_owned())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Linux desktop".to_owned())
}
