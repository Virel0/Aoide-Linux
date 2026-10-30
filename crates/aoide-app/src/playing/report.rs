//! What the desktop hears about what plays (the now-playing state for MPRIS),
//! and the server's last word on signing out. Melete learns what was played
//! from the play events that sync carries.

use aoide_melete::MeleteClient;

use crate::app::App;
use crate::session;

impl App {
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

/// Signs out on the server, inside the grace period, so it never holds up the
/// screen.
pub async fn leave_server(client: MeleteClient) {
    let _ = tokio::time::timeout(session::SIGN_OUT_GRACE, client.sign_out()).await;
}
