use std::time::Duration;

use serde_json::json;

use crate::client::JellyfinClient;
use crate::{Result, ticks};

/// Reported as `Transcode` because streams go through `/Audio/{id}/universal` and
/// the server decides; it knows better than the client whether it transcoded.
const PLAY_METHOD: &str = "Transcode";

impl JellyfinClient {
    /// Reporting playback is what makes play counts and "recently played" work, and
    /// what stops the server showing a phantom session after the app quits.
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

    pub async fn report_playback_stopped(&self, item_id: &str, position: Duration) -> Result<()> {
        let body = json!({
            "ItemId": item_id,
            "PositionTicks": ticks::from_duration(position),
        });
        self.post_json(&["Sessions", "Playing", "Stopped"], &body)
            .await
    }
}
