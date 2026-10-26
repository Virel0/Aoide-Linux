//! Handoff and the resume grid.
//!
//! **Publishing.** Every change the player makes (a new queue, a skip, a
//! reorder, a seek, the ten-second progress report) notes the queue into the
//! store thread's [`QueueBroadcast`], whatever screen is open. The broadcast
//! settles a burst into one `queueState` row; when a row is written,
//! `SyncEngine::publish_queue` pushes just that row, so another device can
//! pick up without waiting for the next full sync.
//!
//! **Remembering.** Playback started from an album's, a playlist's or a
//! station's own page is a [`RecentContext`], kept per server in a JSON file
//! beside the store. Shuffle-all, search results and Infinity are not.
//!
//! **The grid.** Home's first six tiles: [`resume_tiles`] over those contexts,
//! led by "Pick up from {device}" when another device was playing more
//! recently than anything here. A tile plays at once; picking up keeps the
//! other device's earlier tracks and starts at its track and place.

use std::path::PathBuf;

use aoide_store::{
    QueueSnapshot, RecentContext, RecentContextKind, RecentContexts, ResumableQueue, ResumeTile,
};
use iced::{Element, Task};

use crate::actions::{Collection, PlayRequest};
use crate::app::{App, Message};
use crate::config::Paths;
use crate::playback::Enqueue;
use crate::screens::{PageMsg, home};

mod resume;

pub use resume::{Card, Resumed, ordered};
use resume::{load_cards, resume};

/// What handoff and the grid keep while signed in.
#[derive(Debug, Default)]
pub struct Handoff {
    pub recents: RecentContexts,
    file: Option<PathBuf>,
    /// The server the contexts are kept under.
    server: String,
    /// `publish_queue` is in flight: a sync asked for meanwhile waits for it.
    pub publishing: bool,
    pub sync_wanted: bool,
    /// After a pick-up: when this track starts, seek it to here.
    seek_on_start: Option<(String, u64)>,
    /// The queue just picked up, not offered again until it changes.
    picked_up: Option<PickedUp>,
}

/// Which queue was picked up: the same device, list and place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickedUp {
    device_id: String,
    track_ids: Vec<String>,
    position: usize,
}

impl PickedUp {
    pub fn of(queue: &ResumableQueue) -> Self {
        Self {
            device_id: queue.device_id.clone(),
            track_ids: queue.track_ids.clone(),
            position: queue.position,
        }
    }
}

impl Handoff {
    /// Reads the contexts kept for `server` (a missing file is an empty list).
    pub fn open(paths: Option<&Paths>, server: &str) -> Self {
        let file = paths.map(recents_file);
        Self {
            recents: file.as_ref().map(RecentContexts::load).unwrap_or_default(),
            file,
            server: server.to_owned(),
            ..Self::default()
        }
    }

    pub fn contexts(&self) -> &[RecentContext] {
        self.recents.list(&self.server)
    }
}

/// Where the contexts live: beside the store, in the data directory.
pub fn recents_file(paths: &Paths) -> PathBuf {
    paths.store_file.with_file_name("recent-contexts.json")
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// The broadcast's debounce may have passed: write the row if so.
    FlushDue,
    /// Whether that wrote a row.
    Flushed(bool),
    /// The push finished, however it went: the next sync sends it otherwise.
    Published,
    /// Home with this key wants its grid.
    LoadResume(u64),
    Resume(ResumeTile),
    /// "Remove from Recents".
    Forget(String),
    /// Playback started from a page that is a context.
    PlayFrom(Box<(PlayRequest, RecentContext)>),
    Resumed(Box<Resumed>),
    ResumeFailed(String),
}

/// Turns every play this page starts from its own list (Play, Shuffle, a row)
/// into a play from `context`, so it is remembered. Adding to the queue is not
/// starting from something, so only a replacement counts.
pub fn played_from<'a>(
    element: Element<'a, Message>,
    context: Option<RecentContext>,
) -> Element<'a, Message> {
    let Some(context) = context else {
        return element;
    };
    element.map(move |message| from_context(message, &context))
}

/// One message as [`played_from`] rewrites it.
pub fn from_context(message: Message, context: &RecentContext) -> Message {
    match message {
        Message::Play(request) if matches!(request.how, Enqueue::Replace { .. }) => {
            Message::Handoff(Msg::PlayFrom(Box::new((request, context.clone()))))
        }
        other => other,
    }
}

/// The context an album, artist or playlist menu's Play stands for.
pub fn collection_context(collection: &Collection, title: &str) -> Option<RecentContext> {
    let now = aoide_store::now_millis();
    match collection {
        Collection::Album(id) => Some(RecentContext::new(
            RecentContextKind::Album,
            id.clone(),
            title,
            now,
        )),
        Collection::Playlist(id) => Some(RecentContext::new(
            RecentContextKind::LocalPlaylist,
            id.clone(),
            title,
            now,
        )),
        Collection::JellyfinPlaylist(id) => Some(RecentContext::new(
            RecentContextKind::JellyfinPlaylist,
            id.clone(),
            title,
            now,
        )),
        // An artist's songs album by album are not something to resume.
        Collection::Artist(_) => None,
    }
}

impl App {
    /// Notes the queue as it stands into the broadcast, and looks again once
    /// the debounce has had time to pass. Called on every player change.
    pub(crate) fn broadcast_queue(&self) -> Task<Message> {
        if self.session.is_none() {
            return Task::none();
        }
        let (track_ids, position) = self.playback.saved_form();
        let snapshot = QueueSnapshot {
            device_name: crate::app::device_name(),
            track_ids,
            position,
            elapsed_ms: i64::try_from(self.playback.position_ms()).unwrap_or(0),
        };
        let now = aoide_store::now_millis();
        self.store.send(move |w| w.broadcast.note(snapshot, now));
        let wait =
            std::time::Duration::from_millis(aoide_store::QueueBroadcast::DEBOUNCE_MS as u64 + 50);
        Task::perform(tokio::time::sleep(wait), |()| {
            Message::Handoff(Msg::FlushDue)
        })
    }

    pub(crate) fn handoff(&mut self, message: Msg) -> Task<Message> {
        let store = self.store.clone();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        match message {
            Msg::FlushDue => {
                let now = aoide_store::now_millis();
                Task::perform(
                    async move {
                        store
                            .call(move |w| w.broadcast.flush_due(&w.store, now))
                            .await
                            .and_then(Result::ok)
                            .unwrap_or(false)
                    },
                    |written| Message::Handoff(Msg::Flushed(written)),
                )
            }
            Msg::Flushed(written) => self.publish_queue(written),
            Msg::Published => {
                signed.handoff.publishing = false;
                if std::mem::take(&mut signed.handoff.sync_wanted) {
                    return Task::done(Message::SyncNow);
                }
                Task::none()
            }
            Msg::LoadResume(key) => self.load_resume(key),
            Msg::Resume(tile) => {
                let client = signed.connection.client.clone();
                Task::perform(resume(client, store, tile), |result| match result {
                    Ok(found) => Message::Handoff(Msg::Resumed(Box::new(found))),
                    Err(error) => Message::Handoff(Msg::ResumeFailed(error)),
                })
            }
            Msg::Resumed(found) => self.resumed(*found),
            Msg::ResumeFailed(error) => {
                self.notice = Some(error);
                Task::none()
            }
            Msg::Forget(key) => {
                let server = signed.handoff.server.clone();
                signed.handoff.recents.forget(&server, &key);
                self.save_recents();
                self.refresh_home()
            }
            Msg::PlayFrom(boxed) => {
                let (request, context) = *boxed;
                let remembered = self.remembered(Some(context));
                Task::batch([self.play(request), remembered])
            }
        }
    }

    /// A row was written: push it now, unless a sync (which pushes it
    /// anyway) or another push is running.
    fn publish_queue(&mut self, written: bool) -> Task<Message> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let Some(engine) = signed.sync.clone().filter(|_| written) else {
            return Task::none();
        };
        if signed.handoff.publishing || signed.throttle.is_running() {
            return Task::none();
        }
        signed.handoff.publishing = true;
        Task::perform(async move { engine.publish_queue().await }, |_| {
            Message::Handoff(Msg::Published)
        })
    }

    /// The grid for the Home page with this key.
    fn load_resume(&mut self, key: u64) -> Task<Message> {
        let shared = self.store.shared();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let contexts = signed.handoff.contexts().to_vec();
        let picked_up = signed.handoff.picked_up.clone();
        let client = signed.connection.client.clone();
        let sidecar = signed.sidecar.clone();
        let now = aoide_store::now_millis();
        Task::perform(
            load_cards(client, sidecar, shared, contexts, picked_up, now),
            move |cards| Message::Page(key, PageMsg::Home(home::Message::Resume(cards))),
        )
    }

    /// A tile's tracks found: played, a pick-up seeking to where the other
    /// device was.
    fn resumed(&mut self, found: Resumed) -> Task<Message> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let Resumed {
            tracks,
            start,
            source,
            seek_ms,
            picked_up,
            context,
        } = found;
        if tracks.is_empty() {
            self.notice = Some("There's nothing there to play now.".to_owned());
            return Task::none();
        }
        signed.handoff.seek_on_start =
            seek_ms.and_then(|ms| tracks.get(start).map(|t| (t.id.clone(), ms)));
        if picked_up.is_some() {
            signed.handoff.picked_up = picked_up;
        }
        // A pick-up changes the grid too: its tile goes.
        let refresh = if context.is_some() {
            self.remembered(context)
        } else {
            self.refresh_home()
        };
        Task::batch([
            self.play(PlayRequest::from(&tracks, start, source)),
            refresh,
        ])
    }

    /// Moves `context` to the front of this server's recents, stamped now.
    pub(crate) fn remember(&mut self, mut context: RecentContext) {
        let Some(signed) = self.session.as_mut() else {
            return;
        };
        context.last_opened = aoide_store::now_millis();
        let server = signed.handoff.server.clone();
        signed.handoff.recents.remember(&server, context);
        self.save_recents();
    }

    /// Remembers `context`, if there is one, and redraws the grids that show it.
    pub(crate) fn remembered(&mut self, context: Option<RecentContext>) -> Task<Message> {
        let Some(context) = context else {
            return Task::none();
        };
        self.remember(context);
        self.refresh_home()
    }

    fn save_recents(&self) {
        let Some(handoff) = self.session.as_ref().map(|s| &s.handoff) else {
            return;
        };
        let (Some(file), recents) = (handoff.file.clone(), handoff.recents.clone()) else {
            return;
        };
        // On the store thread: nothing on the UI thread waits on the disk.
        self.store.send(move |_| {
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = recents.save(&file);
        });
    }

    /// Every open Home page (showing, or below in the stack, which Back
    /// returns to), asked for its grid again.
    pub(crate) fn refresh_home(&mut self) -> Task<Message> {
        let Some(nav) = self.session.as_mut().and_then(|s| s.nav.as_mut()) else {
            return Task::none();
        };
        let keys: Vec<u64> = nav
            .iter_mut()
            .filter(|entry| matches!(entry.page, crate::screens::Page::Home(_)))
            .map(|entry| entry.key)
            .collect();
        Task::batch(
            keys.into_iter()
                .map(|key| Task::done(Message::Handoff(Msg::LoadResume(key)))),
        )
    }

    /// What follows an engine event for handoff: a picked-up track seeks to
    /// where the other device was.
    pub(crate) fn handoff_engine_event(
        &mut self,
        event: &aoide_audio::EngineEvent,
    ) -> Task<Message> {
        let aoide_audio::EngineEvent::TrackStarted { id } = event else {
            return Task::none();
        };
        let started = self.playback.queued(*id).map(|q| q.meta.id.clone());
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let Some((track, ms)) = signed.handoff.seek_on_start.take() else {
            return Task::none();
        };
        if started.as_deref() == Some(track.as_str())
            && let Err(error) = self.playback.seek(ms)
        {
            self.notice = Some(error);
        }
        Task::none()
    }
}

#[cfg(test)]
mod tests {
    use aoide_jellyfin::{Item, MixSeed};
    use aoide_store::{PlaySource, StationSeed};

    use super::resume::{cards, plan_pick_up, station_seed};
    use super::*;
    use crate::artwork::ArtKey;
    use crate::playback::tracks::TrackMeta;
    use crate::store_worker::StoreHandle;
    use aoide_store::QueueFreshness;

    fn item(id: &str, album: &str) -> Item {
        serde_json::from_value(serde_json::json!({
            "Id": id, "Name": id, "Type": "Audio", "AlbumId": album,
            "AlbumPrimaryImageTag": format!("tag-{album}")
        }))
        .unwrap()
    }

    fn queue(ids: &[&str], position: usize, elapsed_ms: i64) -> ResumableQueue {
        ResumableQueue {
            device_id: "phone".into(),
            device_name: "Phone".into(),
            track_ids: ids.iter().map(|s| (*s).to_owned()).collect(),
            position,
            elapsed_ms,
            age_ms: 1_000,
            observed_at: 10_000,
            freshness: QueueFreshness::Server,
        }
    }

    #[test]
    fn a_pick_up_keeps_earlier_tracks_and_starts_at_the_named_one() {
        let queue = queue(&["a", "gone", "b", "c"], 2, 42_000);
        let found = [item("c", "x"), item("a", "x"), item("b", "x")];
        let plan = plan_pick_up(&queue, &found).unwrap();
        let ids: Vec<&str> = plan.tracks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        assert_eq!(plan.start, 1, "b, found by id past the missing one");
        assert_eq!(plan.seek_ms, Some(42_000));
        assert!(plan.context.is_none(), "a pick-up is not a context");
        assert_eq!(plan.picked_up, Some(PickedUp::of(&queue)));
    }

    #[test]
    fn a_missing_current_track_starts_the_next_from_the_top() {
        let queue = queue(&["a", "b", "c"], 1, 42_000);
        let plan = plan_pick_up(&queue, &[item("a", "x"), item("c", "x")]).unwrap();
        assert_eq!(plan.tracks[plan.start].id, "c");
        assert_eq!(plan.seek_ms, None);
        assert!(plan_pick_up(&queue, &[]).is_none());
    }

    #[test]
    fn tiles_draw_the_cover_of_the_item_they_name() {
        let album = RecentContext::new(RecentContextKind::Album, "al1", "Low Tide", 5);
        let mut station = RecentContext::new(RecentContextKind::Station, "t9", "Radio", 4);
        station.artwork_item_id = Some("t9".into());
        let tiles = vec![
            ResumeTile::Handoff(queue(&["t1"], 0, 0)),
            ResumeTile::Context(album),
            ResumeTile::Context(station),
        ];
        let album_item: Item = serde_json::from_value(serde_json::json!({
            "Id": "al1", "Name": "Low Tide", "Type": "MusicAlbum", "ImageTags": {"Primary": "p"}
        }))
        .unwrap();
        let cards = cards(tiles, &[item("t1", "al2"), album_item]);
        assert_eq!(cards[0].art, Some(ArtKey::new("al2", "tag-al2")));
        assert_eq!(cards[1].art, Some(ArtKey::new("al1", "p")));
        assert_eq!(cards[2].art, None, "t9 was not found");
    }

    #[test]
    fn menus_remember_albums_and_playlists_not_artists() {
        let album = collection_context(&Collection::Album("al1".into()), "Low Tide").unwrap();
        assert_eq!(album.kind, RecentContextKind::Album);
        assert_eq!(album.name, "Low Tide");
        let list = collection_context(&Collection::Playlist("p1".into()), "Mine").unwrap();
        assert_eq!(list.kind, RecentContextKind::LocalPlaylist);
        assert!(collection_context(&Collection::Artist("ar".into()), "X").is_none());
    }

    #[test]
    fn only_a_replacement_from_the_page_is_remembered() {
        let context = RecentContext::new(RecentContextKind::Album, "al1", "Low Tide", 1);
        let tracks = vec![TrackMeta::from_item(&item("t1", "al1"))];
        let replace = Message::Play(PlayRequest::from(&tracks, 0, PlaySource::Album));
        assert!(matches!(
            from_context(replace, &context),
            Message::Handoff(Msg::PlayFrom(_))
        ));
        let next = Message::Play(PlayRequest {
            tracks,
            how: Enqueue::Next,
            source: PlaySource::Album,
        });
        assert!(matches!(from_context(next, &context), Message::Play(_)));
        assert!(matches!(
            from_context(Message::Back, &context),
            Message::Back
        ));
    }

    #[test]
    fn a_station_resumes_from_its_seeds_own_mix() {
        let album = RecentContext::station(StationSeed::Album, "al1", "Low Tide Radio", 1);
        assert_eq!(station_seed(&album), Some(MixSeed::Album("al1".into())));
        let song = RecentContext::new(RecentContextKind::Station, "t1", "Lantern Radio", 1);
        assert_eq!(station_seed(&song), None, "kind-blind Instant Mix");
    }

    #[test]
    fn items_come_back_in_the_lists_order() {
        let ids: Vec<String> = ["b", "a", "b"].map(String::from).to_vec();
        let got = ordered(vec![item("a", "x"), item("b", "x")], &ids);
        let names: Vec<&str> = got.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(names, ["b", "a", "b"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_flushed_queue_is_published_alone() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/aoide/sync/push"))
            .respond_with(|request: &wiremock::Request| {
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                let ops = body["ops"].as_array().unwrap();
                assert!(ops.iter().all(|op| op["entity"] == "queue_state"), "{body}");
                let ids: Vec<_> = ops.iter().map(|op| op["opId"].clone()).collect();
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "accepted": ids, "rejected": [], "cursor": 1
                }))
            })
            .expect(1)
            .mount(&server)
            .await;
        let store = StoreHandle::in_memory("desk").unwrap();
        store
            .call(|w| {
                // A playlist edit waits too; publishing must leave it for the sync.
                w.store.create_playlist("Later", None, None, 1).unwrap();
                w.broadcast.note(
                    QueueSnapshot {
                        device_name: "Desk".into(),
                        track_ids: vec!["t1".into(), "t2".into()],
                        position: 1,
                        elapsed_ms: 3_000,
                    },
                    1_000,
                );
                assert!(
                    !w.broadcast.flush_due(&w.store, 1_100).unwrap(),
                    "debounced"
                );
                assert!(w.broadcast.flush_due(&w.store, 3_000).unwrap());
            })
            .await
            .unwrap();
        let client = aoide_sidecar::SidecarClient::new(&server.uri(), "tok").unwrap();
        let engine = aoide_sidecar::SyncEngine::new(store.shared(), client);
        assert_eq!(engine.publish_queue().await.unwrap(), 1);
        let waiting = store
            .call(|w| w.store.pending_op_count().unwrap())
            .await
            .unwrap();
        assert!(waiting >= 1, "the playlist is still waiting");
    }
}
