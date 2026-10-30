//! Playlists, folders, the entries in them, and keeping entries linked across rescans.

mod artwork;
mod folders;
mod items;
mod playlist;
mod relink;

pub use relink::{RelinkReport, TrackLink, UNRESOLVED_PREFIX};

use rusqlite::{Connection, OptionalExtension};

use crate::error::Result;
use crate::models::{Folder, Playlist};
use crate::store::Store;

/// Why a playlist, folder or entry operation did not apply.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlaylistError {
    /// No live playlist has this id.
    #[error("no playlist {0}")]
    NoSuchPlaylist(String),
    /// No live folder has this id.
    #[error("no folder {0}")]
    NoSuchFolder(String),
    /// No live entry has this id.
    #[error("no playlist entry {0}")]
    NoSuchItem(String),
    /// An entry was named alongside a playlist it does not belong to.
    #[error("entry {item} is not in playlist {playlist}")]
    ItemNotInPlaylist {
        /// The entry's id.
        item: String,
        /// The playlist it was looked for in.
        playlist: String,
    },
    /// A folder was asked to become its own ancestor.
    #[error("folder {folder} cannot move under {under}, which is inside it")]
    FolderCycle {
        /// The folder being moved.
        folder: String,
        /// The folder it was to move under.
        under: String,
    },
    /// The neighbours bracket no gap: the wrong way round, or one is the row moving.
    #[error("those neighbours leave no gap to move into")]
    BadNeighbours,
}

impl Store {
    pub(super) fn require_playlist(&self, id: &str) -> Result<Playlist> {
        self.playlist(id)?
            .ok_or_else(|| PlaylistError::NoSuchPlaylist(id.to_owned()).into())
    }

    pub(super) fn require_folder(&self, id: &str) -> Result<Folder> {
        self.folder(id)?
            .ok_or_else(|| PlaylistError::NoSuchFolder(id.to_owned()).into())
    }
}

pub(super) fn last_key(
    conn: &Connection,
    sql: &str,
    parameter: Option<&str>,
) -> rusqlite::Result<Option<String>> {
    match parameter {
        Some(parameter) => conn
            .query_row(sql, [parameter], |row| row.get(0))
            .optional(),
        None => conn.query_row(sql, [], |row| row.get(0)).optional(),
    }
}

/// The index only asserts on this in debug builds; checking turns a corrupted order in
/// a release build into an error the UI can ignore.
pub(super) fn check_gap(lower: Option<&str>, upper: Option<&str>) -> Result<()> {
    if let (Some(lower), Some(upper)) = (lower, upper)
        && lower >= upper
    {
        return Err(PlaylistError::BadNeighbours.into());
    }
    Ok(())
}
