//! The sidebar's playlists, read from the store.

use iced::Task;

use super::{App, Message};
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

    pub(super) fn playlists_loaded(&mut self, playlists: Vec<PlaylistSummary>) -> Task<Message> {
        if let Some(signed) = self.session.as_mut() {
            signed.playlists = playlists;
        }
        Task::none()
    }
}
