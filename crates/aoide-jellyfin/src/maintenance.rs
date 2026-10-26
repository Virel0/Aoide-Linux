//! Library maintenance: rescans and metadata refreshes. Both need an
//! administrator account; anyone else gets HTTP 403 as [`Error::Http`](crate::Error::Http).

use reqwest::Method;
use serde::Deserialize;

use crate::client::JellyfinClient;
use crate::{Error, Result};

/// The key of the server's "Scan Media Library" scheduled task. Keys are fixed;
/// names are translated.
const LIBRARY_SCAN_KEY: &str = "RefreshLibrary";

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct TaskInfo {
    id: String,
    #[serde(default)]
    key: Option<String>,
}

/// How thoroughly to refresh one item, as Jellyfin's own refresh dialog offers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ItemRefresh {
    /// Rescan the files for new and changed ones, and fill in what's new.
    #[default]
    ScanFiles,
    /// Look up metadata the item is missing, keeping what it has.
    FillMissing,
    /// Replace all metadata, and the images too when `images` is set.
    ReplaceAll {
        /// Replace the item's images as well.
        images: bool,
    },
}

impl ItemRefresh {
    /// The `metadataRefreshMode` / `imageRefreshMode` value, and the replace flags.
    fn query(self) -> Vec<(&'static str, String)> {
        let (mode, replace_metadata, replace_images) = match self {
            ItemRefresh::ScanFiles => ("Default", false, false),
            ItemRefresh::FillMissing => ("FullRefresh", false, false),
            ItemRefresh::ReplaceAll { images } => ("FullRefresh", true, images),
        };
        vec![
            ("metadataRefreshMode", mode.to_owned()),
            ("imageRefreshMode", mode.to_owned()),
            ("replaceAllMetadata", replace_metadata.to_string()),
            ("replaceAllImages", replace_images.to_string()),
        ]
    }
}

impl JellyfinClient {
    /// Starts a scan of every library: the server's "Scan Media Library" scheduled
    /// task, found by its key in `/ScheduledTasks` and started through
    /// `/ScheduledTasks/Running/{id}`. Returns once the task has started; the
    /// server's dashboard shows its progress.
    ///
    /// Not `POST /Library/Refresh`: that runs the whole scan inside the request and
    /// only answers when it ends, which on a big library outlasts the client's
    /// timeout. A server without the task answers [`Error::NotFound`].
    ///
    /// # Errors
    ///
    /// [`Error::NotSignedIn`] without credentials, before any request;
    /// [`Error::Http`] with status 403 for a user who isn't an administrator;
    /// [`Error::NotFound`] when the server has no scan task; otherwise the [request
    /// errors](JellyfinClient#errors).
    pub async fn refresh_library(&self) -> Result<()> {
        self.require_credentials()?;
        let tasks: Vec<TaskInfo> = self.get(&["ScheduledTasks"], &[]).await?;
        let task = tasks
            .into_iter()
            .find(|task| task.key.as_deref() == Some(LIBRARY_SCAN_KEY))
            .ok_or_else(|| Error::NotFound {
                path: format!("ScheduledTasks/{LIBRARY_SCAN_KEY}"),
            })?;
        self.send_empty(Method::POST, &["ScheduledTasks", "Running", &task.id], &[])
            .await
    }

    /// Queues a refresh of one item (a track, an album, an artist, a folder).
    /// Returns once it's queued.
    ///
    /// The modes are always sent: left out, the server's default is `None`, which
    /// refreshes nothing.
    ///
    /// # Errors
    ///
    /// [`Error::NotSignedIn`] without credentials, before any request;
    /// [`Error::Http`] with status 403 for a user who isn't an administrator;
    /// [`Error::NotFound`] for an id the server doesn't have; otherwise the
    /// [request errors](JellyfinClient#errors).
    pub async fn refresh_item(&self, item_id: &str, refresh: ItemRefresh) -> Result<()> {
        self.require_credentials()?;
        self.send_empty(
            Method::POST,
            &["Items", item_id, "Refresh"],
            &refresh.query(),
        )
        .await
    }
}
