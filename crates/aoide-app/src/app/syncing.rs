//! When the Melete sync runs, and what its outcome changes.

use aoide_store::SyncReport;
use iced::Task;

use super::{App, Message};
use crate::screens::{Page, PageMsg, playlist};
use crate::sync;

impl App {
    /// A sync, if one is due: at launch, on focus, at most once a minute.
    pub(super) fn request_sync(&mut self) -> Task<Message> {
        let now = self.now();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let Some(engine) = signed.sync.clone() else {
            return Task::none();
        };
        if signed.handoff.publishing {
            signed.handoff.sync_wanted = true;
            return Task::none();
        }
        // Not before the store is known to be this account's.
        if signed.unknown_owner || !signed.throttle.request(now) {
            return Task::none();
        }
        let (task, handle) = Task::perform(sync::run(engine), Message::Synced).abortable();
        signed.syncing = Some(handle);
        task
    }

    /// Sync Now: at once unless a sync is running; the screen goes on as
    /// the sync runs.
    pub(crate) fn sync_now(&mut self) -> Task<Message> {
        let now = self.now();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let Some(engine) = signed.sync.clone() else {
            return Task::none();
        };
        if signed.handoff.publishing {
            signed.handoff.sync_wanted = true;
            return Task::none();
        }
        if signed.unknown_owner || !signed.throttle.request_now(now) {
            return Task::none();
        }
        let (task, handle) = Task::perform(sync::run(engine), Message::Synced).abortable();
        signed.syncing = Some(handle);
        task
    }

    pub(super) fn synced(&mut self, result: Result<SyncReport, String>) -> Task<Message> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        signed.throttle.finished();
        signed.syncing = None;
        let changed = result.as_ref().is_ok_and(|r| r.applied > 0);
        signed.sync_status = Some(result.as_ref().map(sync::describe).map_err(Clone::clone));
        let mut built_ins = Task::none();
        if result.is_ok() {
            // After a sync, never before the first: until then a built-in the server
            // has isn't here, and making it again would replace it on every device.
            // The store thread runs jobs in order, so the playlists read after this.
            let now = aoide_store::now_millis();
            self.store.send(move |w| {
                let _ = w.store.ensure_built_in_playlists(now);
            });
            built_ins = self.load_playlists();
        }
        let panels = self.refresh_sync_panels();
        if changed {
            Task::batch([built_ins, self.reload_playlist_pages(), panels])
        } else {
            Task::batch([built_ins, panels])
        }
    }

    /// Every open playlist page, asked to read its playlist again: a sync
    /// changed the store, perhaps bringing a waiting song's track.
    fn reload_playlist_pages(&mut self) -> Task<Message> {
        let Some(nav) = self.session.as_mut().and_then(|s| s.nav.as_mut()) else {
            return Task::none();
        };
        let keys: Vec<u64> = nav
            .iter_mut()
            .filter(|e| matches!(e.page, Page::Playlist(_)))
            .map(|e| e.key)
            .collect();
        Task::batch(keys.into_iter().map(|key| {
            Task::done(Message::Page(
                key,
                PageMsg::Playlist(playlist::Message::Reload),
            ))
        }))
    }
}
