//! The application: one state, one message type, and the long-lived services
//! it owns — the audio output and player, the store thread, the artwork
//! cache and the sync loop.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use aoide_audio::EngineEvent;
use aoide_jellyfin::{Device, Item};
use aoide_sidecar::{SidecarClient, SyncEngine};
use aoide_store::{AccountClaim, SyncReport};
use iced::task::Handle;
use iced::theme::Mode;
use iced::widget::{center, text};
use iced::{Element, Subscription, Task, Theme, window};

use crate::actions::{Flags, Menu, PlayRequest, RowAction};
use crate::artwork::{self, ArtKey, Artwork};
use crate::config::{Config, Paths};
use crate::credentials::{CredentialStore, KeyringStore};
use crate::integration;
use crate::nav::BackStack;
use crate::playback::tracks::{self, Measures};
use crate::playback::{Enqueue, Playback};
use crate::screens::{self, Ctx, Page, PageMsg, PlaylistSummary, Route, Section, login};
use crate::session::{self, Connection, Effect, Event, Phase};
use crate::store_worker::StoreHandle;
use crate::sync::{self, SyncThrottle};
use crate::theme;

pub struct App {
    pub(crate) paths: Option<Paths>,
    pub(crate) config: Config,
    keyring: Arc<dyn CredentialStore>,
    pub(crate) store: StoreHandle,
    /// Something the listener should know that is not about one screen.
    pub(crate) notice: Option<String>,
    pub(crate) artwork: Artwork,
    pub(crate) playback: Playback,
    mode: Mode,
    window: Option<window::Id>,
    /// The app's own monotonic clock, for the reporter's and the sync's timing.
    clock: Instant,
    phase: Phase,
    login: login::State,
    pub(crate) session: Option<Signed>,
}

/// An open page and its place in the stack.
#[derive(Debug)]
pub struct Entry {
    pub key: u64,
    pub page: Page,
}

/// The right-hand panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Queue,
    Lyrics,
}

/// Everything that exists only while signed in.
pub(crate) struct Signed {
    pub connection: Connection,
    pub sidecar: Option<SidecarClient>,
    pub sync: Option<sync::Engine>,
    pub throttle: SyncThrottle,
    /// The sync in flight, aborted on sign-out.
    pub syncing: Option<Handle>,
    pub sync_status: Option<String>,
    /// The store held data from before accounts were claimed: only the
    /// listener can say whose it is.
    pub unknown_owner: bool,
    /// The music library to stay inside; `None` until found, or if the
    /// server has none (everything is shown then).
    pub library: Option<String>,
    pub library_known: bool,
    pub section: Section,
    pub nav: Option<BackStack<Entry>>,
    pub next_key: u64,
    pub playlists: Vec<PlaylistSummary>,
    pub menu: Option<Menu>,
    /// Favourites changed this session, over what the items said.
    pub favourites: HashMap<String, bool>,
    pub panel: Option<Panel>,
    pub measures: Measures,
    /// While the seek bar is dragged: where it is.
    pub scrub: Option<u64>,
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
    Libraries(Result<Vec<Item>, String>),
    Section(Section),
    Open(Route),
    Back,
    Page(u64, PageMsg),
    PlaylistsChanged,
    Playlists(Vec<PlaylistSummary>),
    Row(RowAction),
    Favourited(String, Result<bool, String>),
    Play(PlayRequest),
    Player(Transport),
    Notice(String),
    Dismiss,
    Ignore,
}

/// The player bar's controls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Transport {
    Toggle,
    Next,
    Previous,
    Scrub(f32),
    ScrubEnd,
    Volume(f32),
    Shuffle,
    Repeat,
    Crossfade,
    Infinity,
    Panel(Panel),
    ClosePanel,
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
        let mut playback = Playback::start(config.volume);
        let _ = playback.set_settings(config.settings.player());
        let keyring: Arc<dyn CredentialStore> = Arc::new(KeyringStore);

        let (phase, restore) = match config.account.clone() {
            Some(account) => (
                Phase::Restoring,
                Task::perform(
                    session::restore(account, device(&config), Arc::clone(&keyring)),
                    Message::Restored,
                ),
            ),
            None => (
                Phase::SignedOut,
                iced::widget::operation::focus(login::SERVER),
            ),
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

    pub(crate) fn now(&self) -> u64 {
        u64::try_from(self.clock.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        // Position ticks arrive four times a second and change no artwork.
        let quiet = matches!(message, Message::Engine(EngineEvent::Position { .. }));
        let task = self.handle(message);
        if quiet {
            return task;
        }
        Task::batch([task, self.fetch_artwork()])
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ThemeChanged(mode) => {
                self.mode = mode;
                Task::none()
            }
            Message::Window(id, event) => self.window_event(id, event),
            Message::Engine(event) => self.engine_event(event),
            Message::Artwork(key, bytes) => {
                self.artwork.arrived(key, bytes);
                Task::none()
            }
            Message::Desktop(command) => self.desktop(command),
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
                    iced::widget::operation::focus(login::SERVER)
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
                        return Task::none();
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
                Task::batch([self.request_sync(), self.after_claim()])
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
                Task::batch([self.request_sync(), self.after_claim()])
            }
            Message::SignOut => self.leave(Event::SignOut),
            Message::TokenRejected => {
                let task = self.leave(Event::TokenRejected);
                self.login.error =
                    Some("The server no longer accepts this login. Sign in again.".to_owned());
                task
            }
            Message::Synced(result) => {
                let Some(signed) = self.session.as_mut() else {
                    return Task::none();
                };
                signed.throttle.finished();
                signed.syncing = None;
                let changed = result.as_ref().is_ok_and(|r| r.applied > 0);
                signed.sync_status = result.err();
                if changed {
                    self.load_playlists()
                } else {
                    Task::none()
                }
            }
            Message::Libraries(result) => {
                let Some(signed) = self.session.as_mut() else {
                    return Task::none();
                };
                signed.library = result
                    .ok()
                    .and_then(|libraries| libraries.into_iter().next())
                    .map(|library| library.id);
                signed.library_known = true;
                let section = signed.section.clone();
                self.show_section(section)
            }
            Message::Section(section) => self.show_section(section),
            Message::Open(route) => self.open(route),
            Message::Back => {
                if let Some(nav) = self.session.as_mut().and_then(|s| s.nav.as_mut()) {
                    nav.back();
                }
                Task::none()
            }
            Message::Page(key, message) => self.page_message(key, message),
            Message::PlaylistsChanged => self.load_playlists(),
            Message::Playlists(playlists) => {
                if let Some(signed) = self.session.as_mut() {
                    signed.playlists = playlists;
                }
                Task::none()
            }
            Message::Row(action) => self.row_action(action),
            Message::Favourited(id, result) => {
                match result {
                    Ok(favourite) => {
                        if let Some(signed) = self.session.as_mut() {
                            signed.favourites.insert(id, favourite);
                        }
                    }
                    Err(error) => self.notice = Some(error),
                }
                Task::none()
            }
            Message::Play(request) => self.play(request),
            Message::Player(control) => self.transport(control),
            Message::Notice(notice) => {
                self.notice = Some(notice);
                Task::none()
            }
            Message::Dismiss => {
                self.notice = None;
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
            library: None,
            library_known: false,
            section: Section::Home,
            nav: None,
            next_key: 1,
            playlists: Vec::new(),
            menu: None,
            favourites: HashMap::new(),
            panel: None,
            measures: Measures::default(),
            scrub: None,
        });

        let mut tasks = Vec::new();
        for effect in effects {
            match effect {
                // The sync and everything that reads the store wait for the
                // claim (see `Message::Claimed`).
                Effect::AdoptAccount => tasks.push(self.adopt_account()),
                Effect::Connect => tasks.push(self.find_library()),
                _ => {}
            }
        }
        Task::batch(tasks)
    }

    /// What runs once the store is known to be this account's.
    fn after_claim(&mut self) -> Task<Message> {
        self.load_playlists()
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

    fn find_library(&self) -> Task<Message> {
        let Some(signed) = &self.session else {
            return Task::none();
        };
        let client = signed.connection.client.clone();
        Task::perform(
            async move { client.music_libraries().await },
            |result| match result {
                Err(aoide_jellyfin::Error::Unauthorized) => Message::TokenRejected,
                result => Message::Libraries(result.map_err(|e| session::describe(&e))),
            },
        )
    }

    pub(crate) fn load_playlists(&self) -> Task<Message> {
        let store = self.store.clone();
        Task::perform(
            async move {
                store
                    .call(|w| {
                        let lists = w.store.playlists().unwrap_or_default();
                        lists
                            .into_iter()
                            .map(|p| PlaylistSummary {
                                count: w.store.item_count(&p.id).unwrap_or(0),
                                id: p.id,
                                name: p.name,
                            })
                            .collect::<Vec<_>>()
                    })
                    .await
                    .unwrap_or_default()
            },
            Message::Playlists,
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
                Effect::StopPlayback => self.stop_playback(),
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
        tasks.push(iced::widget::operation::focus(login::USERNAME));
        Task::batch(tasks)
    }

    fn stop_playback(&mut self) {
        self.playback.stop_and_clear();
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

    // MARK: - Navigation

    /// A sidebar click: the section's root page, starting the stack over.
    fn show_section(&mut self, section: Section) -> Task<Message> {
        let store = self.store.clone();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        signed.section = section.clone();
        signed.menu = None;
        if !signed.library_known {
            return Task::none();
        }
        let key = signed.next_key;
        signed.next_key += 1;
        let ctx = Ctx {
            client: &signed.connection.client,
            library: signed.library.as_deref(),
            store: &store,
            key,
        };
        let (page, task) = Page::section(&section, &ctx);
        let entry = Entry { key, page };
        match signed.nav.as_mut() {
            Some(nav) => nav.reset(entry),
            None => signed.nav = Some(BackStack::new(entry)),
        }
        task
    }

    /// Opens a page on top of the current one.
    fn open(&mut self, route: Route) -> Task<Message> {
        let store = self.store.clone();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        signed.menu = None;
        let key = signed.next_key;
        signed.next_key += 1;
        let ctx = Ctx {
            client: &signed.connection.client,
            library: signed.library.as_deref(),
            store: &store,
            key,
        };
        let (page, task) = Page::route(&route, &ctx);
        match signed.nav.as_mut() {
            Some(nav) => nav.push(Entry { key, page }),
            None => signed.nav = Some(BackStack::new(Entry { key, page })),
        }
        task
    }

    fn page_message(&mut self, key: u64, message: PageMsg) -> Task<Message> {
        if let PageMsg::Settings(message) = message {
            return self.settings_message(key, message);
        }
        let store = self.store.clone();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let Some(nav) = signed.nav.as_mut() else {
            return Task::none();
        };
        let Some(entry) = nav.iter_mut().find(|e| e.key == key) else {
            return Task::none();
        };
        let ctx = Ctx {
            client: &signed.connection.client,
            library: signed.library.as_deref(),
            store: &store,
            key,
        };
        entry.page.update(message, &ctx)
    }

    fn settings_message(&mut self, key: u64, message: screens::settings::Message) -> Task<Message> {
        use screens::settings::Message as S;
        let task = match &message {
            S::Toggle(which, on) => {
                which.apply(&mut self.config.settings, *on);
                self.settings_changed(*which)
            }
            S::LyricOffset(ms) => {
                self.config.settings.lyric_offset_ms =
                    crate::config::Settings::clamped_lyric_offset(*ms);
                self.save_config();
                Task::none()
            }
            S::Unhide(id) => {
                let id = id.clone();
                let store = self.store.clone();
                let now = aoide_store::now_millis();
                let reload = screens::settings::load_hidden(&store, key);
                return Task::perform(
                    async move {
                        store
                            .call(move |w| {
                                let content_key = w
                                    .store
                                    .flags(&id)
                                    .ok()
                                    .flatten()
                                    .map(|f| f.content_key)
                                    .unwrap_or_default();
                                let _ = w.store.set_not_interested(false, &id, &content_key, now);
                                let _ = w.store.set_dont_count(false, &id, &content_key, now);
                            })
                            .await;
                    },
                    |()| Message::Ignore,
                )
                .chain(reload);
            }
            _ => Task::none(),
        };
        if let Some(Page::Settings(state)) = self
            .session
            .as_mut()
            .and_then(|s| s.nav.as_mut())
            .and_then(|nav| nav.iter_mut().find(|e| e.key == key))
            .map(|e| &mut e.page)
        {
            let _ = state.update(message);
        }
        task
    }

    /// A setting changed: saved, and handed to whatever it governs.
    fn settings_changed(&mut self, _which: screens::settings::Toggle) -> Task<Message> {
        self.save_config();
        let _ = self.playback.set_settings(self.config.settings.player());
        Task::none()
    }

    // MARK: - Row actions

    fn row_action(&mut self, action: RowAction) -> Task<Message> {
        let store = self.store.clone();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        match action {
            RowAction::Open { anchor, tracks } => {
                if signed.menu.as_ref().is_some_and(|m| m.anchor == anchor) {
                    signed.menu = None;
                    return Task::none();
                }
                let single = (tracks.len() == 1).then(|| tracks[0].id.clone());
                signed.menu = Some(Menu::new(anchor, tracks));
                let Some(id) = single else {
                    return Task::none();
                };
                Task::perform(
                    async move {
                        let asked = id.clone();
                        let flags = store
                            .call(move |w| w.store.flags(&asked).ok().flatten())
                            .await
                            .flatten();
                        let flags = flags.map_or_else(Flags::default, |f| Flags {
                            not_interested: f.not_interested,
                            dont_count: f.dont_count,
                        });
                        (id, flags)
                    },
                    |(id, flags)| Message::Row(RowAction::FlagsLoaded(id, flags)),
                )
            }
            RowAction::FlagsLoaded(id, flags) => {
                if let Some(menu) = signed.menu.as_mut()
                    && menu.single().is_some_and(|t| t.id == id)
                {
                    menu.flags = Some(flags);
                }
                Task::none()
            }
            RowAction::Close => {
                signed.menu = None;
                Task::none()
            }
            RowAction::PlayNext | RowAction::PlayLast => {
                let Some(menu) = signed.menu.take() else {
                    return Task::none();
                };
                let how = if matches!(action, RowAction::PlayNext) {
                    Enqueue::Next
                } else {
                    Enqueue::Last
                };
                self.play(PlayRequest {
                    tracks: menu.tracks,
                    how,
                    source: aoide_store::PlaySource::Unknown,
                })
            }
            RowAction::ShowPlaylists => {
                if let Some(menu) = signed.menu.as_mut() {
                    menu.adding = !menu.adding;
                }
                Task::none()
            }
            RowAction::Filter(filter) => {
                if let Some(menu) = signed.menu.as_mut() {
                    menu.filter = filter;
                }
                Task::none()
            }
            RowAction::NewName(name) => {
                if let Some(menu) = signed.menu.as_mut() {
                    menu.new_name = name;
                }
                Task::none()
            }
            RowAction::AddTo(_) | RowAction::CreatePlaylist => {
                let Some(menu) = signed.menu.take() else {
                    return Task::none();
                };
                let target = match action {
                    RowAction::AddTo(id) => Ok(id),
                    _ => Err(menu.new_name.trim().to_owned()),
                };
                if target.as_ref().is_err_and(String::is_empty) {
                    signed.menu = Some(menu);
                    return Task::none();
                }
                let tracks = menu.tracks;
                let now = aoide_store::now_millis();
                Task::perform(
                    async move {
                        store
                            .call(move |w| {
                                let id = match target {
                                    Ok(id) => id,
                                    Err(name) => {
                                        w.store.create_playlist(&name, None, None, now)?.id
                                    }
                                };
                                let rows: Vec<_> =
                                    tracks.iter().map(|t| t.track_ref(now)).collect();
                                w.store.cache_tracks(&rows)?;
                                let links: Vec<_> = tracks.iter().map(|t| t.link()).collect();
                                w.store.add_tracks(&links, &id, now)?;
                                Ok::<_, aoide_store::StoreError>(())
                            })
                            .await
                            .map(|r| r.map_err(|e| e.to_string()))
                            .unwrap_or_else(|| Err("the library is closing".into()))
                    },
                    |result| match result {
                        Ok(()) => Message::PlaylistsChanged,
                        Err(error) => {
                            Message::Notice(format!("The songs couldn't be added: {error}"))
                        }
                    },
                )
            }
            RowAction::NotInterested | RowAction::DontCount => {
                let Some(menu) = signed.menu.as_mut() else {
                    return Task::none();
                };
                let Some(track) = menu.single().cloned() else {
                    return Task::none();
                };
                let mut flags = menu.flags.unwrap_or_default();
                let not_interested = matches!(action, RowAction::NotInterested);
                if not_interested {
                    flags.not_interested = !flags.not_interested;
                } else {
                    flags.dont_count = !flags.dont_count;
                }
                menu.flags = Some(flags);
                let now = aoide_store::now_millis();
                self.store.send(move |w| {
                    let _ = w.store.cache_tracks(&[track.track_ref(now)]);
                    let key = track.content_key();
                    let _ = if not_interested {
                        w.store
                            .set_not_interested(flags.not_interested, &track.id, &key, now)
                    } else {
                        w.store
                            .set_dont_count(flags.dont_count, &track.id, &key, now)
                    };
                });
                Task::none()
            }
            RowAction::Favourite => {
                let Some(track) = signed.menu.as_ref().and_then(Menu::single).cloned() else {
                    return Task::none();
                };
                let current = signed
                    .favourites
                    .get(&track.id)
                    .copied()
                    .unwrap_or(track.favourite);
                let client = signed.connection.client.clone();
                let id = track.id.clone();
                Task::perform(
                    async move { client.set_favorite(&id, !current).await },
                    move |result| match result {
                        Err(aoide_jellyfin::Error::Unauthorized) => Message::TokenRejected,
                        result => Message::Favourited(
                            track.id.clone(),
                            result
                                .map(|data| data.is_favorite)
                                .map_err(|e| session::describe(&e)),
                        ),
                    },
                )
            }
            RowAction::GoToAlbum | RowAction::GoToArtist => {
                let Some(track) = signed.menu.take().and_then(|m| m.single().cloned()) else {
                    return Task::none();
                };
                let route = match action {
                    RowAction::GoToAlbum => track.album_id.map(|id| Route::Album {
                        id,
                        title: track.album.clone(),
                    }),
                    _ => track.artist_id.map(|id| Route::Artist {
                        id,
                        name: track
                            .album_artist
                            .clone()
                            .unwrap_or_else(|| track.artist.clone()),
                    }),
                };
                route.map_or_else(Task::none, |route| self.open(route))
            }
        }
    }

    // MARK: - Playing

    pub(crate) fn play(&mut self, request: PlayRequest) -> Task<Message> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        signed.menu = None;
        let built: Vec<_> = request
            .tracks
            .iter()
            .filter_map(|meta| {
                tracks::build(
                    &signed.connection.client,
                    meta,
                    &signed.measures,
                    &self.config.settings,
                )
                .ok()
            })
            .collect();
        if let Err(error) =
            self.playback
                .enqueue(built, request.how, request.source, &HashMap::new())
        {
            self.notice = Some(error);
        }
        Task::none()
    }

    fn transport(&mut self, control: Transport) -> Task<Message> {
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
            Transport::Panel(panel) => {
                if let Some(signed) = self.session.as_mut() {
                    signed.panel = if signed.panel == Some(panel) {
                        None
                    } else {
                        Some(panel)
                    };
                }
                Ok(())
            }
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
        Task::none()
    }

    fn engine_event(&mut self, event: EngineEvent) -> Task<Message> {
        if let Err(error) = self.playback.handle(&event) {
            self.notice = Some(error);
        }
        Task::none()
    }

    fn desktop(&mut self, command: integration::Command) -> Task<Message> {
        use integration::Command as C;
        let result = match command {
            C::Play => self.playback.play(),
            C::Pause | C::Stop => self.playback.pause(),
            C::PlayPause => return self.transport(Transport::Toggle),
            C::Next => return self.transport(Transport::Next),
            C::Previous => return self.transport(Transport::Previous),
            C::SeekBy(ms) => {
                let at = (self.playback.position_ms() as i64 + ms).max(0) as u64;
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
            C::Raise => {
                return self.window.map_or_else(Task::none, window::gain_focus);
            }
        };
        self.result(result)
    }

    fn result(&mut self, result: Result<(), String>) -> Task<Message> {
        if let Err(error) = result {
            self.notice = Some(error);
        }
        Task::none()
    }

    // MARK: - Window

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

    pub(crate) fn save_config(&self) {
        if let Some(paths) = &self.paths {
            let _ = self.config.save(&paths.config_file);
        }
    }

    // MARK: - Artwork

    /// Asks for the covers on screen and starts the downloads that are due.
    fn fetch_artwork(&mut self) -> Task<Message> {
        let Some(signed) = &self.session else {
            return Task::none();
        };
        let mut wanted = Vec::new();
        if let Some(nav) = &signed.nav {
            wanted.extend(nav.current().page.artwork());
        }
        if let Some(queue) = self.playback.queue() {
            let position = queue.position().unwrap_or(0);
            for entry in queue
                .entries()
                .iter()
                .skip(position.saturating_sub(5))
                .take(60)
            {
                if let Some((id, tag)) = self
                    .playback
                    .queued(entry.id)
                    .and_then(|q| q.meta.artwork.as_ref())
                {
                    wanted.push(ArtKey::new(id, tag));
                }
            }
        }
        self.artwork.want(wanted);
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

    // MARK: - View

    pub fn view(&self) -> Element<'_, Message> {
        match (self.phase, &self.session) {
            (Phase::Restoring, _) => center(text("Aoide").size(32).font(crate::widgets::BOLD))
                .style(theme::page)
                .into(),
            (Phase::SignedIn, Some(signed)) => crate::shell::view(self, signed),
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
