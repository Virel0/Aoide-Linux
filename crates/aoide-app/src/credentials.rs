//! Where the access token is kept: the system keyring (Secret Service on
//! Linux, the Keychain on macOS), behind a trait so tests use memory.
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

impl KeyringStore {
    fn entry(key: &str) -> Result<keyring::Entry, CredentialError> {
        keyring::Entry::new(SERVICE, key).map_err(describe)
    }
}

fn describe(error: keyring::Error) -> CredentialError {
    CredentialError(match error {
        keyring::Error::NoStorageAccess(_) | keyring::Error::PlatformFailure(_) => format!(
            "the system keyring isn't available ({error}). On Linux, a Secret Service \
             such as GNOME Keyring or KWallet needs to be running"
        ),
        other => format!("the system keyring refused: {other}"),
    })
}

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

/// Secrets in memory, for tests. `failing` makes every write fail, to stand in
/// for a machine with no Secret Service.
#[derive(Debug, Default)]
pub struct MemoryStore {
    entries: Mutex<HashMap<String, String>>,
    pub failing: bool,
}

impl MemoryStore {
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
