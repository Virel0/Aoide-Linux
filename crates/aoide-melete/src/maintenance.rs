//! Library maintenance: rescans. Administrators only; anyone else gets HTTP 403
//! as [`Error::Http`](crate::Error::Http).

use reqwest::Method;

use crate::Result;
use crate::client::MeleteClient;

impl MeleteClient {
    /// Starts a whole scan of every library: Melete's `library` task, asked to run
    /// once more. Returns once it is asked for (202); the web UI shows its
    /// progress.
    ///
    /// # Errors
    ///
    /// [`Error::Http`](crate::Error::Http) with status 403 for a user who isn't an
    /// administrator, and 409 with Melete's reason when a scan is running
    /// already; otherwise the [request errors](MeleteClient#errors).
    pub async fn refresh_library(&self) -> Result<()> {
        self.send_empty(
            Method::POST,
            &["api", "v1", "admin", "system", "tasks", "library", "run"],
            &[],
        )
        .await
    }
}
