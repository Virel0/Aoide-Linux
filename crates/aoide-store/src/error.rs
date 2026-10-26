use crate::covers::CoverError;
use crate::playlists::PlaylistError;
use crate::smart::SmartPlaylistError;

/// Everything a store call can fail with.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The database refused a statement, or a row did not read back.
    #[error("database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A payload or stored document is not the JSON it should be.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    /// A playlist, folder or entry operation that does not apply.
    #[error(transparent)]
    Playlist(#[from] PlaylistError),
    /// A smart rule set that is malformed or cannot be evaluated.
    #[error(transparent)]
    Smart(#[from] SmartPlaylistError),
    /// A cover picture that cannot be stored.
    #[error(transparent)]
    Cover(#[from] CoverError),
    /// A file beside the database: a cover blob, the recent-contexts list.
    #[error("file: {0}")]
    Io(#[from] std::io::Error),
}

/// A store result: [`StoreError`] unless a call says otherwise.
pub type Result<T, E = StoreError> = std::result::Result<T, E>;
