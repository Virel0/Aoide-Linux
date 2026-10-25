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

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use aoide_jellyfin::{Item, JellyfinClient, MixSeed};
use aoide_sidecar::resumable_queues;
use aoide_store::{
    Calendar, PlaySource, QueueSnapshot, RESUME_GRID_LIMIT, RecentContext, RecentContextKind,
    RecentContexts, ResumableQueue, ResumeTile, StationSeed, resume_tiles,
};
use iced::{Element, Task};

use crate::actions::{Collection, PlayRequest};
use crate::app::{App, Message};
use crate::artwork::ArtKey;
use crate::config::Paths;
use crate::playback::Enqueue;
use crate::playback::tracks::TrackMeta;
use crate::screens::{PageMsg, home};
use crate::store_worker::StoreHandle;

/// How many tracks a station resumed from the grid starts with.
pub const STATION_LENGTH: u32 = 100;

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

/// A tile of the grid, with the cover to draw.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub tile: ResumeTile,
    pub art: Option<ArtKey>,
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

/// A tile's tracks, found.
#[derive(Debug, Clone)]
pub struct Resumed {
    pub tracks: Vec<TrackMeta>,
    pub start: usize,
    pub source: PlaySource,
    /// Where in the start track to begin (a pick-up).
    pub seek_ms: Option<u64>,
    /// The queue this picks up.
    pub picked_up: Option<PickedUp>,
    /// The context to move to the front again.
    pub context: Option<RecentContext>,
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

/// The station to play for a station context: the seed's own Instant Mix
/// when it says what it was, else the kind-blind one.
pub fn station_seed(context: &RecentContext) -> Option<MixSeed> {
    match context.seed? {
        StationSeed::Album => Some(MixSeed::Album(context.id.clone())),
        StationSeed::Artist => Some(MixSeed::Artist(context.id.clone())),
    }
}

/// The other device's queue in its order, as far as this server still has
/// it, starting at its track. `None` when nothing in it can play here.
pub fn plan_pick_up(queue: &ResumableQueue, found: &[Item]) -> Option<Resumed> {
    let by_id: HashMap<&str, &Item> = found.iter().map(|i| (i.id.as_str(), i)).collect();
    let pick = queue.pick_up(|id| by_id.contains_key(id))?;
    let tracks: Vec<TrackMeta> = pick
        .track_ids
        .iter()
        .filter_map(|id| {
            by_id
                .get(id.as_str())
                .map(|item| TrackMeta::from_item(item))
        })
        .collect();
    let seek = u64::try_from(pick.elapsed_ms).unwrap_or(0);
    Some(Resumed {
        tracks,
        start: pick.start_index,
        source: PlaySource::Unknown,
        seek_ms: (seek > 0).then_some(seek),
        picked_up: Some(PickedUp::of(queue)),
        context: None,
    })
}

/// Each tile's cover, from the items its artwork ids name.
pub fn cards(tiles: Vec<ResumeTile>, items: &[Item]) -> Vec<Card> {
    let art: HashMap<&str, ArtKey> = items
        .iter()
        .filter_map(|item| {
            let (id, tag) = item.artwork_source()?;
            Some((item.id.as_str(), ArtKey::new(id, tag)))
        })
        .collect();
    tiles
        .into_iter()
        .map(|tile| Card {
            art: tile.artwork_item_id().and_then(|id| art.get(id).cloned()),
            tile,
        })
        .collect()
}

/// The grid's tiles: the newest queue another device put down, then this
/// device's contexts, with each tile's cover.
pub async fn load_cards(
    client: JellyfinClient,
    sidecar: Option<aoide_sidecar::SidecarClient>,
    store: std::sync::Arc<aoide_store::Store>,
    contexts: Vec<RecentContext>,
    picked_up: Option<PickedUp>,
    now: aoide_store::Millis,
) -> Vec<Card> {
    let queues = resumable_queues(sidecar.as_ref(), &store, now)
        .await
        .unwrap_or_default();
    let newest = queues
        .first()
        .filter(|queue| picked_up.as_ref() != Some(&PickedUp::of(queue)));
    let tiles = resume_tiles(&contexts, newest, RESUME_GRID_LIMIT);
    let ids: Vec<String> = tiles
        .iter()
        .filter_map(|t| t.artwork_item_id().map(str::to_owned))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let items = if ids.is_empty() {
        Vec::new()
    } else {
        client.items_by_ids(&ids).await.unwrap_or_default()
    };
    cards(tiles, &items)
}

/// A tile's tracks, from wherever its kind keeps them.
pub async fn resume(
    client: JellyfinClient,
    store: StoreHandle,
    tile: ResumeTile,
) -> Result<Resumed, String> {
    let describe = |e: aoide_jellyfin::Error| crate::session::describe(&e);
    let context = match tile {
        ResumeTile::Handoff(queue) => {
            let found = client
                .items_by_ids(&queue.track_ids)
                .await
                .map_err(describe)?;
            return plan_pick_up(&queue, &found).ok_or_else(|| {
                format!(
                    "None of what {} was playing is on this server.",
                    queue.device_name
                )
            });
        }
        ResumeTile::Context(context) => context,
    };
    let (items, source) = match context.kind {
        RecentContextKind::Album => {
            let mut tracks: Vec<TrackMeta> = client
                .album_tracks(&context.id)
                .await
                .map_err(describe)?
                .iter()
                .map(TrackMeta::from_item)
                .collect();
            crate::screens::album::album_order(&mut tracks);
            return Ok(Resumed {
                tracks,
                start: 0,
                source: PlaySource::Album,
                seek_ms: None,
                picked_up: None,
                context: Some(context),
            });
        }
        RecentContextKind::JellyfinPlaylist => (
            client.playlist_items(&context.id).await.map_err(describe)?,
            PlaySource::Playlist,
        ),
        RecentContextKind::LocalPlaylist => {
            let id = context.id.clone();
            let found = store
                .call(move |w| local_playlist_ids(&w.store, &id))
                .await
                .ok_or_else(|| "the library is closing".to_owned())?;
            let Some((ids, smart)) = found? else {
                return Err(format!(
                    "\u{201c}{}\u{201d} has been deleted.",
                    context.name
                ));
            };
            let items = ordered(client.items_by_ids(&ids).await.map_err(describe)?, &ids);
            let source = if smart {
                PlaySource::Smart
            } else {
                PlaySource::Playlist
            };
            (items, source)
        }
        RecentContextKind::Station => {
            let items = match station_seed(&context) {
                Some(seed) => client.instant_mix_from(&seed, STATION_LENGTH).await,
                None => client.instant_mix(&context.id, STATION_LENGTH).await,
            }
            .map_err(describe)?;
            let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
            let hidden = store
                .call(move |w| w.store.not_interested(&ids).unwrap_or_default())
                .await
                .unwrap_or_default();
            (
                items
                    .into_iter()
                    .filter(|i| !hidden.contains(&i.id))
                    .collect(),
                PlaySource::Unknown,
            )
        }
        RecentContextKind::Mix => {
            return Err(
                "A mix is made again from the Mixes screen, which this build doesn't have yet."
                    .to_owned(),
            );
        }
    };
    Ok(Resumed {
        tracks: items.iter().map(TrackMeta::from_item).collect(),
        start: 0,
        source,
        seek_ms: None,
        picked_up: None,
        context: Some(context),
    })
}

/// An Aoide playlist's track ids in play order, smart ones evaluated now, and
/// whether it is smart. `None` when it has been deleted.
pub fn local_playlist_ids(
    store: &aoide_store::Store,
    id: &str,
) -> Result<Option<(Vec<String>, bool)>, String> {
    let Some(playlist) = store.playlist(id).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    if playlist.deleted {
        return Ok(None);
    }
    if playlist.is_smart {
        let ids = store
            .resolve_smart(&playlist, aoide_store::now_millis(), Calendar::Local)
            .map_err(|e| e.to_string())?;
        return Ok(Some((ids, true)));
    }
    let ids = store
        .items(id)
        .map_err(|e| e.to_string())?
        .iter()
        .filter(|item| !item.is_unresolved())
        .map(|item| item.linked_jellyfin_id().to_owned())
        .collect();
    Ok(Some((ids, false)))
}

/// `items` in the order of `ids`, each as often as it is named.
pub fn ordered(items: Vec<Item>, ids: &[String]) -> Vec<Item> {
    let by_id: HashMap<String, Item> = items.into_iter().map(|i| (i.id.clone(), i)).collect();
    ids.iter().filter_map(|id| by_id.get(id).cloned()).collect()
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
            Msg::Flushed(written) => {
                // A row was written: push it now, unless a sync (which pushes
                // it anyway) or another push is running.
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
            Msg::Published => {
                signed.handoff.publishing = false;
                if std::mem::take(&mut signed.handoff.sync_wanted) {
                    return Task::done(Message::SyncNow);
                }
                Task::none()
            }
            Msg::LoadResume(key) => {
                let contexts = signed.handoff.contexts().to_vec();
                let picked_up = signed.handoff.picked_up.clone();
                let client = signed.connection.client.clone();
                let sidecar = signed.sidecar.clone();
                let shared = store.shared();
                let now = aoide_store::now_millis();
                Task::perform(
                    load_cards(client, sidecar, shared, contexts, picked_up, now),
                    move |cards| Message::Page(key, PageMsg::Home(home::Message::Resume(cards))),
                )
            }
            Msg::Resume(tile) => {
                let client = signed.connection.client.clone();
                Task::perform(resume(client, store, tile), |result| match result {
                    Ok(found) => Message::Handoff(Msg::Resumed(Box::new(found))),
                    Err(error) => Message::Handoff(Msg::ResumeFailed(error)),
                })
            }
            Msg::Resumed(found) => {
                let Resumed {
                    tracks,
                    start,
                    source,
                    seek_ms,
                    picked_up,
                    context,
                } = *found;
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
    use super::*;
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
