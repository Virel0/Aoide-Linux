//! What the server and the desktop hear about what plays: Jellyfin's
//! playback reports, and the now-playing state for MPRIS.

use std::time::Duration;

use aoide_jellyfin::JellyfinClient;
use iced::Task;

use crate::app::{App, Message};
use crate::playback::report::Report;
use crate::session;

impl App {
    /// Pause and resume are reported straight away.
    pub(crate) fn report_pause_state(&mut self) -> Task<Message> {
        let paused = !self.playback.is_playing();
        let now = self.now();
        let report = self
            .session
            .as_mut()
            .and_then(|s| s.reporter.paused(paused, now));
        self.send_reports(report.into_iter().collect())
    }

    pub(crate) fn send_reports(&self, reports: Vec<Report>) -> Task<Message> {
        let Some(signed) = &self.session else {
            return Task::none();
        };
        // Private listening: Jellyfin hears nothing.
        if self.listening.private {
            return Task::none();
        }
        Task::batch(reports.into_iter().map(|report| {
            let client = signed.connection.client.clone();
            Task::perform(send_report(client, report), |result| match result {
                Err(aoide_jellyfin::Error::Unauthorized) => Message::TokenRejected,
                _ => Message::Ignore,
            })
        }))
    }

    /// Stops reporting: signing out or quitting. Returns the report to send.
    pub(crate) fn final_report(&mut self) -> Option<Report> {
        let report = self.session.as_mut().and_then(|s| s.reporter.stopped());
        report.filter(|_| !self.listening.private)
    }

    /// Hands what is playing to the desktop. The cover goes as the cached
    /// file, never as a server URL: that one carries the token.
    pub(crate) fn publish(&self) {
        let current = self.playback.current().map(|(_, q)| &q.meta);
        let art_url = current
            .and_then(|m| m.artwork.as_ref())
            .zip(self.artwork.dir())
            .and_then(|((id, tag), dir)| {
                crate::integration::art_url(
                    &dir.join(crate::artwork::ArtKey::new(id, tag).file_name()),
                )
            });
        crate::integration::publish(
            &self.desktop,
            &crate::integration::NowPlaying {
                track_id: current.map(|m| m.id.clone()),
                title: current.map(|m| m.title.clone()).unwrap_or_default(),
                artist: current.map(|m| m.artist.clone()).unwrap_or_default(),
                album_artist: current.and_then(|m| m.album_artist.clone()),
                album: current.map(|m| m.album.clone()).unwrap_or_default(),
                track_number: current
                    .and_then(|m| m.track_number)
                    .and_then(|n| u32::try_from(n).ok()),
                art_url,
                length_ms: current
                    .and_then(|m| m.duration_ms)
                    .map(|ms| ms.max(0) as u64),
                position_ms: self.playback.position_ms(),
                playing: self.playback.is_playing(),
                has_next: self.playback.ahead() > 0,
                shuffle: self.playback.is_shuffled(),
                repeat: self.playback.repeat(),
                volume: self.playback.volume(),
            },
        );
    }
}

/// One report to Jellyfin.
pub async fn send_report(
    client: JellyfinClient,
    report: Report,
) -> Result<(), aoide_jellyfin::Error> {
    let ms = |ms: u64| Duration::from_millis(ms);
    match report {
        Report::Started { item, position_ms } => {
            client.report_playback_started(&item, ms(position_ms)).await
        }
        Report::Progress {
            item,
            position_ms,
            paused,
        } => {
            client
                .report_playback_progress(&item, ms(position_ms), paused)
                .await
        }
        Report::Stopped { item, position_ms } => {
            client.report_playback_stopped(&item, ms(position_ms)).await
        }
    }
}

/// The last report and the sign-out together, inside the grace period: the
/// token has to be alive for both, and neither may hold up the screen.
pub async fn leave_server(client: JellyfinClient, report: Option<Report>) {
    let _ = tokio::time::timeout(session::SIGN_OUT_GRACE, async {
        if let Some(report) = report {
            let _ = send_report(client.clone(), report).await;
        }
        let _ = client.sign_out().await;
    })
    .await;
}

/// The window is closing: the last report gets a moment, no more.
pub async fn report_on_quit(client: JellyfinClient, report: Report) {
    let _ = tokio::time::timeout(Duration::from_millis(1_500), send_report(client, report)).await;
}
