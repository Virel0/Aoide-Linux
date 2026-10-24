//! Covers this device holds, named by the SHA-256 of their own bytes.

use rusqlite::Row;
use rusqlite::types::Value as SqlValue;
use sha2::{Digest, Sha256};

use crate::Millis;
use crate::error::Result;
use crate::record::{Record, int, opt_int, text};
use crate::store::Store;

/// A cover's bookkeeping; the bytes live on disk beside the database. Content
/// addressing makes the same picture chosen twice one blob, and re-uploading a no-op.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageBlob {
    pub hash: String,
    pub mime: String,
    pub bytes: i64,
    pub created_at: Millis,
    /// `None` until the server has these bytes. An op naming this hash must not be
    /// pushed before then.
    pub uploaded_at: Option<Millis>,
}

impl Record for ImageBlob {
    const TABLE: &'static str = "imageBlob";
    const KEY: &'static str = "hash";
    const COLUMNS: &'static [&'static str] = &["hash", "mime", "bytes", "createdAt", "uploadedAt"];

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            hash: row.get("hash")?,
            mime: row.get("mime")?,
            bytes: row.get("bytes")?,
            created_at: row.get("createdAt")?,
            uploaded_at: row.get("uploadedAt")?,
        })
    }

    fn values(&self) -> Vec<SqlValue> {
        vec![
            text(&self.hash),
            text(&self.mime),
            int(self.bytes),
            int(self.created_at),
            opt_int(self.uploaded_at),
        ]
    }

    fn key(&self) -> &str {
        &self.hash
    }
}

/// How long a blob nothing references is kept before it is even considered for
/// removal, matching the sidecar's own sweep: "nothing references this" is only ever
/// true *here*, and a phone in a drawer may still name it.
pub const IMAGE_GRACE_DAYS: i64 = 30;

impl Store {
    /// The SHA-256 the server addresses these bytes by: lower-case hex, because the
    /// server rejects a hash that does not match the bytes and case is part of that.
    pub fn image_hash(data: &[u8]) -> String {
        Sha256::digest(data)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Records that this device holds these bytes. Local only — the blob is not synced
    /// state, the playlist row naming it is. Never overwrites, so re-registering does
    /// not un-mark an upload.
    pub fn register_image(
        &self,
        hash: &str,
        mime: &str,
        bytes: i64,
        now: Millis,
    ) -> Result<ImageBlob> {
        let blob = ImageBlob {
            hash: hash.to_owned(),
            mime: mime.to_owned(),
            bytes,
            created_at: now,
            uploaded_at: None,
        };
        self.write_local(|tx| {
            if ImageBlob::fetch(tx, hash)?.is_none() {
                blob.insert(tx)?;
            }
            Ok(())
        })?;
        Ok(self.image(hash)?.unwrap_or(blob))
    }

    pub fn image(&self, hash: &str) -> Result<Option<ImageBlob>> {
        self.read(|conn| ImageBlob::fetch(conn, hash))
    }

    pub fn mark_image_uploaded(&self, hash: &str, now: Millis) -> Result<()> {
        self.write_local(|tx| {
            tx.execute(
                "UPDATE imageBlob SET uploadedAt = ? WHERE hash = ?",
                rusqlite::params![now, hash],
            )?;
            Ok(())
        })
    }

    /// Hashes named by ops still waiting to be pushed whose bytes the server has not
    /// been given. Read out of the payloads, so an op and its blob cannot drift apart.
    pub fn images_needed_before_push(&self) -> Result<Vec<ImageBlob>> {
        self.read(|conn| {
            ImageBlob::fetch_where(
                conn,
                "hash IN (SELECT DISTINCT b.hash FROM op o JOIN imageBlob b ON o.payload LIKE '%' || b.hash || '%' \
                          WHERE o.synced = 0 AND b.uploadedAt IS NULL) ORDER BY createdAt, hash",
                [],
            )
        })
    }

    /// Blobs no live playlist names, older than the grace period, oldest first.
    /// Reports rather than reclaims; a shorter grace is raised, never honoured.
    pub fn orphaned_images(&self, older_than_days: i64, now: Millis) -> Result<Vec<ImageBlob>> {
        let grace = older_than_days.max(IMAGE_GRACE_DAYS);
        let cutoff = now - grace * 24 * 60 * 60 * 1000;
        self.read(|conn| {
            ImageBlob::fetch_where(
                conn,
                "createdAt < ? AND hash NOT IN \
                 (SELECT imageHash FROM playlist WHERE imageHash IS NOT NULL AND deleted = 0) \
                 ORDER BY createdAt",
                [cutoff],
            )
        })
    }

    /// Removes blobs this device is done with; a separate, explicit step.
    pub fn forget_images(&self, hashes: &[String]) -> Result<usize> {
        self.write_local(|tx| {
            let mut removed = 0;
            for hash in hashes {
                if ImageBlob::delete(tx, hash)? {
                    removed += 1;
                }
            }
            Ok(removed)
        })
    }
}
