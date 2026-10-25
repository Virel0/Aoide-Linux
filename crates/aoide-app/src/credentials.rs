//! Where the access token is kept: the system keyring (on Linux through oo7:
//! the Secret Service, or inside a Flatpak the Secret portal; the Keychain on
//! macOS), behind a trait so tests use memory.
//!
//! Only the token. The password is used once, to sign in, and never stored.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

/// The keyring's service name: the app id.
pub const SERVICE: &str = "io.github.Virel0.Aoide";

/// A keyring failure, in words that say what to check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialError(pub String);

impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CredentialError {}

/// Secrets by account key. Calls may block (D-Bus, a Keychain prompt), so the
/// app makes them off the UI thread.
pub trait CredentialStore: Send + Sync {
    /// `None` when there is no entry.
    fn get(&self, key: &str) -> Result<Option<String>, CredentialError>;
    fn set(&self, key: &str, secret: &str) -> Result<(), CredentialError>;
    /// Removing an entry that is not there is not an error.
    fn delete(&self, key: &str) -> Result<(), CredentialError>;
}

/// The system keyring.
#[derive(Debug, Default, Clone, Copy)]
pub struct KeyringStore;

#[cfg(target_os = "macos")]
impl KeyringStore {
    fn entry(key: &str) -> Result<keyring::Entry, CredentialError> {
        keyring::Entry::new(SERVICE, key).map_err(describe)
    }
}

#[cfg(target_os = "macos")]
fn describe(error: keyring::Error) -> CredentialError {
    CredentialError(match error {
        keyring::Error::NoStorageAccess(_) | keyring::Error::PlatformFailure(_) => format!(
            "the system keyring isn't available ({error}). On Linux, a Secret Service \
             such as GNOME Keyring or KWallet needs to be running"
        ),
        other => format!("the system keyring refused: {other}"),
    })
}

#[cfg(target_os = "macos")]
impl CredentialStore for KeyringStore {
    fn get(&self, key: &str) -> Result<Option<String>, CredentialError> {
        match Self::entry(key)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(describe(error)),
        }
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), CredentialError> {
        Self::entry(key)?.set_password(secret).map_err(describe)
    }

    fn delete(&self, key: &str) -> Result<(), CredentialError> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(describe(error)),
        }
    }
}

#[cfg(target_os = "linux")]
impl CredentialStore for KeyringStore {
    fn get(&self, key: &str) -> Result<Option<String>, CredentialError> {
        linux::run(linux::get(key))
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), CredentialError> {
        linux::run(linux::set(key, secret))
    }

    fn delete(&self, key: &str) -> Result<(), CredentialError> {
        linux::run(linux::delete(key))
    }
}

/// The keyring on Linux, through oo7.
///
/// Outside a sandbox, the Secret Service (GNOME Keyring, KWallet,
/// KeePassXC). Inside a Flatpak, the Secret portal: it hands the app a key
/// for an encrypted file of its own, so the app never sees the rest of the
/// keyring. Where the portal fails, the host's Secret Service is tried, which
/// works only if the user granted it (`flatpak override`), and the error says
/// how.
///
/// Items carry the attributes keyring-rs 3.6 wrote (service, username,
/// target "default", application "rust-keyring"), so a token saved before
/// this store is found, and replaced in place.
#[cfg(target_os = "linux")]
mod linux {
    use std::collections::HashMap;
    use std::sync::LazyLock;

    use tokio::sync::OnceCell;

    use super::{CredentialError, SERVICE};

    /// oo7 is async and its D-Bus connection lives on the runtime that made
    /// it: one small runtime of the store's own, kept for the process.
    static RUNTIME: LazyLock<Option<tokio::runtime::Runtime>> = LazyLock::new(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("aoide-keyring")
            .enable_all()
            .build()
            .ok()
    });

    enum Backend {
        /// oo7's choice: the portal in a sandbox, the Secret Service outside.
        Keyring(oo7::Keyring),
        /// The host's Secret Service from inside a Flatpak, once the portal
        /// failed and the user has granted it.
        Host(oo7::dbus::Collection),
    }

    /// Made on first use; a failure is tried again on the next call.
    static BACKEND: OnceCell<Backend> = OnceCell::const_new();

    /// Runs `job` on the store's runtime, from a thread of its own: callers
    /// are blocking code, which may itself be on another runtime's thread.
    pub fn run<T: Send>(
        job: impl Future<Output = Result<T, CredentialError>> + Send,
    ) -> Result<T, CredentialError> {
        let runtime = RUNTIME.as_ref().ok_or_else(|| {
            CredentialError("the keyring couldn't be reached (no runtime for it)".into())
        })?;
        std::thread::scope(|scope| {
            scope
                .spawn(|| runtime.block_on(job))
                .join()
                .unwrap_or_else(|_| Err(CredentialError("the keyring call failed".into())))
        })
    }

    fn in_flatpak() -> bool {
        std::path::Path::new("/.flatpak-info").exists()
    }

    fn unavailable(error: impl std::fmt::Display) -> CredentialError {
        CredentialError(format!(
            "the system keyring isn't available ({error}). A Secret Service such as GNOME \
             Keyring or KWallet needs to be running"
        ))
    }

    async fn backend() -> Result<&'static Backend, CredentialError> {
        BACKEND
            .get_or_try_init(|| async {
                match oo7::Keyring::new().await {
                    Ok(keyring) => Ok(Backend::Keyring(keyring)),
                    Err(portal) if in_flatpak() => {
                        let host =
                            async { oo7::dbus::Service::new().await?.default_collection().await };
                        host.await.map(Backend::Host).map_err(|_| {
                            CredentialError(format!(
                                "the desktop's Secret portal didn't answer ({portal}). It comes \
                                 with GNOME Keyring, and with KWallet on KDE Plasma. To use the \
                                 whole keyring instead, run: flatpak override --user \
                                 --talk-name=org.freedesktop.secrets io.github.Virel0.Aoide.Linux"
                            ))
                        })
                    }
                    Err(error) => Err(unavailable(error)),
                }
            })
            .await
    }

    /// What an item is found by.
    fn search(key: &str) -> HashMap<&str, &str> {
        HashMap::from([("service", SERVICE), ("username", key)])
    }

    /// What an item is written with: keyring-rs 3.6's set.
    fn attributes(key: &str) -> HashMap<&str, &str> {
        let mut all = search(key);
        all.insert("target", "default");
        all.insert("application", "rust-keyring");
        all
    }

    fn label(key: &str) -> String {
        format!("{key}@{SERVICE}:default (Aoide)")
    }

    fn text(secret: &oo7::Secret) -> Result<String, CredentialError> {
        String::from_utf8(secret.as_bytes().to_vec())
            .map_err(|_| CredentialError("the keyring's entry isn't text".into()))
    }

    pub async fn get(key: &str) -> Result<Option<String>, CredentialError> {
        match backend().await? {
            Backend::Keyring(keyring) => {
                keyring.unlock().await.map_err(unavailable)?;
                let items = keyring
                    .search_items(&search(key))
                    .await
                    .map_err(unavailable)?;
                match items.first() {
                    Some(item) => Ok(Some(text(&item.secret().await.map_err(unavailable)?)?)),
                    None => Ok(None),
                }
            }
            Backend::Host(collection) => {
                collection.unlock(None).await.map_err(unavailable)?;
                let items = collection
                    .search_items(&search(key))
                    .await
                    .map_err(unavailable)?;
                match items.first() {
                    Some(item) => Ok(Some(text(&item.secret().await.map_err(unavailable)?)?)),
                    None => Ok(None),
                }
            }
        }
    }

    pub async fn set(key: &str, secret: &str) -> Result<(), CredentialError> {
        let secret = oo7::Secret::text(secret);
        match backend().await? {
            Backend::Keyring(keyring) => {
                keyring.unlock().await.map_err(unavailable)?;
                keyring
                    .create_item(&label(key), &attributes(key), secret, true)
                    .await
                    .map_err(unavailable)
            }
            Backend::Host(collection) => {
                collection.unlock(None).await.map_err(unavailable)?;
                collection
                    .create_item(&label(key), &attributes(key), secret, true, None)
                    .await
                    .map(|_| ())
                    .map_err(unavailable)
            }
        }
    }

    pub async fn delete(key: &str) -> Result<(), CredentialError> {
        match backend().await? {
            Backend::Keyring(keyring) => {
                keyring.unlock().await.map_err(unavailable)?;
                keyring.delete(&search(key)).await.map_err(unavailable)
            }
            Backend::Host(collection) => {
                collection.unlock(None).await.map_err(unavailable)?;
                let items = collection
                    .search_items(&search(key))
                    .await
                    .map_err(unavailable)?;
                for item in items {
                    item.delete(None).await.map_err(unavailable)?;
                }
                Ok(())
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::super::{CredentialStore, KeyringStore};

        /// Against a real Secret Service: CI runs it with gnome-keyring on a
        /// private bus (`cargo test -p aoide-app -- --ignored keyring`).
        #[test]
        #[ignore = "needs a Secret Service"]
        fn keyring_round_trips_and_finds_what_keyring_rs_wrote() {
            let store = KeyringStore;
            let key = "ci-tester@https://music.example.net";
            store.delete(key).unwrap();
            assert_eq!(store.get(key).unwrap(), None);
            store.set(key, "first").unwrap();
            store.set(key, "second").unwrap();
            assert_eq!(store.get(key).unwrap().as_deref(), Some("second"));
            // An entry exactly as keyring-rs 3.6 wrote one is read and replaced.
            super::run(async {
                let keyring = oo7::Keyring::new().await.unwrap();
                keyring
                    .create_item("old", &super::attributes("legacy"), "old-token", true)
                    .await
                    .unwrap();
                Ok(())
            })
            .unwrap();
            assert_eq!(store.get("legacy").unwrap().as_deref(), Some("old-token"));
            store.set("legacy", "new-token").unwrap();
            assert_eq!(store.get("legacy").unwrap().as_deref(), Some("new-token"));
            store.delete("legacy").unwrap();
            store.delete(key).unwrap();
            assert_eq!(store.get(key).unwrap(), None);
        }
    }
}

/// Secrets in memory, for tests. `failing` makes every write fail, to stand in
/// for a machine with no Secret Service.
#[derive(Debug, Default)]
pub struct MemoryStore {
    entries: Mutex<HashMap<String, String>>,
    pub failing: bool,
}

impl MemoryStore {
    #[cfg(test)]
    pub fn failing() -> Self {
        Self {
            failing: true,
            ..Self::default()
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl CredentialStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<String>, CredentialError> {
        Ok(self.lock().get(key).cloned())
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), CredentialError> {
        if self.failing {
            return Err(CredentialError("the system keyring isn't available".into()));
        }
        self.lock().insert(key.to_owned(), secret.to_owned());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), CredentialError> {
        self.lock().remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_round_trips_and_is_deleted() {
        let store = MemoryStore::default();
        assert_eq!(store.get("u@s").unwrap(), None);
        store.set("u@s", "tok").unwrap();
        assert_eq!(store.get("u@s").unwrap().as_deref(), Some("tok"));
        store.delete("u@s").unwrap();
        assert_eq!(store.get("u@s").unwrap(), None);
    }

    #[test]
    fn deleting_what_is_not_there_is_fine() {
        assert_eq!(MemoryStore::default().delete("nobody"), Ok(()));
    }

    #[test]
    fn entries_are_kept_per_key() {
        let store = MemoryStore::default();
        store.set("a@s", "one").unwrap();
        store.set("b@s", "two").unwrap();
        store.delete("a@s").unwrap();
        assert_eq!(store.get("b@s").unwrap().as_deref(), Some("two"));
    }

    #[test]
    fn a_failing_store_reports_rather_than_pretending() {
        let store = MemoryStore::failing();
        assert!(store.set("u@s", "tok").is_err());
        assert_eq!(store.get("u@s").unwrap(), None);
    }

    #[test]
    fn the_trait_is_usable_as_an_object() {
        let store: Box<dyn CredentialStore> = Box::new(MemoryStore::default());
        store.set("k", "v").unwrap();
        assert_eq!(store.get("k").unwrap().as_deref(), Some("v"));
    }
}
