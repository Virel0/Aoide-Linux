//! Motion covers: an album's looping video from its folder, played muted
//! over its still cover.

use crate::client::MeleteClient;
use crate::{Error, Result};

/// The largest motion cover read: a loop of a few seconds is far smaller.
pub const MOTION_COVER_MAX_BYTES: usize = 32 << 20;

impl MeleteClient {
    /// An album's looping video, from the path its album gave
    /// ([`Item::motion_cover`](crate::Item::motion_cover)): MP4, or WebM from
    /// an album's own folder. `None` for a path that isn't a plain path on this
    /// server, a video larger than [`MOTION_COVER_MAX_BYTES`], or an album
    /// with none.
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors), but for a 404, which is
    /// `Ok(None)`.
    pub async fn motion_cover(&self, served_path: &str) -> Result<Option<Vec<u8>>> {
        let Some(url) = self.address().join_path(served_path) else {
            return Ok(None);
        };
        match self
            .download(url, MOTION_COVER_MAX_BYTES, served_path)
            .await
        {
            Err(Error::NotFound { .. }) => Ok(None),
            read => read,
        }
    }
}
