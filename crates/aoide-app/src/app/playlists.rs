//! The sidebar's playlists, read from the store, and their covers.

use iced::Task;

use super::{App, Message};
use crate::covers::{self, Resolved};
use crate::screens::PlaylistSummary;

impl App {
    pub(crate) fn load_playlists(&self) -> Task<Message> {
        let store = self.store.clone();
        Task::perform(
            async move {
                store
                    .call(|w| {
                        let lists = w.store.playlists().unwrap_or_default();
                        let now = aoide_store::now_millis();
                        lists
                            .into_iter()
                            .map(|p| PlaylistSummary {
                                // A smart playlist's count is what its rules match now.
                                count: if p.is_smart {
                                    w.store
                                        .match_count_of(&p, now, aoide_store::Calendar::Local)
                                        .unwrap_or(0)
                                } else {
                                    w.store.item_count(&p.id).unwrap_or(0)
                                },
                                smart: p.is_smart,
                                folder_id: p.folder_id,
                                id: p.id,
                                name: p.name,
                            })
                            .collect::<Vec<_>>()
                    })
                    .await
                    .unwrap_or_default()
            },
            Message::Playlists,
        )
    }

    /// The playlists read: listed, and their covers found for the sidebar.
    pub(super) fn playlists_loaded(&mut self, playlists: Vec<PlaylistSummary>) -> Task<Message> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        signed.playlists = playlists;
        let ids = signed.playlists.iter().map(|p| p.id.clone()).collect();
        let store = self.store.clone();
        let shared = store.shared();
        let blobs = covers::open_blobs(self.paths.as_ref());
        Task::perform(
            covers::resolve(store, shared, signed.sidecar.clone(), blobs, ids),
            |found| Message::Covers(covers::Msg::Sidebar(Box::new(found))),
        )
    }

    /// The sidebar's covers found: the chosen pictures into the artwork
    /// cache, and the keys to draw.
    pub(crate) fn sidebar_covers(&mut self, found: Resolved) -> Task<Message> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let Resolved { art, bytes } = found;
        for (key, bytes) in bytes {
            self.artwork.insert(key, bytes);
        }
        signed.playlist_art = art;
        Task::none()
    }
}
