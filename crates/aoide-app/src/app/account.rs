//! Signing in and out: the login form (or Quick Connect), restoring a saved
//! login, and claiming the store for the account.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use aoide_sidecar::{SidecarClient, SyncEngine};
use aoide_store::AccountClaim;
use iced::Task;
use iced::task::Handle;

use super::{App, Entry, Message, Panel, device};
use crate::actions::Menu;
use crate::lyrics_panel::LyricsState;
use crate::nav::BackStack;
use crate::playback::infinity::InfinityTrigger;
use crate::playback::tracks::Measures;
use crate::screens::{PlaylistSummary, Section, login};
use crate::session::{self, Connection, Effect, Event, Phase};
use crate::sync::{self, SyncThrottle};

/// How often a Quick Connect code is asked about while it waits.
const QUICK_CONNECT_POLL: std::time::Duration = std::time::Duration::from_secs(3);

/// Everything that exists only while signed in.
pub(crate) struct Signed {
    pub connection: Connection,
    pub sidecar: Option<SidecarClient>,
    pub sync: Option<sync::Engine>,
    pub throttle: SyncThrottle,
    /// The sync in flight, aborted on sign-out.
    pub syncing: Option<Handle>,
    /// The last sync's outcome in words: what it did, or why it failed.
    pub sync_status: Option<Result<String, String>>,
    /// The store held data from before accounts were claimed: only the
    /// listener can say whose it is.
    pub unknown_owner: bool,
    /// Erase Them was pressed: asking once more before erasing.
    pub confirm_erase: bool,
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
    /// Bumped by every request that replaces the queue, so a slower earlier
    /// one does not land on top of it.
    pub play_generation: u64,
    /// The server measures tracks (false after a 404 on both endpoints).
    pub measures_available: bool,
    /// A batch of the queue is being measured.
    pub measuring: bool,
    /// Tracks already asked about for the queue this session.
    pub measure_asked: HashSet<String>,
    /// The server answers `/aoide/next` (false after a 404).
    pub next_available: bool,
    /// When the saved queue last followed the playhead, by the app's clock.
    pub queue_saved_at: u64,
    pub infinity: InfinityTrigger,
    pub lyrics: Option<LyricsState>,
    /// The lyric line last scrolled to.
    pub lyric_line: Option<usize>,
    /// Handoff and the resume grid.
    pub handoff: crate::handoff::Handoff,
    /// This week's Discover list.
    pub discover: crate::screens::discover::Discover,
    /// The Get Info sheet, while it is open.
    pub info: Option<crate::info::Sheet>,
    /// Listening together with other devices.
    pub together: crate::together::Together,
}

impl Signed {
    /// A fresh session: nothing open yet, the library still to be found.
    fn new(
        connection: Connection,
        sidecar: Option<SidecarClient>,
        sync: Option<sync::Engine>,
        handoff: crate::handoff::Handoff,
    ) -> Self {
        Self {
            connection,
            sidecar,
            sync,
            throttle: SyncThrottle::default(),
            syncing: None,
            sync_status: None,
            unknown_owner: false,
            confirm_erase: false,
            section: Section::Home,
            nav: None,
            next_key: 1,
            playlists: Vec::new(),
            menu: None,
            favourites: HashMap::new(),
            panel: None,
            measures: Measures::default(),
            scrub: None,
            play_generation: 0,
            measures_available: true,
            measuring: false,
            measure_asked: HashSet::new(),
            next_available: true,
            queue_saved_at: 0,
            infinity: InfinityTrigger::default(),
            lyrics: None,
            lyric_line: None,
            handoff,
            discover: Default::default(),
            info: None,
            together: Default::default(),
        }
    }
}

impl App {
    pub(super) fn login_message(&mut self, message: login::Message) -> Task<Message> {
        match self.login.update(message) {
            login::Action::None => Task::none(),
            login::Action::Focus(id) => iced::widget::operation::focus(id),
            login::Action::SignIn(form) => {
                self.phase = session::transition(self.phase, Event::Submitted).0;
                Task::perform(
                    session::sign_in(form, device(&self.config), Arc::clone(&self.keyring)),
                    Message::SignedIn,
                )
            }
            login::Action::StartQuickConnect(server) => {
                self.phase = session::transition(self.phase, Event::Submitted).0;
                self.quick_attempts += 1;
                let attempt = self.quick_attempts;
                Task::perform(
                    session::start_quick_connect(server, device(&self.config)),
                    move |result| Message::QuickStarted(attempt, result),
                )
            }
            login::Action::StopQuickConnect => {
                self.quick = None;
                self.phase = session::transition(self.phase, Event::SignInFailed).0;
                Task::none()
            }
        }
    }

    pub(super) fn quick_started(
        &mut self,
        attempt: u64,
        result: Result<(aoide_melete::MeleteClient, aoide_melete::QuickConnectCode), String>,
    ) -> Task<Message> {
        if attempt != self.quick_attempts || self.phase != Phase::SigningIn {
            return Task::none();
        }
        match result {
            Ok((client, code)) => {
                self.login
                    .update(login::Message::QuickStarted(Ok(code.code)));
                self.quick = Some((attempt, client, code.secret));
                self.poll_quick()
            }
            Err(error) => self.signed_in(Err(error)),
        }
    }

    pub(super) fn quick_polled(
        &mut self,
        attempt: u64,
        result: Result<Option<Connection>, String>,
    ) -> Task<Message> {
        if self
            .quick
            .as_ref()
            .is_none_or(|(waiting, ..)| *waiting != attempt)
        {
            return Task::none();
        }
        match result {
            Ok(None) => self.poll_quick(),
            Ok(Some(connection)) => {
                self.quick = None;
                self.signed_in(Ok(connection))
            }
            Err(error) => {
                self.quick = None;
                self.signed_in(Err(error))
            }
        }
    }

    /// Asks about the waiting code again in a few seconds.
    fn poll_quick(&self) -> Task<Message> {
        let Some((attempt, client, secret)) = self.quick.clone() else {
            return Task::none();
        };
        let keyring = Arc::clone(&self.keyring);
        Task::perform(
            async move {
                tokio::time::sleep(QUICK_CONNECT_POLL).await;
                session::poll_quick_connect(client, secret, keyring).await
            },
            move |result| Message::QuickPolled(attempt, result),
        )
    }

    pub(super) fn restored(&mut self, result: Result<Option<Connection>, String>) -> Task<Message> {
        match result {
            Ok(Some(connection)) => self.enter(Event::Restored, connection),
            Ok(None) => {
                self.phase = session::transition(self.phase, Event::NothingToRestore).0;
                iced::widget::operation::focus(self.login.first_field())
            }
            Err(error) => {
                self.phase = session::transition(self.phase, Event::NothingToRestore).0;
                self.login.error = Some(format!("Your saved login couldn't be read: {error}."));
                Task::none()
            }
        }
    }

    pub(super) fn signed_in(&mut self, result: Result<Connection, String>) -> Task<Message> {
        match result {
            Ok(connection) => {
                self.login = login::State::new(Some(connection.account.server.clone()));
                self.enter(Event::SignInSucceeded, connection)
            }
            Err(error) => {
                self.phase = session::transition(self.phase, Event::SignInFailed).0;
                self.login.failed(error);
                Task::none()
            }
        }
    }

    pub(super) fn claimed(&mut self, result: Result<AccountClaim, String>) -> Task<Message> {
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

    pub(super) fn confirm_erase(&mut self, ask: bool) -> Task<Message> {
        if let Some(signed) = self.session.as_mut() {
            signed.confirm_erase = ask;
        }
        Task::none()
    }

    pub(super) fn resolve_owner(&mut self, keep: bool) -> Task<Message> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        signed.unknown_owner = false;
        let user = signed.connection.account.store_owner();
        self.store.send(move |w| {
            let _ = if keep {
                w.store.keep_and_claim(&user)
            } else {
                w.store.erase_and_claim(&user)
            };
        });
        Task::batch([self.request_sync(), self.after_claim()])
    }

    pub(super) fn token_rejected(&mut self) -> Task<Message> {
        let task = self.leave(Event::TokenRejected);
        self.login.error =
            Some("The server no longer accepts this login. Sign in again.".to_owned());
        task
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
        crate::failure::set_server(Some(&connection.account.server));
        self.save_config();

        let sidecar = SidecarClient::new(&connection.account.server, connection.token()).ok();
        let blobs = crate::covers::open_blobs(self.paths.as_ref());
        let sync = sidecar.clone().map(|client| {
            // The user's id tells a share granted to them from one they granted.
            let engine = SyncEngine::new(self.store.shared(), client)
                .with_user_id(connection.account.user_id.clone());
            // Chosen covers are uploaded from where they are kept.
            Arc::new(match blobs {
                Some(blobs) => engine.with_image_bytes(Arc::new(blobs)),
                None => engine,
            })
        });
        let handoff =
            crate::handoff::Handoff::open(self.paths.as_ref(), &connection.account.server);
        self.session = Some(Signed::new(connection, sidecar, sync, handoff));

        let mut tasks = Vec::new();
        for effect in effects {
            match effect {
                // The sync and everything that reads the store wait for the
                // claim (see `Message::Claimed`).
                Effect::AdoptAccount => tasks.push(self.adopt_account()),
                Effect::Connect => {
                    let section = self.session.as_ref().map(|s| s.section.clone());
                    if let Some(section) = section {
                        tasks.push(self.show_section(section));
                    }
                }
                _ => {}
            }
        }
        Task::batch(tasks)
    }

    /// What runs once the store is known to be this account's.
    fn after_claim(&mut self) -> Task<Message> {
        // Before the playlists are read: the store thread runs jobs in order.
        let now = aoide_store::now_millis();
        self.store.send(move |w| {
            let _ = w.store.ensure_built_in_playlists(now);
        });
        Task::batch([self.load_playlists(), self.restore_queue()])
    }

    /// Claims the store for the signed-in account before anything reads it.
    fn adopt_account(&self) -> Task<Message> {
        let Some(signed) = &self.session else {
            return Task::none();
        };
        let user = signed.connection.account.store_owner();
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
    pub(super) fn leave(&mut self, event: Event) -> Task<Message> {
        let (phase, effects) = session::transition(self.phase, event);
        if phase == self.phase {
            return Task::none();
        }
        self.phase = phase;
        crate::failure::set_server(None);
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
                        // What waited to be published was this account's.
                        w.broadcast = aoide_store::QueueBroadcast::new();
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
                    tasks.push(Task::perform(crate::playing::leave_server(client), |()| {
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
}
