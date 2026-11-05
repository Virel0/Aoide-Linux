//! The covers and profile pictures on screen: which are wanted, and their
//! downloads.

use iced::Task;

use super::{App, Message};
use crate::artwork::{self, ArtKey};
use crate::avatars;

impl App {
    pub(super) fn artwork_arrived(&mut self, key: ArtKey, bytes: Option<Vec<u8>>) -> Task<Message> {
        self.artwork.arrived(key, bytes);
        Task::none()
    }

    /// Asks for the covers on screen and starts the downloads that are due.
    pub(super) fn fetch_artwork(&mut self) -> Task<Message> {
        let Some(signed) = &self.session else {
            return Task::none();
        };
        let mut wanted = Vec::new();
        if let Some(nav) = &signed.nav {
            wanted.extend(nav.current().page.artwork());
        }
        wanted.extend(
            signed
                .playlist_art
                .values()
                .flat_map(|cover| cover.keys().iter().cloned()),
        );
        // The playing song's cover as the Now Playing panel draws it (and the
        // desktop is shown), and as the full player's card does while it's open.
        if let Some((id, tag)) = self
            .playback
            .current()
            .and_then(|(_, q)| q.meta.artwork.as_ref())
        {
            wanted.push(ArtKey::sized(id, tag, artwork::TILE));
            if self.listening.full.is_some() {
                wanted.push(ArtKey::sized(id, tag, artwork::FULL));
            }
        }
        // The queue's rows and the player bar.
        if let Some(queue) = self.playback.queue() {
            let position = queue.position().unwrap_or(0);
            let first = position.saturating_sub(5);
            for index in first..(first + 60).min(queue.entries().len()) {
                if let Some((id, tag)) = self
                    .playback
                    .queued_at(index)
                    .and_then(|q| q.meta.artwork.as_ref())
                {
                    wanted.push(ArtKey::sized(id, tag, artwork::ROW));
                }
            }
        }
        wanted.extend(avatars::keys(
            std::iter::once(signed.avatar.as_deref()).chain(
                signed
                    .together
                    .session
                    .iter()
                    .flat_map(|s| s.members.iter().map(|m| m.avatar_url.as_deref())),
            ),
        ));
        self.artwork.want(wanted);
        let client = signed.connection.client.clone();
        let authorization = client.authorization();
        let dir = self.artwork.dir().map(ToOwned::to_owned);
        Task::batch(self.artwork.next_batch().into_iter().map(|key| {
            let url = match avatars::path_of(&key) {
                None => client.cover_url(&key.item, key.size),
                Some(path) => match client.avatar_url(path) {
                    Some(url) => url,
                    // Not a path on this server: nothing to fetch.
                    None => return Task::done(Message::Artwork(key, None)),
                },
            }
            .to_string();
            let dir = dir.clone();
            let done = key.clone();
            Task::perform(
                artwork::fetch(url, authorization.clone(), dir, key),
                move |bytes| Message::Artwork(done.clone(), bytes),
            )
        }))
    }
}
