//! The covers on screen: which are wanted, and their downloads.

use iced::Task;

use super::{App, Message};
use crate::artwork::{self, ArtKey};

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
        if let Some(queue) = self.playback.queue() {
            let position = queue.position().unwrap_or(0);
            let first = position.saturating_sub(5);
            for index in first..(first + 60).min(queue.entries().len()) {
                if let Some((id, tag)) = self
                    .playback
                    .queued_at(index)
                    .and_then(|q| q.meta.artwork.as_ref())
                {
                    wanted.push(ArtKey::new(id, tag));
                }
            }
        }
        self.artwork.want(wanted);
        let client = signed.connection.client.clone();
        let authorization = client.authorization();
        let dir = self.artwork.dir().map(ToOwned::to_owned);
        Task::batch(self.artwork.next_batch().into_iter().map(|key| {
            let url = client.cover_url(&key.item).to_string();
            let dir = dir.clone();
            let done = key.clone();
            Task::perform(
                artwork::fetch(url, authorization.clone(), dir, key),
                move |bytes| Message::Artwork(done.clone(), bytes),
            )
        }))
    }
}
