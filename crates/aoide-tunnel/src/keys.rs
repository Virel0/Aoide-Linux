//! WireGuard keys as Melete's tunnel API spells them: 32 bytes in base64.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use gotatun::x25519::{PublicKey, StaticSecret};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// A key that isn't 32 bytes in base64.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("a WireGuard key is 32 bytes in base64")]
pub struct BadKey;

/// This device's private key. Only its public half is ever sent; the key itself
/// stays in the keyring, and is wiped from memory when dropped.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct PrivateKey([u8; 32]);

impl PrivateKey {
    /// A new key from the OS's randomness.
    ///
    /// # Errors
    ///
    /// When the OS has no randomness to give.
    pub fn generate() -> Result<Self, getrandom::Error> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes)?;
        // Clamped as X25519 does, so the stored key is the one in use.
        Ok(Self(StaticSecret::from(bytes).to_bytes()))
    }

    /// A key kept earlier, from its base64.
    ///
    /// # Errors
    ///
    /// [`BadKey`] for anything but 32 bytes in base64.
    pub fn from_base64(text: &str) -> Result<Self, BadKey> {
        decode(text).map(Self)
    }

    /// The key as base64, for the keyring.
    pub fn to_base64(&self) -> String {
        STANDARD.encode(self.0)
    }

    /// Its public half, as base64, for Melete.
    pub fn public_base64(&self) -> String {
        STANDARD.encode(PublicKey::from(&self.secret()).as_bytes())
    }

    pub(crate) fn secret(&self) -> StaticSecret {
        StaticSecret::from(self.0)
    }
}

impl std::fmt::Debug for PrivateKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PrivateKey(…)")
    }
}

/// A public or preshared key from Melete.
///
/// # Errors
///
/// [`BadKey`] for anything but 32 bytes in base64.
pub fn decode(text: &str) -> Result<[u8; 32], BadKey> {
    let bytes = STANDARD.decode(text.trim()).map_err(|_| BadKey)?;
    bytes.try_into().map_err(|_| BadKey)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_survives_the_keyring() {
        let key = PrivateKey::generate().unwrap();
        let again = PrivateKey::from_base64(&key.to_base64()).unwrap();
        assert_eq!(key.public_base64(), again.public_base64());
        assert_eq!(decode(&key.public_base64()).map(|k| k.len()), Ok(32));
    }

    #[test]
    fn anything_but_32_bytes_is_refused() {
        assert_eq!(decode("c2hvcnQ=").unwrap_err(), BadKey);
        assert!(PrivateKey::from_base64("not base64!").is_err());
    }

    #[test]
    fn the_key_is_never_printed() {
        let key = PrivateKey::generate().unwrap();
        assert_eq!(format!("{key:?}"), "PrivateKey(…)");
    }
}
