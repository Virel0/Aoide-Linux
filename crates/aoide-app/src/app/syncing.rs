//! When the Melete sync runs, and what its outcome changes.

use aoide_store::SyncReport;
use iced::Task;

use super::{App, Message};
use crate::screens::{Page, PageMsg, playlist};
use crate::{sync, track_fill};

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
            let user = signed.connection.account.user_id.clone();
            self.store.send(move |w| {
                let _ = w.store.ensure_built_in_playlists(&user, now);
            });
            // A collection made in the web since shows up in the sidebar.
            let client = signed.connection.client.clone();
            built_ins = Task::batch([
                self.load_playlists(),
                crate::screens::web_collection::ask(client),
            ]);
        }
        let panels = self.refresh_sync_panels();
        // Other devices' plays may have come in, of songs not cached here.
        let fill = if result.is_ok() {
            self.fill_track_cache()
        } else {
            Task::none()
        };
        if changed {
            Task::batch([built_ins, self.reload_playlist_pages(), panels, fill])
        } else {
            Task::batch([built_ins, panels, fill])
        }
    }

    /// Caches the played tracks the track cache lacks, off the UI thread;
    /// see `track_fill`.
    pub(super) fn fill_track_cache(&mut self) -> Task<Message> {
        let store = self.store.clone();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let Some(skip) = signed.track_fill.start() else {
            return Task::none();
        };
        let client = signed.connection.client.clone();
        Task::perform(
            track_fill::fill(client, store, skip),
            Message::TrackCacheFilled,
        )
    }

    pub(super) fn track_cache_filled(&mut self, filled: track_fill::Filled) -> Task<Message> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        if let Some(error) = &filled.failed {
            log::warn!("Played songs couldn't be added to the track cache: {error}");
        }
        let again = signed.track_fill.finished(filled.gone);
        let reload = if filled.cached > 0 {
            // A smart playlist's history rules can match them now, and a built-in
            // left out of the list while empty may have songs.
            Task::batch([self.load_playlists(), self.reload_playlist_pages()])
        } else {
            Task::none()
        };
        if again {
            Task::batch([reload, self.fill_track_cache()])
        } else {
            reload
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
