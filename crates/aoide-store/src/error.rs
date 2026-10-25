use crate::covers::CoverError;
use crate::playlists::PlaylistError;
use crate::smart::SmartPlaylistError;

/// Everything a store call can fail with.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Playlist(#[from] PlaylistError),
    #[error(transparent)]
    Smart(#[from] SmartPlaylistError),
    #[error(transparent)]
    Cover(#[from] CoverError),
    /// A file beside the database: a cover blob, the recent-contexts list.
    #[error("file: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = StoreError> = std::result::Result<T, E>;
