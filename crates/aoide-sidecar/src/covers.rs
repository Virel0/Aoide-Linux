//! Cover bytes behind a hash: this device's blob directory first, the sidecar second.
//!
//! Call [`cover_bytes`] when a cover is **about to be drawn** — a visible row, an
//! open detail page — and never from sync: a device joining an account would
//! otherwise download every cover in the library before it finished syncing, most of
//! which it will never show. A fetched copy is kept forever, because the bytes behind
//! a SHA-256 cannot change.
//!
//! Which hash to ask for, and what to draw when there is none, is
//! [`aoide_store::Store::playlist_covers`].

use aoide_store::{BlobDirectory, Store, StoreError, is_image_hash, sniff_image_mime};

use crate::client::SidecarClient;
use crate::engine::ImageByteSource;
use crate::error::SidecarError;

/// The engine uploads covers from the same directory they are drawn from.
impl ImageByteSource for BlobDirectory {
    fn bytes(&self, sha256: &str) -> Option<Vec<u8>> {
        self.read(sha256)
    }
}

/// Why [`cover_bytes`] couldn't produce a cover's bytes.
#[derive(Debug, thiserror::Error)]
pub enum CoverFetchError {
    /// Fetching from the sidecar failed; a 404 is not this but `Ok(None)`.
    #[error("transport: {0}")]
    Transport(#[from] SidecarError),
    /// Recording the fetched blob in the store failed.
    #[error("store: {0}")]
    Store(#[from] StoreError),
    /// Writing the fetched bytes to the blob directory failed.
    #[error("file: {0}")]
    Io(#[from] std::io::Error),
    /// The server answered with bytes that are not the ones named. Nothing is kept:
    /// a blob stored under the wrong name would be wrong on this device for ever.
    #[error("the server's bytes for {0} do not match that hash")]
    Mismatch(String),
}

/// The bytes behind `hash`, from `blobs` if this device holds them, otherwise fetched
/// from the sidecar and kept in `blobs` (and recorded as already on the server, so
/// nothing ever uploads them back). `Ok(None)` when nobody has them yet — the device
/// that chose the cover has not synced — or there is no sidecar, or `hash` is not a
/// hash at all; draw the next source instead. `mime` is what the playlist row says
/// the bytes are, used only when the bytes do not say for themselves.
///
/// # Errors
///
/// [`CoverFetchError::Transport`] when the sidecar fails other than with a 404;
/// [`CoverFetchError::Mismatch`] when its bytes don't hash to `hash`;
/// [`CoverFetchError::Io`] when the bytes can't be written to `blobs`;
/// [`CoverFetchError::Store`] when the store can't record them. Nothing is kept
/// after a mismatch.
pub async fn cover_bytes(
    store: &Store,
    blobs: &BlobDirectory,
    sidecar: Option<&SidecarClient>,
    hash: &str,
    mime: Option<&str>,
) -> Result<Option<Vec<u8>>, CoverFetchError> {
    if !is_image_hash(hash) {
        return Ok(None);
    }
    if let Some(held) = blobs.read(hash) {
        return Ok(Some(held));
    }
    let Some(sidecar) = sidecar else {
        return Ok(None);
    };
    let Some(bytes) = sidecar.get_image(hash).await? else {
        return Ok(None);
    };
    if Store::image_hash(&bytes) != hash {
        return Err(CoverFetchError::Mismatch(hash.to_owned()));
    }
    blobs.write(&bytes)?;
    let mime = sniff_image_mime(&bytes).or(mime).unwrap_or("image/jpeg");
    let now = aoide_store::now_millis();
    store.register_image(hash, mime, bytes.len() as i64, now)?;
    store.mark_image_uploaded(hash, now)?;
    Ok(Some(bytes))
}
