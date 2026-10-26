//! Playlists, folders, the entries in them, and keeping entries linked across rescans.

mod artwork;
mod folders;
mod import;
mod items;
mod playlist;
mod relink;

pub use relink::{RelinkReport, TrackLink, UNRESOLVED_PREFIX};

use rusqlite::{Connection, OptionalExtension};

use crate::error::Result;
use crate::models::{Folder, Playlist};
use crate::store::Store;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlaylistError {
    #[error("no playlist {0}")]
    NoSuchPlaylist(String),
    #[error("no folder {0}")]
    NoSuchFolder(String),
    #[error("no playlist entry {0}")]
    NoSuchItem(String),
    #[error("entry {item} is not in playlist {playlist}")]
    ItemNotInPlaylist { item: String, playlist: String },
    /// A folder was asked to become its own ancestor.
    #[error("folder {folder} cannot move under {under}, which is inside it")]
    FolderCycle { folder: String, under: String },
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
