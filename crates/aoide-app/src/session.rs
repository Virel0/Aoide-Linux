//! Who is signed in: the session's states, signing in and out, and the words
//! a failure is reported in.
//!
//! Signing in checks the address really is Melete before asking for the
//! password, so a wrong address says so rather than "wrong password". Quick
//! Connect signs in without a password: the app shows a code, someone signed
//! in to Melete's web UI approves it, and the app, asking every few seconds,
//! gets a session of its own. The
//! token goes to the system keyring before anything else is written; if the
//! keyring will not keep it, signing in fails rather than leaving a login that
//! vanishes at the next launch. Signing out never waits on the network: the
//! screen leaves at once and the server is told afterwards, briefly.

use std::sync::Arc;
use std::time::Duration;

use aoide_melete::{Credentials, Device, Error, MeleteClient, QuickConnectCode, ServerAddress};

use crate::config::Account;
use crate::credentials::CredentialStore;

/// How long the server gets to hear about a sign-out.
pub const SIGN_OUT_GRACE: Duration = Duration::from_secs(5);

/// Where the session stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// At launch, while the keyring is read.
    Restoring,
    SignedOut,
    /// The sign-in form was submitted and the server has not answered.
    SigningIn,
    SignedIn,
}

/// What moves the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The keyring had a token for the saved account.
    Restored,
    /// No saved account, or no token for it.
    NothingToRestore,
    Submitted,
    SignInSucceeded,
    SignInFailed,
    SignOut,
    /// The server stopped accepting the token: the password changed, or the
    /// session was signed out from Melete's web UI.
    TokenRejected,
}

/// What the app does on a transition, in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Claim the store for this account, erasing another account's data.
    AdoptAccount,
    /// Start the sync loop and load the library.
    Connect,
    /// Stop the music and stop reporting it.
    StopPlayback,
    /// Empty this device's saved queue (a local write; it travels with the
    /// next sync).
    ClearSavedQueue,
    /// Forget the token and the saved account.
    ForgetCredentials,
    /// Tell the server, in the background, with a short timeout.
    NotifyServer,
}

/// The transition table. Anything not listed leaves the phase as it is and
/// does nothing, which is what makes repeats (several 401s at once) harmless.
pub fn transition(phase: Phase, event: Event) -> (Phase, Vec<Effect>) {
    use Effect::*;
    match (phase, event) {
        (Phase::Restoring, Event::Restored) => (Phase::SignedIn, vec![AdoptAccount, Connect]),
        (Phase::Restoring, Event::NothingToRestore) => (Phase::SignedOut, Vec::new()),
        (Phase::SignedOut, Event::Submitted) => (Phase::SigningIn, Vec::new()),
        (Phase::SigningIn, Event::SignInSucceeded) => {
            (Phase::SignedIn, vec![AdoptAccount, Connect])
        }
        (Phase::SigningIn, Event::SignInFailed) => (Phase::SignedOut, Vec::new()),
        (Phase::SignedIn, Event::SignOut) => (
            Phase::SignedOut,
            vec![
                StopPlayback,
                ClearSavedQueue,
                ForgetCredentials,
                NotifyServer,
            ],
        ),
        // The token is already dead: there is nobody to tell.
        (Phase::SignedIn, Event::TokenRejected) => (
            Phase::SignedOut,
            vec![StopPlayback, ClearSavedQueue, ForgetCredentials],
        ),
        (phase, _) => (phase, Vec::new()),
    }
}

/// What the form holds. The password is dropped as soon as it has been sent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SignInForm {
    pub server: String,
    pub username: String,
    pub password: String,
}

impl SignInForm {
    /// Checks the form before anything is sent: an address that parses, and a
    /// username. The password is sent as typed; the server decides.
    pub fn validate(&self) -> Result<(ServerAddress, String), String> {
        if self.server.trim().is_empty() {
            return Err("Enter your server's address.".to_owned());
        }
        let address = ServerAddress::parse(&self.server).map_err(|e| describe(&e))?;
        let username = self.username.trim();
        if username.is_empty() {
            return Err("Enter your username.".to_owned());
        }
        Ok((address, username.to_owned()))
    }

    pub fn can_submit(&self) -> bool {
        !self.server.trim().is_empty() && !self.username.trim().is_empty()
    }
}

/// A failure in words for a person, as the phone puts them.
pub fn describe(error: &Error) -> String {
    match error {
        Error::MalformedAddress(raw) => format!("\"{}\" isn't a valid server address.", raw.trim()),
        Error::NotMelete(host) => {
            format!("{host} responded, but doesn't look like a Melete server.")
        }
        Error::Http { status: 429, .. } => {
            "Too many tries with this username. Wait 15 minutes, then try again.".to_owned()
        }
        Error::Unauthorized => "Your username or password wasn't accepted.".to_owned(),
        // The rest as every screen words them: a sentence, never the
        // transport's own text.
        other => crate::failure::describe_at(other, None).summary,
    }
}

/// A signed-in session's connection.
#[derive(Debug, Clone)]
pub struct Connection {
    pub client: MeleteClient,
    pub account: Account,
    /// The runtime the client's connections live on, for the player's
    /// downloads to ask for stream links from their own threads.
    pub runtime: tokio::runtime::Handle,
}

impl Connection {
    /// Where the player asks for stream links.
    pub fn streams(&self) -> crate::playback::tracks::Streams {
        crate::playback::tracks::Streams {
            client: self.client.clone(),
            runtime: self.runtime.clone(),
        }
    }

    pub fn token(&self) -> &str {
        self.client
            .credentials()
            .map(|c| c.access_token.as_str())
            .unwrap_or_default()
    }
}

/// Signs in: probe, authenticate, keep the token. The password goes no further.
pub async fn sign_in(
    form: SignInForm,
    device: Device,
    keyring: Arc<dyn CredentialStore>,
) -> Result<Connection, String> {
    let (address, username) = form.validate()?;
    let anonymous = probe(address, device).await?;
    let credentials = anonymous
        .sign_in(&username, &form.password)
        .await
        .map_err(|e| describe(&e))?;
    keep(anonymous, credentials, keyring).await
}

/// Quick Connect's first half: the address is checked and a code made for
/// this device, to show until it is approved or runs out.
pub async fn start_quick_connect(
    server: String,
    device: Device,
) -> Result<(MeleteClient, QuickConnectCode), String> {
    if server.trim().is_empty() {
        return Err("Enter your server's address.".to_owned());
    }
    let address = ServerAddress::parse(&server).map_err(|e| describe(&e))?;
    let anonymous = probe(address, device).await?;
    let code = anonymous
        .start_quick_connect()
        .await
        .map_err(|e| describe(&e))?;
    Ok((anonymous, code))
}

/// Quick Connect's second half, asked every few seconds: `Ok(None)` while the
/// code waits; once approved, the token is kept as signing in keeps it.
pub async fn poll_quick_connect(
    anonymous: MeleteClient,
    secret: String,
    keyring: Arc<dyn CredentialStore>,
) -> Result<Option<Connection>, String> {
    match anonymous.poll_quick_connect(&secret).await {
        Ok(None) => Ok(None),
        Ok(Some(credentials)) => keep(anonymous, credentials, keyring).await.map(Some),
        Err(Error::NotFound { .. }) => {
            Err("The code ran out before it was approved. Start again for a new one.".to_owned())
        }
        Err(error) => Err(describe(&error)),
    }
}

/// A signed-out client for the address, once it answers as Melete does.
async fn probe(address: ServerAddress, device: Device) -> Result<MeleteClient, String> {
    let anonymous = MeleteClient::new(address, device).map_err(|e| describe(&e))?;
    anonymous.server_info().await.map_err(|e| describe(&e))?;
    Ok(anonymous)
}

/// Keeps a new session's token in the keyring, then hands the session over.
async fn keep(
    anonymous: MeleteClient,
    credentials: Credentials,
    keyring: Arc<dyn CredentialStore>,
) -> Result<Connection, String> {
    let account = Account {
        server: anonymous.address().to_string(),
        user_id: credentials.user_id.clone(),
        username: credentials.username.clone(),
    };
    let key = account.keyring_key();
    let token = credentials.access_token.clone();
    blocking(move || keyring.set(&key, &token))
        .await
        .map_err(|e| format!("Signed in, but {e}, so the login couldn't be kept."))?;
    Ok(Connection {
        client: anonymous.with_credentials(credentials),
        account,
        runtime: tokio::runtime::Handle::current(),
    })
}

/// Restores the saved account from its keyring token. `Ok(None)` when there
/// is no token: a profile without its secret is a signed-out device.
pub async fn restore(
    account: Account,
    device: Device,
    keyring: Arc<dyn CredentialStore>,
) -> Result<Option<Connection>, String> {
    let key = account.keyring_key();
    let Some(token) = blocking(move || keyring.get(&key))
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let address = ServerAddress::parse(&account.server).map_err(|e| describe(&e))?;
    let client = MeleteClient::new(address, device)
        .map_err(|e| describe(&e))?
        .with_credentials(Credentials {
            user_id: account.user_id.clone(),
            access_token: token,
            username: account.username.clone(),
        });
    Ok(Some(Connection {
        client,
        account,
        runtime: tokio::runtime::Handle::current(),
    }))
}

/// Forgets the token. Errors are reported but change nothing: the session is
/// already over locally.
pub async fn forget(account: Account, keyring: Arc<dyn CredentialStore>) -> Result<(), String> {
    let key = account.keyring_key();
    blocking(move || keyring.delete(&key))
        .await
        .map_err(|e| e.to_string())
}

/// Runs keyring work off the async threads: D-Bus calls and Keychain prompts block.
async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, crate::credentials::CredentialError> + Send + 'static,
) -> Result<T, crate::credentials::CredentialError> {
    tokio::task::spawn_blocking(job)
        .await
        .unwrap_or_else(|e| Err(crate::credentials::CredentialError(e.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::MemoryStore;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn launch_restores_or_shows_the_form() {
        assert_eq!(
            transition(Phase::Restoring, Event::Restored),
            (Phase::SignedIn, vec![Effect::AdoptAccount, Effect::Connect])
        );
        assert_eq!(
            transition(Phase::Restoring, Event::NothingToRestore),
            (Phase::SignedOut, vec![])
        );
    }

    #[test]
    fn signing_in_goes_through_signing_in() {
        let (phase, _) = transition(Phase::SignedOut, Event::Submitted);
        assert_eq!(phase, Phase::SigningIn);
        assert_eq!(
            transition(phase, Event::SignInFailed),
            (Phase::SignedOut, vec![])
        );
        assert_eq!(
            transition(phase, Event::SignInSucceeded),
            (Phase::SignedIn, vec![Effect::AdoptAccount, Effect::Connect])
        );
        // A second submit while one is in flight does nothing.
        assert_eq!(
            transition(phase, Event::Submitted),
            (Phase::SigningIn, vec![])
        );
    }

    #[test]
    fn signing_out_leaves_at_once_and_tells_the_server_last() {
        let (phase, effects) = transition(Phase::SignedIn, Event::SignOut);
        assert_eq!(phase, Phase::SignedOut);
        assert_eq!(effects.first(), Some(&Effect::StopPlayback));
        assert_eq!(effects.last(), Some(&Effect::NotifyServer));
        assert!(effects.contains(&Effect::ForgetCredentials));
        assert!(effects.contains(&Effect::ClearSavedQueue));
    }

    #[test]
    fn a_rejected_token_signs_out_once() {
        let (phase, effects) = transition(Phase::SignedIn, Event::TokenRejected);
        assert_eq!(phase, Phase::SignedOut);
        assert!(!effects.contains(&Effect::NotifyServer));
        assert_eq!(
            transition(phase, Event::TokenRejected),
            (Phase::SignedOut, vec![])
        );
    }

    #[test]
    fn the_form_needs_an_address_and_a_username() {
        let mut form = SignInForm::default();
        assert_eq!(form.validate().unwrap_err(), "Enter your server's address.");
        form.server = "ftp://example.com".into();
        assert_eq!(
            form.validate().unwrap_err(),
            "\"ftp://example.com\" isn't a valid server address."
        );
        form.server = "melete.local:8100".into();
        assert_eq!(form.validate().unwrap_err(), "Enter your username.");
        form.username = "  alice ".into();
        let (address, username) = form.validate().unwrap();
        assert_eq!(address.to_string(), "http://melete.local:8100");
        assert_eq!(username, "alice");
        assert!(form.can_submit());
    }

    #[test]
    fn failures_read_as_plain_words() {
        assert_eq!(
            describe(&Error::Unauthorized),
            "Your username or password wasn't accepted."
        );
        assert_eq!(
            describe(&Error::NotMelete("music.example.net".into())),
            "music.example.net responded, but doesn't look like a Melete server."
        );
        let unreachable = describe(&Error::Transport(
            "tcp connect error: Connection refused (os error 61)".into(),
        ));
        assert!(
            unreachable.starts_with("Can't reach your server."),
            "{unreachable}"
        );
        assert!(!unreachable.contains("os error"), "{unreachable}");
        let broken = describe(&Error::Http {
            status: 500,
            path: "Users/AuthenticateByName".into(),
            detail: Some("System.NullReferenceException".into()),
        });
        assert!(
            broken.starts_with("Your server ran into a problem"),
            "{broken}"
        );
        assert!(!broken.contains("NullReference"), "{broken}");
    }

    fn device() -> Device {
        Device::new("Test", "device-1")
    }

    async fn melete() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/health"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"status": "ok", "version": "0.1.0"})),
            )
            .mount(&server)
            .await;
        server
    }

    fn session(id: i64, username: &str, token: &str) -> serde_json::Value {
        serde_json::json!({
            "token": token,
            "user": {"id": id, "username": username, "is_admin": false},
        })
    }

    fn form(server: &MockServer, password: &str) -> SignInForm {
        SignInForm {
            server: server.uri(),
            username: "alice".into(),
            password: password.into(),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn signing_in_keeps_the_token_and_never_the_password() {
        let server = melete().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/login"))
            .and(body_json(serde_json::json!({
                "username": "alice", "password": "pw-123", "device_name": "Test"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(session(1, "Alice", "tok-1")))
            .mount(&server)
            .await;
        let keyring = Arc::new(MemoryStore::default());
        let connection = sign_in(form(&server, "pw-123"), device(), keyring.clone())
            .await
            .unwrap();
        assert_eq!(connection.account.user_id, "1");
        assert_eq!(connection.account.username, "Alice");
        assert_eq!(connection.token(), "tok-1");
        let key = connection.account.keyring_key();
        assert_eq!(keyring.get(&key).unwrap().as_deref(), Some("tok-1"));
        let config = serde_json::to_string(&connection.account).unwrap();
        assert!(!config.contains("pw-123") && !config.contains("tok-1"));

        // And it restores from the keyring alone.
        let restored = restore(connection.account.clone(), device(), keyring.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored.token(), "tok-1");
        forget(connection.account.clone(), keyring.clone())
            .await
            .unwrap();
        assert!(
            restore(connection.account, device(), keyring)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_wrong_password_says_so() {
        let server = melete().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/login"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let error = sign_in(
            form(&server, "nope"),
            device(),
            Arc::new(MemoryStore::default()),
        )
        .await
        .unwrap_err();
        assert_eq!(error, "Your username or password wasn't accepted.");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn something_that_isnt_melete_is_named_before_the_password_is_sent() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/health"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>router</html>"))
            .mount(&server)
            .await;
        let error = sign_in(
            form(&server, "pw"),
            device(),
            Arc::new(MemoryStore::default()),
        )
        .await
        .unwrap_err();
        assert!(
            error.ends_with("doesn't look like a Melete server."),
            "{error}"
        );
        let sent_password = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.url.path().contains("login"));
        assert!(!sent_password);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_keyring_that_refuses_fails_the_sign_in() {
        let server = melete().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/login"))
            .respond_with(ResponseTemplate::new(200).set_body_json(session(1, "Alice", "tok-1")))
            .mount(&server)
            .await;
        let error = sign_in(
            form(&server, "pw"),
            device(),
            Arc::new(MemoryStore::failing()),
        )
        .await
        .unwrap_err();
        assert!(error.starts_with("Signed in, but"), "{error}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn quick_connect_waits_then_keeps_the_approved_session() {
        let server = melete().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/quick-connect"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": "482913", "secret": "s3cret", "expires_in": 300
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/quick-connect/poll"))
            .respond_with(ResponseTemplate::new(202))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/quick-connect/poll"))
            .respond_with(ResponseTemplate::new(200).set_body_json(session(2, "bea", "tok-2")))
            .mount(&server)
            .await;
        let keyring = Arc::new(MemoryStore::default());
        let (client, code) = start_quick_connect(server.uri(), device()).await.unwrap();
        assert_eq!(code.code, "482913");
        let waiting = poll_quick_connect(client.clone(), code.secret.clone(), keyring.clone())
            .await
            .unwrap();
        assert!(waiting.is_none());
        let connection = poll_quick_connect(client, code.secret, keyring.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(connection.account.username, "bea");
        let key = connection.account.keyring_key();
        assert_eq!(keyring.get(&key).unwrap().as_deref(), Some("tok-2"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_quick_connect_code_that_ran_out_says_start_again() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/quick-connect/poll"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let client =
            MeleteClient::new(ServerAddress::parse(&server.uri()).unwrap(), device()).unwrap();
        let error = poll_quick_connect(client, "old".into(), Arc::new(MemoryStore::default()))
            .await
            .unwrap_err();
        assert!(error.starts_with("The code ran out"), "{error}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn telling_the_server_gives_up_after_the_grace_period() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/logout"))
            .respond_with(ResponseTemplate::new(204).set_delay(Duration::from_secs(30)))
            .mount(&server)
            .await;
        let client = MeleteClient::new(ServerAddress::parse(&server.uri()).unwrap(), device())
            .unwrap()
            .with_credentials(Credentials {
                user_id: "u1".into(),
                access_token: "tok".into(),
                username: "alice".into(),
            });
        let started = std::time::Instant::now();
        crate::playing::leave_server(client).await;
        assert!(started.elapsed() < SIGN_OUT_GRACE + Duration::from_secs(2));
    }
}
