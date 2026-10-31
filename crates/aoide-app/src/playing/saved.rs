//! The saved queue: read back at launch and held, and written through the
//! handoff broadcast as it changes.

use aoide_melete::{Item, MeleteClient};
use iced::Task;

use super::{Destination, Pending};
use crate::app::{App, Message};
use crate::playback::tracks::TrackMeta;

/// The saved queue, read back: ids in order, where it was, how far in.
#[derive(Debug, Clone)]
pub struct Saved {
    pub items: Vec<Item>,
    pub position: usize,
    pub elapsed_ms: u64,
}

/// Reads the saved queue and fetches its tracks. The saved position names a
/// track; if some tracks have gone from the server since, the position follows
/// that track rather than its old index.
pub async fn read_saved(
    store: crate::store_worker::StoreHandle,
    client: MeleteClient,
) -> Result<Option<Saved>, aoide_melete::Error> {
    let saved = store
        .call(|w| w.store.local_queue().ok().flatten())
        .await
        .flatten();
    let Some(saved) = saved else {
        return Ok(None);
    };
    let ids = saved.track_id_list();
    if ids.is_empty() {
        return Ok(None);
    }
    let position = usize::try_from(saved.position.max(0))
        .unwrap_or(0)
        .min(ids.len() - 1);
    let current = ids[position].clone();
    let items = client.items_by_ids(&ids).await?;
    if items.is_empty() {
        return Ok(None);
    }
    let position = items
        .iter()
        .position(|item| item.id == current)
        .unwrap_or(0);
    Ok(Some(Saved {
        items,
        position,
        elapsed_ms: u64::try_from(saved.elapsed_ms.max(0)).unwrap_or(0),
    }))
}

impl App {
    /// At launch: the queue as it was left, held, not playing.
    pub(crate) fn restore_queue(&self) -> Task<Message> {
        let Some(signed) = &self.session else {
            return Task::none();
        };
        if self.playback.queue().is_some_and(|q| !q.is_empty()) {
            return Task::none();
        }
        let store = self.store.clone();
        let client = signed.connection.client.clone();
        let generation = signed.play_generation;
        Task::perform(read_saved(store, client), move |result| match result {
            Err(aoide_melete::Error::Unauthorized) => Message::TokenRejected,
            Ok(Some(saved)) => Message::Restore(Box::new(Pending {
                tracks: saved.items.iter().map(TrackMeta::from_item).collect(),
                how: Destination::Restore {
                    position: saved.position,
                    elapsed_ms: saved.elapsed_ms,
                },
                generation,
            })),
            _ => Message::Ignore,
        })
    }

    /// Writes the queue as it stands, when it has changed enough to matter:
    /// through the handoff broadcast, which settles a burst of changes into
    /// one row and publishes it (see `handoff`).
    pub(crate) fn save_queue(&mut self) -> Task<Message> {
        Task::batch([self.broadcast_queue(), self.share_change()])
    }
}
