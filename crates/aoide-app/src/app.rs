//! The application: one state, one message type, and the long-lived services
//! it owns — the audio output and player, the store thread, the artwork
//! cache and the sync loop.

use std::sync::Arc;
use std::time::Instant;

use aoide_audio::EngineEvent;
use aoide_melete::{Device, Lyrics};
use aoide_store::{AccountClaim, SyncReport};
use iced::theme::Mode;
use iced::widget::{center, text};
use iced::{Element, Subscription, Task, Theme, window};

use crate::actions::{PlayRequest, RowAction};
use crate::artwork::{ArtKey, Artwork};
use crate::config::{Config, Paths};
use crate::credentials::{CredentialStore, KeyringStore};
use crate::integration;
use crate::playback::Playback;
use crate::playback::infinity::TopUp;
use crate::playing::{Measured, Pending};
use crate::queue_panel::QueueAction;
use crate::screens::{self, Page, PageMsg, PlaylistSummary, Route, Section, login};
use crate::session::{self, Connection, Event, Phase};
use crate::store_worker::StoreHandle;
use crate::theme;

mod account;
mod art;
mod lyrics;
mod navigation;
mod playlists;
mod row_menu;
mod settings;
mod syncing;
mod transport;
mod window_events;

pub(crate) use account::Signed;

pub struct App {
    pub(crate) paths: Option<Paths>,
    pub(crate) config: Config,
    pub(crate) keyring: Arc<dyn CredentialStore>,
    pub(crate) store: StoreHandle,
    /// Something the listener should know that is not about one screen.
    pub(crate) notice: Option<String>,
    pub(crate) artwork: Artwork,
    pub(crate) playback: Playback,
    mode: Mode,
    window: Option<window::Id>,
    /// MPRIS, single instance and sleep inhibit.
    pub(crate) desktop: integration::Link,
    /// The app's own monotonic clock, for the reporter's and the sync's timing.
    clock: Instant,
    phase: Phase,
    login: login::State,
    /// Quick Connect while its code waits: which attempt, the server, the
    /// secret to ask with.
    quick: Option<(u64, aoide_melete::MeleteClient, String)>,
    quick_attempts: u64,
    /// A password sign-in waiting for its two-factor code.
    code_step: Option<session::CodeStep>,
    pub(crate) session: Option<Signed>,
    /// The tray and the other services of Settings → System.
    pub(crate) system: crate::system::State,
    /// Smart search's key and model picker.
    pub(crate) smart: crate::smart::State,
    /// A failure's details are shown (until another page opens).
    pub(crate) failure_details: bool,
    /// The player bar's menus, the sleep timer, private listening, the
    /// full-screen player.
    pub(crate) listening: crate::listening::State,
}

/// An open page and its place in the stack.
#[derive(Debug)]
pub struct Entry {
    pub key: u64,
    pub page: Page,
    /// Where it was opened from, to open it again for Retry.
    pub origin: Origin,
}

/// What opened a page: a sidebar section, or a route from another page.
#[derive(Debug, Clone)]
pub enum Origin {
    Section(Section),
    Route(Route),
}

/// The right-hand panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Queue,
    Lyrics,
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
    /// The password was right; a two-factor code finishes the sign-in.
    CodeNeeded(Box<session::CodeStep>),
    /// What the server said to the two-factor code.
    CodeChecked(Result<Connection, session::CodeRefused>),
    /// Quick Connect attempt `.0` has a code, or failed to get one.
    QuickStarted(
        u64,
        Result<(aoide_melete::MeleteClient, aoide_melete::QuickConnectCode), String>,
    ),
    /// What asking about attempt `.0`'s code said.
    QuickPolled(u64, Result<Option<Connection>, String>),
    Claimed(Result<AccountClaim, String>),
    ResolveOwner {
        keep: bool,
    },
    /// Erase Them pressed (true), or the erase called off (false).
    ConfirmErase(bool),
    SignOut,
    TokenRejected,
    /// Sync with Melete now, not at the next launch or focus.
    SyncNow,
    Synced(Result<SyncReport, String>),
    Section(Section),
    /// Settings, at one category: the command palette's way in.
    OpenSettings(screens::settings::Category),
    Open(Route),
    Back,
    Page(u64, PageMsg),
    /// What the page with this key failed to load: open it again.
    Retry(u64),
    /// Show (true) or hide a failure's details.
    FailureDetails(bool),
    /// The lyrics failed to load: ask again.
    RetryLyrics,
    PlaylistsChanged,
    Playlists(Vec<PlaylistSummary>),
    Row(RowAction),
    Favourited(String, Result<bool, String>),
    Play(PlayRequest),
    Measured(Box<(Pending, Measured)>),
    /// A batch of the queue, measured after it was queued.
    QueueMeasured(Box<Measured>),
    Restore(Box<Pending>),
    ToppedUp(Box<TopUp>),
    Player(Transport),
    Queue(QueueAction),
    Lyrics(String, Result<Option<Lyrics>, crate::failure::Failure>),
    Notice(String),
    Dismiss,
    Ignore,
    Tray(crate::tray::Message),
    Discord(crate::discord::Message),
    Smart(crate::smart::Message),
    Mixes(crate::mixes::Request),
    Update(crate::updates::Message),
    Palette(crate::palette::Message),
    /// Settings → System's file, log and cache rows.
    System(crate::maintenance::Message),
    Listening(crate::listening::Msg),
    Handoff(crate::handoff::Msg),
    Discover(crate::screens::discover::Msg),
    SyncPanel(crate::screens::sync_status::Msg),
    Radio(crate::radio::Msg),
    Covers(crate::covers::Msg),
    Sharing(crate::screens::share::Msg),
    /// Match an imported playlist for the import page with this key.
    ImportMatch(u64, Box<aoide_sidecar::ImportedPlaylist>),
    Collections(crate::screens::collections::Msg),
    Info(crate::info::Msg),
    Rescan(crate::rescan::Msg),
    Together(crate::together::Msg),
}

/// The player bar's controls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Transport {
    Toggle,
    Next,
    Previous,
    Scrub(f32),
    ScrubEnd,
    Seek(u64),
    Volume(f32),
    Shuffle,
    Repeat,
    Crossfade,
    Infinity,
    Panel(Panel),
    ClosePanel,
}

impl App {
    pub fn boot(
        paths: Option<Paths>,
        config: Config,
        desktop: integration::Link,
    ) -> (Self, Task<Message>) {
        let (store, notice) = open_store(paths.as_ref(), &config);
        let artwork = Artwork::new(paths.as_ref().map(|p| p.artwork_dir.clone()));
        let (playback, equaliser) = start_playback(&config);
        let notice = equaliser.or(notice);
        let keyring = keyring();
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
                iced::widget::operation::focus(
                    login::State::new(config.last_server.clone()).first_field(),
                ),
            ),
        };
        let system = crate::system::State::new(&desktop, &config);
        let key = crate::smart::load_key(Arc::clone(&keyring));
        let app = Self {
            login: login::State::new(config.last_server.clone()),
            quick: None,
            quick_attempts: 0,
            code_step: None,
            paths,
            config,
            keyring,
            store,
            notice,
            artwork,
            playback,
            mode: Mode::None,
            window: None,
            desktop,
            clock: Instant::now(),
            phase,
            session: None,
            system,
            smart: crate::smart::State::default(),
            listening: crate::listening::State::default(),
            failure_details: false,
        };
        let theme = iced::system::theme().map(Message::ThemeChanged);
        let update = Task::done(Message::Update(crate::updates::Message::Tick));
        let listening = Task::batch([
            crate::listening::boot(),
            crate::lyrics_extras::key_state(app.keyring.clone()),
        ]);
        (app, Task::batch([theme, restore, key, update, listening]))
    }

    pub fn title(&self) -> String {
        self.window_title()
    }

    pub fn theme(&self) -> Theme {
        crate::appearance::theme(self.mode, &self.config.interface)
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            self.playback.subscription().map(Message::Engine),
            iced::system::theme_changes().map(Message::ThemeChanged),
            window::events().map(|(id, event)| Message::Window(id, event)),
            integration::commands(&self.desktop).map(Message::Desktop),
            crate::tray::events(&self.system.tray).map(Message::Tray),
            crate::discord::status(self.system.discord.as_ref()).map(Message::Discord),
            crate::updates::ticks(&self.config.system.updates).map(Message::Update),
            crate::palette::keys().map(Message::Palette),
            self.listening_subscription(),
            self.together_subscription(),
        ])
    }

    /// The open page's back-stack entry, if signed in.
    fn page_key(&self) -> Option<u64> {
        let nav = self.session.as_ref()?.nav.as_ref()?;
        Some(nav.current().key)
    }

    pub(crate) fn now(&self) -> u64 {
        u64::try_from(self.clock.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        // Position ticks arrive four times a second and change no artwork.
        let quiet = matches!(message, Message::Engine(EngineEvent::Position { .. }))
            || crate::listening::is_quiet(&message);
        let page = self.page_key();
        let mut task = self.handle(message);
        // Another page opened: it starts at its top, not at the offset the
        // last one was scrolled to (iced keeps one scroll state for both).
        if self.page_key() != page {
            self.failure_details = false;
            task = Task::batch([
                task,
                iced::widget::operation::snap_to(
                    crate::widgets::PAGE,
                    iced::widget::scrollable::RelativeOffset::START,
                ),
            ]);
        }
        // The desktop hears only what changed.
        self.publish();
        self.publish_system();
        if quiet {
            return task;
        }
        Task::batch([task, self.fetch_artwork()])
    }

    /// Routes each message to the feature that handles it.
    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ThemeChanged(mode) => {
                self.mode = mode;
                Task::none()
            }
            Message::Window(id, event) => self.window_event(id, event),
            Message::Engine(event) => {
                let task = self.engine_event(event.clone());
                Task::batch([task, self.handoff_engine_event(&event)])
            }
            Message::Artwork(key, bytes) => self.artwork_arrived(key, bytes),
            Message::Desktop(command) => self.desktop(command),
            Message::Login(message) => self.login_message(message),
            Message::Restored(result) => self.restored(result),
            Message::SignedIn(result) => self.signed_in(result),
            Message::CodeNeeded(step) => self.code_needed(*step),
            Message::CodeChecked(result) => self.code_checked(result),
            Message::QuickStarted(attempt, result) => self.quick_started(attempt, result),
            Message::QuickPolled(attempt, result) => self.quick_polled(attempt, result),
            Message::Claimed(result) => self.claimed(result),
            Message::ConfirmErase(ask) => self.confirm_erase(ask),
            Message::ResolveOwner { keep } => self.resolve_owner(keep),
            Message::SignOut => self.leave(Event::SignOut),
            Message::SyncNow => self.sync_now(),
            Message::TokenRejected => self.token_rejected(),
            Message::Synced(result) => self.synced(result),
            Message::Section(section) => self.show_section(section),
            Message::Retry(key) => self.retry(key),
            Message::FailureDetails(open) => {
                self.failure_details = open;
                Task::none()
            }
            Message::RetryLyrics => self.retry_lyrics(),
            Message::OpenSettings(category) => self.open_settings(category),
            Message::Open(route) => self.open(route),
            Message::Back => self.back(),
            Message::Page(key, message) => self.page_message(key, message),
            Message::PlaylistsChanged => self.load_playlists(),
            Message::Playlists(playlists) => self.playlists_loaded(playlists),
            Message::Row(action) => self.row_action(action),
            Message::Favourited(id, result) => self.favourited(id, result),
            Message::Play(request) => self.play(request),
            Message::Measured(measured) => {
                let (pending, measured) = *measured;
                self.measured(pending, measured)
            }
            Message::QueueMeasured(measured) => self.queue_measured(*measured),
            Message::Restore(pending) => self.measure_then_queue(*pending),
            Message::ToppedUp(found) => self.topped_up(*found),
            Message::Player(control) => self.transport(control),
            Message::Queue(action) => self.queue_action(action),
            Message::Lyrics(item, result) => self.lyrics_arrived(item, result),
            Message::Notice(notice) => {
                self.notice = Some(notice);
                Task::none()
            }
            Message::Dismiss => {
                self.notice = None;
                Task::none()
            }
            Message::Ignore => Task::none(),
            Message::Tray(message) => self.tray_message(message),
            Message::Discord(message) => self.discord_message(message),
            Message::Smart(message) => self.smart_message(message),
            Message::Mixes(request) => self.mix_request(request),
            Message::Update(message) => self.update_message(message),
            Message::Palette(message) => self.palette_message(message),
            Message::System(message) => self.maintenance_message(message),
            Message::Listening(msg) => self.listening(msg),
            Message::Handoff(message) => self.handoff(message),
            Message::Discover(message) => self.discover(message),
            Message::SyncPanel(message) => self.sync_panel(message),
            Message::Radio(message) => self.radio(message),
            Message::Covers(message) => self.covers(message),
            Message::Sharing(message) => self.sharing(message),
            Message::ImportMatch(key, playlist) => self.import_match(key, *playlist),
            Message::Collections(message) => self.collections(message),
            Message::Info(message) => self.info(message),
            Message::Rescan(message) => self.rescan(message),
            Message::Together(message) => self.together(message),
        }
    }

    fn result(&mut self, result: Result<(), String>) -> Task<Message> {
        if let Err(error) = result {
            self.notice = Some(error);
        }
        Task::none()
    }

    pub(crate) fn save_config(&self) {
        if let Some(paths) = &self.paths {
            let _ = self.config.save(&paths.config_file);
        }
    }

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

/// The store on disk, or one in memory (and a notice saying so) if it
/// couldn't be opened.
fn open_store(paths: Option<&Paths>, config: &Config) -> (StoreHandle, Option<String>) {
    match paths.map(|p| StoreHandle::open(&p.store_file, &config.device_id)) {
        Some(Ok(store)) => (store, None),
        failed => {
            let reason = match failed {
                Some(Err(error)) => error,
                _ => "there is no home directory".to_owned(),
            };
            let notice = format!(
                "Your listening history couldn't be opened ({reason}), so nothing is kept after you quit."
            );
            let store =
                StoreHandle::in_memory(&config.device_id).expect("an in-memory store always opens");
            (store, Some(notice))
        }
    }
}

/// The audio output and player, set as the config says; with a notice if
/// the equaliser couldn't be set.
fn start_playback(config: &Config) -> (Playback, Option<String>) {
    let mut playback = Playback::start(config.volume, config.sound.device.clone());
    let _ = playback.set_settings(config.settings.player());
    let _ = playback.set_level_volume(config.settings.level_volume);
    let notice = playback
        .apply_sound(&config.sound)
        .err()
        .map(|error| format!("The equaliser couldn't be set: {error}"));
    (playback, notice)
}

/// Where the token is kept.
fn keyring() -> Arc<dyn CredentialStore> {
    // A development build can keep the token in memory instead, so a
    // rebuilt binary doesn't meet the Keychain's "allow this app" prompt.
    if cfg!(debug_assertions) && std::env::var_os("AOIDE_DEV_MEMORY_KEYRING").is_some() {
        Arc::new(crate::credentials::MemoryStore::default())
    } else {
        Arc::new(KeyringStore)
    }
}

/// This install, as the server's devices dashboard shows it.
fn device(config: &Config) -> Device {
    Device::new(device_name(), config.device_id.clone())
}

/// The machine's name, where the system says it.
pub(crate) fn device_name() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|name| name.trim().to_owned())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Linux desktop".to_owned())
}
