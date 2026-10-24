//! Who is signed in: the session's states, signing in and out, and the words
//! a failure is reported in.
//!
//! Signing in checks the address really is Jellyfin before asking for the
//! password, so a wrong address says so rather than "wrong password". The
//! token goes to the system keyring before anything else is written; if the
//! keyring will not keep it, signing in fails rather than leaving a login that
//! vanishes at the next launch. Signing out never waits on the network: the
//! screen leaves at once and the server is told afterwards, briefly.

use std::sync::Arc;
use std::time::Duration;

use aoide_jellyfin::{Credentials, Device, Error, JellyfinClient, ServerAddress};

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
    /// device was removed from the dashboard.
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
    /// username. An empty password is allowed; Jellyfin allows one.
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
        Error::NotJellyfin(host) => {
            format!("{host} responded, but doesn't look like a Jellyfin server.")
        }
        Error::Unauthorized => "Your username or password wasn't accepted.".to_owned(),
        Error::NotSignedIn => "You're not signed in.".to_owned(),
        Error::NotFound { path } => format!("The server has no {path}."),
        Error::Http {
            status,
            path,
            detail,
        } => match detail {
            Some(detail) => format!("{path} returned an error (HTTP {status}): {detail}"),
            None => format!("{path} returned an error (HTTP {status})."),
        },
        Error::Decoding { .. } => "The server sent a response Aoide couldn't read.".to_owned(),
        Error::Transport(message) => format!(
            "Couldn't reach the server. Check the address and that the server is running. ({message})"
        ),
    }
}

/// A signed-in session's connection.
#[derive(Debug, Clone)]
pub struct Connection {
    pub client: JellyfinClient,
    pub account: Account,
}

impl Connection {
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
    let anonymous = JellyfinClient::new(address.clone(), device).map_err(|e| describe(&e))?;
    anonymous
        .public_system_info()
        .await
        .map_err(|e| describe(&e))?;
    let credentials = anonymous
        .authenticate_by_name(&username, &form.password)
        .await
        .map_err(|e| describe(&e))?;
    let account = Account {
        server: address.to_string(),
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
    let client = JellyfinClient::new(address, device)
        .map_err(|e| describe(&e))?
        .with_credentials(Credentials {
            user_id: account.user_id.clone(),
            access_token: token,
            username: account.username.clone(),
        });
    Ok(Some(Connection { client, account }))
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
        form.server = "jellyfin.local:8096".into();
        assert_eq!(form.validate().unwrap_err(), "Enter your username.");
        form.username = "  alice ".into();
        let (address, username) = form.validate().unwrap();
        assert_eq!(address.to_string(), "http://jellyfin.local:8096");
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
            describe(&Error::NotJellyfin("music.example.net".into())),
            "music.example.net responded, but doesn't look like a Jellyfin server."
        );
        assert!(
            describe(&Error::Transport("connection refused".into()))
                .starts_with("Couldn't reach the server.")
        );
        assert_eq!(
            describe(&Error::Http {
                status: 500,
                path: "Users/AuthenticateByName".into(),
                detail: None
            }),
            "Users/AuthenticateByName returned an error (HTTP 500)."
        );
    }

    fn device() -> Device {
        Device::new("Test", "device-1")
    }

    async fn jellyfin() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/System/Info/Public"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Id": "server-1", "Version": "10.11.2", "ServerName": "Test"
            })))
            .mount(&server)
            .await;
        server
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
        let server = jellyfin().await;
        Mock::given(method("POST"))
            .and(path("/Users/AuthenticateByName"))
            .and(body_json(
                serde_json::json!({"Username": "alice", "Pw": "pw-123"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "User": {"Id": "u1", "Name": "Alice"}, "AccessToken": "tok-1"
            })))
            .mount(&server)
            .await;
        let keyring = Arc::new(MemoryStore::default());
        let connection = sign_in(form(&server, "pw-123"), device(), keyring.clone())
            .await
            .unwrap();
        assert_eq!(connection.account.user_id, "u1");
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
        let server = jellyfin().await;
        Mock::given(method("POST"))
            .and(path("/Users/AuthenticateByName"))
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
    async fn something_that_isnt_jellyfin_is_named_before_the_password_is_sent() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/System/Info/Public"))
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
            error.ends_with("doesn't look like a Jellyfin server."),
            "{error}"
        );
        let sent_password = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|r| r.url.path().contains("Authenticate"));
        assert!(!sent_password);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_keyring_that_refuses_fails_the_sign_in() {
        let server = jellyfin().await;
        Mock::given(method("POST"))
            .and(path("/Users/AuthenticateByName"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "User": {"Id": "u1", "Name": "Alice"}, "AccessToken": "tok-1"
            })))
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
    async fn telling_the_server_gives_up_after_the_grace_period() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/Sessions/Logout"))
            .respond_with(ResponseTemplate::new(204).set_delay(Duration::from_secs(30)))
            .mount(&server)
            .await;
        let client = JellyfinClient::new(ServerAddress::parse(&server.uri()).unwrap(), device())
            .unwrap()
            .with_credentials(Credentials {
                user_id: "u1".into(),
                access_token: "tok".into(),
                username: "alice".into(),
            });
        let started = std::time::Instant::now();
        crate::playing::leave_server(client, None).await;
        assert!(started.elapsed() < SIGN_OUT_GRACE + Duration::from_secs(2));
    }
}
