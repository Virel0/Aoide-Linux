use std::time::Duration;

use serde_json::json;

use crate::client::JellyfinClient;
use crate::{Result, ticks};

/// Reported as `Transcode` because streams go through `/Audio/{id}/universal` and
/// the server decides; it knows better than the client whether it transcoded.
const PLAY_METHOD: &str = "Transcode";

impl JellyfinClient {
    /// Tells the server `item_id` started playing at `position`.
    ///
    /// Reporting playback is what makes play counts and "recently played" work, and
    /// what stops the server showing a phantom session after the app quits.
    ///
    /// # Errors
    ///
    /// The [request errors](JellyfinClient#errors). A signed-out client sends no
    /// token, which the server answers with
    /// [`Error::Unauthorized`](crate::Error::Unauthorized).
    pub async fn report_playback_started(&self, item_id: &str, position: Duration) -> Result<()> {
        let body = json!({
            "ItemId": item_id,
            "PositionTicks": ticks::from_duration(position),
            "IsPaused": false,
            "CanSeek": true,
            "PlayMethod": PLAY_METHOD,
        });
        self.post_json(&["Sessions", "Playing"], &body).await
    }

    /// Tells the server where playback of `item_id` is, and whether it's paused
    /// (sent as a `Pause` or a `TimeUpdate` event).
    ///
    /// # Errors
    ///
    /// The [request errors](JellyfinClient#errors). A signed-out client sends no
    /// token, which the server answers with
    /// [`Error::Unauthorized`](crate::Error::Unauthorized).
    pub async fn report_playback_progress(
        &self,
        item_id: &str,
        position: Duration,
        paused: bool,
    ) -> Result<()> {
        let body = json!({
            "ItemId": item_id,
            "PositionTicks": ticks::from_duration(position),
            "IsPaused": paused,
            "PlayMethod": PLAY_METHOD,
            "EventName": if paused { "Pause" } else { "TimeUpdate" },
        });
        self.post_json(&["Sessions", "Playing", "Progress"], &body)
            .await
    }

    /// Tells the server playback of `item_id` ended at `position`, which ends this
    /// device's now-playing entry on the server.
    ///
    /// # Errors
    ///
    /// The [request errors](JellyfinClient#errors). A signed-out client sends no
    /// token, which the server answers with
    /// [`Error::Unauthorized`](crate::Error::Unauthorized).
    pub async fn report_playback_stopped(&self, item_id: &str, position: Duration) -> Result<()> {
        let body = json!({
            "ItemId": item_id,
            "PositionTicks": ticks::from_duration(position),
        });
        self.post_json(&["Sessions", "Playing", "Stopped"], &body)
            .await
    }
}
