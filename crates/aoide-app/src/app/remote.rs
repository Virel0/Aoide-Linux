//! Settings → Account → Remote access: joining Melete's tunnel, leaving it, and
//! keeping the keys. The watch that uses them is `crate::remote`.

use std::sync::Arc;

use aoide_melete::{Error, MeleteClient, TunnelDevices, TunnelJoined};
use aoide_tunnel::{PrivateKey, Settings};
use iced::Task;

use super::{App, Message};
use crate::config::RemoteAccess;
use crate::remote::{self, Way};

#[derive(Debug, Clone)]
pub enum Msg {
    /// The account pane opened: ask whether Melete has a tunnel, and whether
    /// this device is still on it.
    Check,
    Checked(Result<TunnelDevices, String>),
    /// Set up pressed.
    SetUp,
    Joined(Result<(TunnelJoined, PrivateKey), String>),
    /// The keys are in the keyring, or couldn't be put there.
    Kept(Result<Arc<Settings>, String>),
    /// Remove pressed.
    Remove,
    Removed(Result<(), String>),
    /// The keys read back at sign-in, if this account has them.
    Loaded(Option<Arc<Settings>>),
    Way(Way),
}

fn plainly(error: &Error) -> String {
    crate::failure::describe(error).summary
}

/// Melete's 409 to a join: no tunnel on this server.
fn joining_words(error: &Error) -> String {
    match error {
        Error::Http { status: 409, .. } => {
            "Melete's own tunnel isn't set up on this server (MELETE_TUNNEL_PORT).".to_owned()
        }
        Error::Http { status: 429, .. } => {
            "Your account has as many devices on Melete's tunnel as it takes. Remove one in Melete first.".to_owned()
        }
        other => plainly(other),
    }
}

/// Runs keyring work off the async threads: D-Bus calls block.
async fn keyring<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, crate::credentials::CredentialError> + Send + 'static,
) -> Result<T, String> {
    match tokio::task::spawn_blocking(job).await {
        Ok(result) => result.map_err(|e| e.to_string()),
        Err(_) => Err("the keyring didn't answer".to_owned()),
    }
}

impl App {
    /// The watch over how Melete is reached, while remote access is set up.
    pub(super) fn remote_subscription(&self) -> iced::Subscription<Message> {
        let Some(signed) = &self.session else {
            return iced::Subscription::none();
        };
        remote::subscription(
            &signed.connection.client,
            signed.remote.probe.as_ref(),
            signed.remote.settings.as_ref(),
        )
    }

    /// What signing out takes off the tunnel: `signed`'s device, and the
    /// settings to reach Melete through a tunnel of its own if need be.
    pub(super) fn remote_device(
        &self,
        signed: &super::account::Signed,
    ) -> Option<(i64, Option<Arc<Settings>>)> {
        let key = signed.connection.account.keyring_key();
        let mine = self
            .config
            .remote_access
            .as_ref()
            .filter(|r| r.account == key)?;
        Some((mine.device_id, signed.remote.settings.clone()))
    }

    fn remote_state(&mut self) -> Option<&mut remote::State> {
        self.session.as_mut().map(|s| &mut s.remote)
    }

    fn signed_client(&self) -> Option<MeleteClient> {
        self.session.as_ref().map(|s| s.connection.client.clone())
    }

    /// This account's remote access in the config, if it has any.
    fn remote_access(&self) -> Option<&RemoteAccess> {
        let key = self.session.as_ref()?.connection.account.keyring_key();
        self.config
            .remote_access
            .as_ref()
            .filter(|r| r.account == key)
    }

    /// At sign-in: the keys back from the keyring, for the watch.
    pub(super) fn load_remote(&self) -> Task<Message> {
        let Some(remote) = self.remote_access().cloned() else {
            return Task::none();
        };
        let store = Arc::clone(&self.keyring);
        let key = RemoteAccess::keyring_key(&remote.account);
        Task::perform(
            async move {
                let secret = keyring(move || store.get(&key)).await.ok().flatten()?;
                remote::settings(&remote, &secret).map(Arc::new)
            },
            |found| Message::Remote(Msg::Loaded(found)),
        )
    }

    pub(super) fn remote(&mut self, msg: Msg) -> Task<Message> {
        match msg {
            Msg::Check => self.check_remote(),
            Msg::Checked(result) => self.checked_remote(result),
            Msg::SetUp => self.join_remote(),
            Msg::Joined(result) => self.joined_remote(result),
            Msg::Kept(result) => self.kept_remote(result),
            Msg::Remove => self.leave_remote(),
            Msg::Removed(result) => {
                let forget = if result.is_ok() {
                    self.forget_remote()
                } else {
                    Task::none()
                };
                if let Some(state) = self.remote_state() {
                    state.busy = false;
                    state.problem = result.err();
                }
                forget
            }
            Msg::Loaded(settings) => {
                self.use_remote(settings);
                Task::none()
            }
            Msg::Way(way) => {
                if let Some(state) = self.remote_state() {
                    state.way = Some(way);
                }
                Task::none()
            }
        }
    }

    /// The watch starts (or stops) with these settings.
    fn use_remote(&mut self, settings: Option<Arc<Settings>>) {
        let probe = self.session.as_ref().and_then(|s| {
            let client = &s.connection.client;
            MeleteClient::new(client.address().clone(), client.device().clone()).ok()
        });
        if let Some(state) = self.remote_state() {
            state.probe = settings.is_some().then_some(probe).flatten();
            state.way = None;
            state.settings = settings;
        }
    }

    fn check_remote(&mut self) -> Task<Message> {
        let Some(client) = self.signed_client() else {
            return Task::none();
        };
        Task::perform(
            async move { client.tunnel_devices().await },
            |result| match result {
                Err(Error::Unauthorized) => Message::TokenRejected,
                result => Message::Remote(Msg::Checked(result.map_err(|e| plainly(&e)))),
            },
        )
    }

    fn checked_remote(&mut self, result: Result<TunnelDevices, String>) -> Task<Message> {
        let Ok(devices) = result else {
            // An older Melete has no tunnel to offer; nothing else changes.
            if let Some(state) = self.remote_state() {
                state.offered = Some(false);
            }
            return Task::none();
        };
        let gone = self
            .remote_access()
            .is_some_and(|mine| !devices.devices.iter().any(|d| d.id == mine.device_id));
        if let Some(state) = self.remote_state() {
            state.offered = Some(devices.enabled);
        }
        if !gone {
            return Task::none();
        }
        // Taken off the tunnel in Melete: its keys are no use now.
        let task = self.forget_remote();
        if let Some(state) = self.remote_state() {
            state.problem = Some(
                "This computer was taken off Melete's tunnel. Set it up again to use it."
                    .to_owned(),
            );
        }
        task
    }

    fn join_remote(&mut self) -> Task<Message> {
        let Some(client) = self.signed_client() else {
            return Task::none();
        };
        if let Some(state) = self.remote_state() {
            state.busy = true;
            state.problem = None;
        }
        let name = super::device_name();
        Task::perform(
            async move {
                let key = PrivateKey::generate().map_err(|e| e.to_string())?;
                match client.join_tunnel(&name, &key.public_base64()).await {
                    Ok(joined) => Ok((joined, key)),
                    Err(Error::Unauthorized) => Err(String::new()),
                    Err(error) => Err(joining_words(&error)),
                }
            },
            |result| match result {
                Err(words) if words.is_empty() => Message::TokenRejected,
                result => Message::Remote(Msg::Joined(result)),
            },
        )
    }

    fn joined_remote(
        &mut self,
        result: Result<(TunnelJoined, PrivateKey), String>,
    ) -> Task<Message> {
        let (joined, key) = match result {
            Ok(joined) => joined,
            Err(words) => {
                if let Some(state) = self.remote_state() {
                    state.busy = false;
                    state.problem = Some(words);
                }
                return Task::none();
            }
        };
        let Some(account) = self
            .session
            .as_ref()
            .map(|s| s.connection.account.keyring_key())
        else {
            return Task::none();
        };
        let remote = remote::remembered(&account, &joined);
        let Some(settings) =
            remote::settings(&remote, &remote::secret(&key, &joined.preshared_key))
        else {
            return Task::done(Message::Remote(Msg::Kept(Err(
                "Melete's answer didn't read as a tunnel config".to_owned(),
            ))));
        };
        self.config.remote_access = Some(remote);
        self.save_config();
        let store = Arc::clone(&self.keyring);
        let (entry, secret) = (
            RemoteAccess::keyring_key(&account),
            remote::secret(&key, &joined.preshared_key),
        );
        Task::perform(
            async move {
                keyring(move || store.set(&entry, &secret)).await?;
                Ok(Arc::new(settings))
            },
            |result| Message::Remote(Msg::Kept(result)),
        )
    }

    fn kept_remote(&mut self, result: Result<Arc<Settings>, String>) -> Task<Message> {
        match result {
            Ok(settings) => {
                self.use_remote(Some(settings));
                if let Some(state) = self.remote_state() {
                    state.busy = false;
                }
                Task::none()
            }
            Err(words) => {
                // Without its keys the device is no use on the tunnel: take it off.
                let task = self.leave_remote();
                if let Some(state) = self.remote_state() {
                    state.problem = Some(format!("Couldn't keep the tunnel's keys: {words}"));
                }
                task
            }
        }
    }

    /// Takes this computer off the tunnel first, and forgets its keys once
    /// Melete has: forgotten first, a device away from home could no longer
    /// reach Melete to be taken off, and would keep its place there.
    fn leave_remote(&mut self) -> Task<Message> {
        let device = self.remote_access().map(|r| r.device_id);
        let settings = self
            .session
            .as_ref()
            .and_then(|s| s.remote.settings.clone());
        let Some((device, client)) = device.zip(self.signed_client()) else {
            return Task::done(Message::Remote(Msg::Removed(Ok(()))));
        };
        if let Some(state) = self.remote_state() {
            state.busy = true;
            state.problem = None;
        }
        Task::perform(
            leave_device(client, settings, device),
            |result| match result {
                Err(Error::Unauthorized) => Message::TokenRejected,
                result => Message::Remote(Msg::Removed(result.map_err(|e| {
                    format!(
                        "Couldn't take this computer off Melete's tunnel: {}",
                        plainly(&e)
                    )
                }))),
            },
        )
    }

    /// The keys and config gone here, and the tunnel closed.
    pub(super) fn forget_remote(&mut self) -> Task<Message> {
        let Some(remote) = self.config.remote_access.take() else {
            return Task::none();
        };
        self.save_config();
        self.use_remote(None);
        let store = Arc::clone(&self.keyring);
        let entry = RemoteAccess::keyring_key(&remote.account);
        Task::perform(keyring(move || store.delete(&entry)), |_| Message::Ignore)
    }
}

/// Signs out at Melete, taking this computer off the tunnel first: signing
/// out ends the token that needs.
pub(super) fn sign_out(
    client: MeleteClient,
    device: Option<(i64, Option<Arc<Settings>>)>,
) -> Task<Message> {
    let leave = async move {
        if let Some((device, settings)) = device
            && let Err(error) = leave_device(client.clone(), settings, device).await
        {
            log::warn!("couldn't take this computer off Melete's tunnel: {error}");
        }
        crate::playing::leave_server(client).await;
    };
    Task::perform(leave, |()| Message::Ignore)
}

/// How long a removal through the tunnel waits for Melete's answer.
const LEAVE_WAIT: std::time::Duration = std::time::Duration::from_secs(8);

/// Takes `device` off the tunnel: at Melete's usual address, or when that
/// doesn't answer, through a tunnel of its own, since the watch's closes as
/// the device is removed or the session ends. Gone already counts as done.
///
/// # Errors
///
/// The request's, as `leave_tunnel` gives them.
pub(crate) async fn leave_device(
    client: MeleteClient,
    settings: Option<Arc<Settings>>,
    device: i64,
) -> Result<(), Error> {
    let done = |result: Result<(), Error>| match result {
        Err(Error::NotFound { .. }) => Ok(()),
        other => other,
    };
    // A client of its own, on its own route, whatever the watch does meanwhile.
    let own = MeleteClient::new(client.address().clone(), client.device().clone())?;
    let own = match client.credentials() {
        Some(credentials) => own.with_credentials(credentials.clone()),
        None => own,
    };
    let first = own.leave_tunnel(device).await;
    let (Err(Error::Transport(_)), Some(settings)) = (&first, settings) else {
        return done(first);
    };
    let tunnel = aoide_tunnel::Tunnel::start(&settings)
        .await
        .map_err(|e| Error::Transport(e.to_string()))?;
    let local = aoide_melete::ServerAddress::parse(&format!("http://{}", tunnel.local_address()))?;
    own.route().set(Some(&local));
    // Melete cuts a device off the moment it removes it, so the answer to
    // this, sent through that device's own tunnel, may never come.
    let through = tokio::time::timeout(LEAVE_WAIT, own.leave_tunnel(device)).await;
    match through {
        Ok(Ok(())) | Ok(Err(Error::NotFound { .. })) => return Ok(()),
        Ok(Err(Error::Transport(_))) | Err(_) => {}
        Ok(Err(error)) => return Err(error),
    }
    // No answer: gone if the tunnel no longer answers at all, still on it if it does.
    let still_on = matches!(
        tokio::time::timeout(LEAVE_WAIT, own.server_info()).await,
        Ok(Ok(_))
    );
    drop(tunnel);
    if still_on {
        Err(Error::Transport(
            "Melete didn't answer the removal".to_owned(),
        ))
    } else {
        Ok(())
    }
}
