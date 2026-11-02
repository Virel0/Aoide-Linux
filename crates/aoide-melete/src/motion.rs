//! Motion covers: an album's looping video, played muted over its still
//! cover, and whether this user sees the ones Melete made from still covers.

use reqwest::Method;
use serde::Deserialize;
use serde_json::json;

use crate::client::MeleteClient;
use crate::{Error, Result};

/// The largest motion cover read: a loop of a few seconds is far smaller.
pub const MOTION_COVER_MAX_BYTES: usize = 32 << 20;

/// Whether motion covers are made and shown, as `/api/v1/music/motion-covers`
/// says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct MotionCoverChoice {
    /// Whether this user sees the motion covers Melete made from still
    /// covers; true until they turn it off. Theirs on every device.
    pub show_generated: bool,
    /// Whether the server makes motion covers at all (an administrator's
    /// setting). Those in an album's own folder are shown either way.
    pub on: bool,
}

const CHOICE: [&str; 4] = ["api", "v1", "music", "motion-covers"];

impl MeleteClient {
    /// An album's looping video, from the path its album gave
    /// ([`Item::motion_cover`](crate::Item::motion_cover)): MP4, or WebM from
    /// an album's own folder. `None` for a path that isn't a plain path on this
    /// server, a video larger than [`MOTION_COVER_MAX_BYTES`], or an album
    /// with none (for this user).
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

    /// Whether motion covers are on, and whether this user sees those Melete
    /// made.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] from a server older than motion covers; otherwise
    /// the [request errors](MeleteClient#errors).
    pub async fn motion_cover_choice(&self) -> Result<MotionCoverChoice> {
        self.get(&CHOICE, &[]).await
    }

    /// Shows or hides the motion covers Melete made from still covers, for
    /// this user on every device. Returns the choice as it is now.
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors).
    pub async fn set_show_generated_motion_covers(&self, shows: bool) -> Result<MotionCoverChoice> {
        let body = json!({ "show_generated": shows });
        self.send_decoded(Method::PUT, &CHOICE, &[], Some(&body))
            .await
    }
}
