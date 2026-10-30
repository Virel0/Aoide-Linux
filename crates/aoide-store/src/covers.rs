//! An Aoide playlist's cover: which picture to draw, and choosing one from a file.
//!
//! **Which picture**, in the order both other clients use (`PlaylistArtwork.swift`,
//! `use-playlist-cover.ts`):
//!
//! 1. `imageHash` — a picture somebody chose. Drawn from this device's
//!    [`BlobDirectory`] when it holds the bytes, otherwise fetched from Melete's
//!    `GET /aoide/images/{sha}` **at the moment it is about to be drawn** (see
//!    `aoide_sidecar::cover_bytes`), never during sync: a device joining an account
//!    would otherwise download every cover in the library first.
//! 2. `artworkItemId` — an album chosen as the cover.
//! 3. The first track's album art, as a stand-in (nothing is written).
//!
//! **Bytes never enter the op log.** The row carries the hash and the MIME type; the
//! bytes travel through `/aoide/images`, uploaded by the sync engine before the op
//! naming them is pushed.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use rusqlite::params_from_iter;

use crate::Millis;
use crate::error::Result;
use crate::history::MAX_PARAMETERS;
use crate::models::Playlist;
use crate::playlists::{PlaylistError, UNRESOLVED_PREFIX};
use crate::record::placeholders;
use crate::store::Store;

/// The sidecar refuses anything larger (its default; configurable on the server).
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// What a playlist's cover is drawn from. See the module documentation for the order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaylistCover {
    /// Bytes named by their SHA-256: from the [`BlobDirectory`] if held, otherwise
    /// from the sidecar when about to be drawn.
    Image {
        /// The bytes' SHA-256, lower-case hex.
        hash: String,
        /// Their media type, when the row records it.
        mime: Option<String>,
    },
    /// An album chosen as the cover, by its id.
    Album(String),
    /// The first track's album (or the track itself when its album is unknown) — a
    /// stand-in until somebody chooses a cover.
    FirstTrack(String),
    /// An empty playlist with nothing to draw: the lettered tile.
    None,
}

/// Why a picture cannot be a cover.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoverError {
    /// Not JPEG, PNG or WebP, judged from the bytes.
    #[error("that file is not a JPEG, PNG or WebP picture")]
    NotAnImage,
    /// Larger than [`MAX_IMAGE_BYTES`].
    #[error("that picture is {bytes} bytes; covers can be at most {max}", max = MAX_IMAGE_BYTES)]
    TooLarge {
        /// The picture's size in bytes.
        bytes: u64,
    },
    /// A hash that is not 64 lower-case hex digits. Hashes arrive in synced payloads,
    /// so one is never used as a file name before this check.
    #[error("'{0}' is not a picture's SHA-256")]
    BadHash(String),
}

/// The picture formats the sidecar accepts, told from the bytes rather than the file
/// name.
pub fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// Whether `hash` is a lower-case hex SHA-256 — the only shape the sidecar addresses.
pub fn is_image_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

// MARK: - Bytes on disk

/// Cover bytes on disk, one file per hash. Keep it somewhere durable (the data
/// directory, not a cache): a cover chosen here is the only copy until it is uploaded.
///
/// Content addressing makes it simple: a file either is the bytes behind its name or
/// does not exist, nothing is updated in place, and two playlists sharing a cover
/// share a file.
#[derive(Debug, Clone)]
pub struct BlobDirectory {
    root: PathBuf,
}

impl BlobDirectory {
    /// Opens (creating if needed) the directory at `root`.
    ///
    /// # Errors
    ///
    /// Fails when the directory cannot be created.
    pub fn open(root: impl Into<PathBuf>) -> std::io::Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    /// Where the files live.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path(&self, hash: &str) -> Option<PathBuf> {
        is_image_hash(hash).then(|| self.root.join(hash))
    }

    /// Whether a file for `hash` is here; false for anything that is not a hash.
    pub fn contains(&self, hash: &str) -> bool {
        self.path(hash).is_some_and(|path| path.is_file())
    }

    /// The bytes behind `hash`, if this device holds them.
    pub fn read(&self, hash: &str) -> Option<Vec<u8>> {
        std::fs::read(self.path(hash)?).ok()
    }

    /// Keeps `bytes` under their own hash, atomically, and returns the hash.
    ///
    /// # Errors
    ///
    /// Fails when the file cannot be written or renamed into place; no partial file is
    /// left under the hash then.
    pub fn write(&self, bytes: &[u8]) -> std::io::Result<String> {
        let hash = Store::image_hash(bytes);
        let path = self.root.join(&hash);
        if !path.is_file() {
            let temporary = self.root.join(format!("{hash}.tmp"));
            std::fs::write(&temporary, bytes)?;
            std::fs::rename(&temporary, &path)?;
        }
        Ok(hash)
    }

    /// Removes the file for `hash`; reports whether there was one.
    ///
    /// # Errors
    ///
    /// Fails when the file exists but cannot be removed.
    pub fn remove(&self, hash: &str) -> std::io::Result<bool> {
        let Some(path) = self.path(hash) else {
            return Ok(false);
        };
        match std::fs::remove_file(path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }
}

// MARK: - Choosing and resolving

impl Store {
    /// Makes `bytes` the playlist's cover: checked (JPEG, PNG or WebP, at most
    /// [`MAX_IMAGE_BYTES`]), kept in `blobs`, registered as waiting for upload, and
    /// named on the playlist row — whose op the sync engine pushes only after the
    /// bytes are on the server. A chosen picture outranks a chosen item, so
    /// `artworkItemId` is cleared rather than left to shadow it.
    ///
    /// # Errors
    ///
    /// [`PlaylistError::NoSuchPlaylist`] when there is no live playlist,
    /// [`CoverError::TooLarge`] or [`CoverError::NotAnImage`] for the bytes, a file error
    /// writing them, or a database error.
    pub fn set_cover_image(
        &self,
        playlist_id: &str,
        bytes: &[u8],
        blobs: &BlobDirectory,
        now: Millis,
    ) -> Result<Playlist> {
        let Some(mut list) = self.playlist(playlist_id)? else {
            return Err(PlaylistError::NoSuchPlaylist(playlist_id.to_owned()).into());
        };
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(CoverError::TooLarge {
                bytes: bytes.len() as u64,
            }
            .into());
        }
        let mime = sniff_image_mime(bytes).ok_or(CoverError::NotAnImage)?;
        let hash = blobs.write(bytes)?;
        self.register_image(&hash, mime, bytes.len() as i64, now)?;
        list.image_hash = Some(hash);
        list.image_mime = Some(mime.to_owned());
        list.artwork_item_id = None;
        self.record(list, false, now)
    }

    /// [`set_cover_image`](Self::set_cover_image) from a file the listener picked. A
    /// file over the limit is refused before it is read.
    ///
    /// # Errors
    ///
    /// A file error opening or reading `file`, [`CoverError::TooLarge`] for a file over the
    /// limit, and then as [`Self::set_cover_image`].
    pub fn set_cover_from_file(
        &self,
        playlist_id: &str,
        file: impl AsRef<Path>,
        blobs: &BlobDirectory,
        now: Millis,
    ) -> Result<Playlist> {
        let handle = std::fs::File::open(file)?;
        let length = handle.metadata()?.len();
        if length > MAX_IMAGE_BYTES as u64 {
            return Err(CoverError::TooLarge { bytes: length }.into());
        }
        let mut bytes = Vec::with_capacity(length as usize);
        handle
            .take(MAX_IMAGE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        self.set_cover_image(playlist_id, &bytes, blobs, now)
    }

    /// Takes the chosen picture off a playlist; it falls back to the next source.
    /// The blob stays on disk: another playlist may share it, and the grace period
    /// decides when it can go ([`Store::orphaned_images`]).
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn clear_cover_image(&self, playlist_id: &str, now: Millis) -> Result<Option<Playlist>> {
        let Some(mut list) = self.playlist(playlist_id)? else {
            return Ok(None);
        };
        if list.image_hash.is_none() && list.image_mime.is_none() {
            return Ok(Some(list));
        }
        list.image_hash = None;
        list.image_mime = None;
        Ok(Some(self.record(list, false, now)?))
    }

    /// What each playlist's cover is drawn from, for a whole list screen in one query.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn playlist_covers(
        &self,
        playlists: &[Playlist],
    ) -> Result<HashMap<String, PlaylistCover>> {
        let need_first: Vec<String> = playlists
            .iter()
            .filter(|list| chosen_cover(list).is_none())
            .map(|list| list.id.clone())
            .collect();
        let firsts = self.first_track_art(&need_first)?;
        Ok(playlists
            .iter()
            .map(|list| {
                let cover = chosen_cover(list).unwrap_or_else(|| {
                    firsts.get(&list.id).map_or(PlaylistCover::None, |id| {
                        PlaylistCover::FirstTrack(id.clone())
                    })
                });
                (list.id.clone(), cover)
            })
            .collect())
    }

    /// One playlist's cover; see [`playlist_covers`](Self::playlist_covers).
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn playlist_cover(&self, playlist: &Playlist) -> Result<PlaylistCover> {
        Ok(self
            .playlist_covers(std::slice::from_ref(playlist))?
            .remove(&playlist.id)
            .unwrap_or(PlaylistCover::None))
    }

    /// For each playlist, the first live entry's album id from the track cache, or
    /// the entry's own track id when the cache does not know the album.
    fn first_track_art(&self, playlist_ids: &[String]) -> Result<HashMap<String, String>> {
        if playlist_ids.is_empty() {
            return Ok(HashMap::new());
        }
        self.read(|conn| {
            let mut firsts = HashMap::new();
            for chunk in playlist_ids.chunks(MAX_PARAMETERS) {
                let mut statement = conn.prepare(&format!(
                    "SELECT i.playlistId, i.jellyfinId, t.albumId FROM playlistItem i \
                     LEFT JOIN track t ON t.jellyfinId = i.jellyfinId \
                     WHERE i.deleted = 0 AND i.playlistId IN ({}) \
                     ORDER BY i.playlistId, i.position, i.rowid",
                    placeholders(chunk.len())
                ))?;
                let rows = statement.query_map(params_from_iter(chunk), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                })?;
                for row in rows {
                    let (playlist_id, track_id, album_id) = row?;
                    firsts.entry(playlist_id).or_insert_with(|| {
                        album_id.unwrap_or_else(|| {
                            track_id
                                .strip_prefix(UNRESOLVED_PREFIX)
                                .unwrap_or(&track_id)
                                .to_owned()
                        })
                    });
                }
            }
            Ok(firsts)
        })
    }
}

/// The cover somebody chose, before any stand-in.
fn chosen_cover(list: &Playlist) -> Option<PlaylistCover> {
    if let Some(hash) = list
        .image_hash
        .as_deref()
        .filter(|hash| is_image_hash(hash))
    {
        return Some(PlaylistCover::Image {
            hash: hash.to_owned(),
            mime: list.image_mime.clone(),
        });
    }
    list.artwork_item_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .map(|id| PlaylistCover::Album(id.to_owned()))
}
