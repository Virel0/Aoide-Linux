//! The audio side of the app: the output device, the player and its queue,
//! and what the app knows about each queued entry.
//!
//! The player has no thread of its own: every engine event arrives through
//! [`Playback::subscription`] and must be handed to [`Playback::handle`] (which
//! hands it to `Player::handle`), which is how the queue learns a track
//! boundary was heard and chains the next one.

pub mod infinity;
pub mod lyrics;
pub mod queue;
pub mod recorder;
pub mod report;
pub mod tracks;

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hash, Hasher};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use aoide_audio::{
    EngineEvent, EntryId, Output, PlayQueue, Player, PlayerSettings, PlayerTrack, RepeatMode,
    Transport,
};
use aoide_store::PlaySource;
use iced::Subscription;
use iced::futures::{SinkExt, Stream, StreamExt};

use tracks::{BuiltTrack, TrackMeta};

/// One queued entry, as the app knows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Queued {
    pub meta: TrackMeta,
    /// Infinity's factor summary, shown on the queue row.
    pub reason: Option<String>,
    pub source: PlaySource,
}

/// Where new tracks go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enqueue {
    /// Replace the queue and play from `start`; `shuffle` shuffles the rest
    /// around it.
    Replace { start: usize, shuffle: bool },
    /// The front of the hand-queued lane.
    Next,
    /// The back of the hand-queued lane.
    Last,
    /// Behind everything: what Infinity found.
    Extend,
}

/// The engine's event channel, handed to the subscription once.
#[derive(Clone)]
struct EventSource {
    id: u64,
    receiver: Arc<Mutex<Option<Receiver<EngineEvent>>>>,
}

impl Hash for EventSource {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

pub struct Playback {
    /// Kept alive for as long as there should be sound.
    _output: Option<Output>,
    player: Option<Player>,
    events: Option<EventSource>,
    /// Why there is no sound, when there is none.
    unavailable: Option<String>,
    meta: HashMap<EntryId, Queued>,
    position_ms: u64,
    volume: f32,
}

impl Playback {
    /// Opens the default output device. Without one the app still runs; it
    /// says why nothing plays.
    pub fn start(volume: f32) -> Self {
        match Output::start_default() {
            Ok((output, handle, events)) => {
                let mut player = Player::new(handle);
                let _ = player.set_volume(volume);
                Self {
                    _output: Some(output),
                    player: Some(player),
                    events: Some(EventSource {
                        id: 1,
                        receiver: Arc::new(Mutex::new(Some(events))),
                    }),
                    unavailable: None,
                    meta: HashMap::new(),
                    position_ms: 0,
                    volume,
                }
            }
            Err(error) => Self::without_output(error.to_string(), volume),
        }
    }

    /// A player with no device, for when none could be opened.
    pub fn without_output(reason: String, volume: f32) -> Self {
        Self {
            _output: None,
            player: None,
            events: None,
            unavailable: Some(reason),
            meta: HashMap::new(),
            position_ms: 0,
            volume,
        }
    }

    /// The engine's events, as they happen.
    pub fn subscription(&self) -> Subscription<EngineEvent> {
        match &self.events {
            Some(source) => Subscription::run_with(source.clone(), stream_events),
            None => Subscription::none(),
        }
    }

    pub fn unavailable(&self) -> Option<&str> {
        self.unavailable.as_deref()
    }

    fn player_mut(&mut self) -> Result<&mut Player, String> {
        match self.player.as_mut() {
            Some(player) => Ok(player),
            None => Err(format!(
                "Nothing can play: {}",
                self.unavailable.as_deref().unwrap_or("no sound output")
            )),
        }
    }

    pub fn queue(&self) -> Option<&PlayQueue<PlayerTrack>> {
        self.player.as_ref().map(Player::queue)
    }

    pub fn queued(&self, entry: EntryId) -> Option<&Queued> {
        self.meta.get(&entry)
    }

    /// The current entry and what it is.
    pub fn current(&self) -> Option<(EntryId, &Queued)> {
        let entry = self.queue()?.current()?.id;
        Some((entry, self.meta.get(&entry)?))
    }

    pub fn transport(&self) -> Transport {
        self.player
            .as_ref()
            .map_or(Transport::Stopped, |p| p.engine().transport())
    }

    pub fn is_playing(&self) -> bool {
        self.transport() == Transport::Playing
    }

    pub fn position_ms(&self) -> u64 {
        self.position_ms
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn is_shuffled(&self) -> bool {
        self.queue().is_some_and(PlayQueue::is_shuffled)
    }

    pub fn repeat(&self) -> RepeatMode {
        self.queue().map_or(RepeatMode::Off, |q| q.repeat_mode)
    }

    /// How many entries are left after the current one.
    pub fn ahead(&self) -> usize {
        self.queue().map_or(0, |q| q.up_next().len())
    }

    /// The queue as Jellyfin ids, and the current position, for saving.
    pub fn saved_form(&self) -> (Vec<String>, i64) {
        let Some(queue) = self.queue() else {
            return (Vec::new(), 0);
        };
        let ids = queue
            .entries()
            .iter()
            .filter_map(|e| self.meta.get(&e.id).map(|q| q.meta.id.clone()))
            .collect();
        (ids, queue.position().map_or(0, |p| p as i64))
    }

    /// Every Jellyfin id in the queue.
    pub fn known_ids(&self) -> HashSet<String> {
        self.meta.values().map(|q| q.meta.id.clone()).collect()
    }

    /// The ids queued after the current entry, in order.
    pub fn up_next_ids(&self) -> Vec<String> {
        self.queue()
            .map(|q| {
                q.up_next()
                    .iter()
                    .filter_map(|e| self.meta.get(&e.id).map(|m| m.meta.id.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Puts built tracks in the queue.
    pub fn enqueue(
        &mut self,
        built: Vec<BuiltTrack>,
        how: Enqueue,
        source: PlaySource,
        reasons: &HashMap<String, String>,
    ) -> Result<(), String> {
        if built.is_empty() {
            return Ok(());
        }
        let added: Vec<Queued> = built
            .iter()
            .map(|b| Queued {
                meta: b.meta.clone(),
                reason: reasons.get(&b.meta.id).cloned(),
                source,
            })
            .collect();
        let tracks: Vec<PlayerTrack> = built.into_iter().map(|b| b.track).collect();
        let unavailable = self.player.is_none();
        if unavailable {
            self.player_mut()?;
        }
        let Some(player) = self.player.as_mut() else {
            return Ok(());
        };
        let before: HashSet<EntryId> = player.queue().entries().iter().map(|e| e.id).collect();
        // Every edit inserts the new tracks in the order given and, shuffle
        // aside, keeps them together: so the entries that were not there
        // before are these tracks, in this order. Shuffling waits until they
        // have been matched up.
        let result = match how {
            Enqueue::Replace { start, shuffle } => {
                let result = player.play_tracks(tracks, start, false);
                adopt(&mut self.meta, player.queue(), &before, added);
                let result = result.and_then(|()| {
                    player.edit_queue(|q| {
                        if q.is_shuffled() != shuffle {
                            q.set_shuffled(shuffle);
                        }
                    })
                });
                self.position_ms = 0;
                result
            }
            Enqueue::Next => {
                let result = player.edit_queue(|q| q.play_next(tracks));
                adopt(&mut self.meta, player.queue(), &before, added);
                result
            }
            Enqueue::Last => {
                let result = player.edit_queue(|q| q.append(tracks));
                adopt(&mut self.meta, player.queue(), &before, added);
                result
            }
            Enqueue::Extend => {
                let result = player.edit_queue(|q| q.extend(tracks));
                adopt(&mut self.meta, player.queue(), &before, added);
                result
            }
        };
        self.prune();
        result.map_err(|e| e.to_string())
    }

    /// Restores a saved queue: loaded at `position`, at `elapsed_ms`, paused.
    pub fn restore(
        &mut self,
        built: Vec<BuiltTrack>,
        position: usize,
        elapsed_ms: u64,
    ) -> Result<(), String> {
        if built.is_empty() {
            return Ok(());
        }
        self.enqueue(
            built,
            Enqueue::Replace {
                start: position,
                shuffle: false,
            },
            PlaySource::Unknown,
            &HashMap::new(),
        )?;
        let player = self.player_mut()?;
        // `play_tracks` starts at once; the seek reloads held, at the saved
        // position, and the pause makes sure of it. The engine applies the
        // three together at its next render, so nothing is heard.
        player.seek(elapsed_ms).map_err(|e| e.to_string())?;
        player.pause().map_err(|e| e.to_string())?;
        self.position_ms = elapsed_ms;
        Ok(())
    }

    /// Feeds an engine event to the player. Call it for every event.
    pub fn handle(&mut self, event: &EngineEvent) -> Result<(), String> {
        match event {
            EngineEvent::Position { id, ms } => {
                if self.queue().and_then(|q| q.current()).map(|e| e.id) == Some(*id) {
                    self.position_ms = *ms;
                }
            }
            EngineEvent::TrackStarted { .. } => self.position_ms = 0,
            _ => {}
        }
        match self.player.as_mut() {
            Some(player) => player.handle(event).map_err(|e| e.to_string()),
            None => Ok(()),
        }
    }

    pub fn toggle(&mut self) -> Result<(), String> {
        let playing = self.is_playing();
        let player = self.player_mut()?;
        if playing {
            player.pause()
        } else {
            player.play()
        }
        .map_err(|e| e.to_string())
    }

    pub fn play(&mut self) -> Result<(), String> {
        self.player_mut()?.play().map_err(|e| e.to_string())
    }

    pub fn pause(&mut self) -> Result<(), String> {
        self.player_mut()?.pause().map_err(|e| e.to_string())
    }

    pub fn next(&mut self) -> Result<(), String> {
        self.position_ms = 0;
        self.player_mut()?.next_track().map_err(|e| e.to_string())
    }

    pub fn previous(&mut self) -> Result<(), String> {
        self.player_mut()?
            .previous_track()
            .map_err(|e| e.to_string())
    }

    pub fn seek(&mut self, ms: u64) -> Result<(), String> {
        self.position_ms = ms;
        self.player_mut()?.seek(ms).map_err(|e| e.to_string())
    }

    pub fn jump(&mut self, entry: EntryId) -> Result<(), String> {
        self.position_ms = 0;
        self.player_mut()?.jump(entry).map_err(|e| e.to_string())
    }

    pub fn set_volume(&mut self, volume: f32) -> Result<(), String> {
        self.volume = volume.clamp(0.0, 1.0);
        let volume = self.volume;
        self.player_mut()?
            .set_volume(volume)
            .map_err(|e| e.to_string())
    }

    pub fn set_settings(&mut self, settings: PlayerSettings) -> Result<(), String> {
        match self.player.as_mut() {
            Some(player) => player.set_settings(settings).map_err(|e| e.to_string()),
            None => Ok(()),
        }
    }

    pub fn set_shuffled(&mut self, shuffled: bool) -> Result<(), String> {
        self.player_mut()?
            .edit_queue(|q| q.set_shuffled(shuffled))
            .map_err(|e| e.to_string())
    }

    /// Off, then all, then one, then off.
    pub fn cycle_repeat(&mut self) -> Result<(), String> {
        let next = match self.repeat() {
            RepeatMode::Off => RepeatMode::All,
            RepeatMode::All => RepeatMode::One,
            RepeatMode::One => RepeatMode::Off,
        };
        self.player_mut()?
            .edit_queue(|q| q.repeat_mode = next)
            .map_err(|e| e.to_string())
    }

    pub fn remove(&mut self, entry: EntryId) -> Result<(), String> {
        let result = self.player_mut()?.edit_queue(|q| q.remove(entry));
        self.prune();
        result.map_err(|e| e.to_string())
    }

    /// Moves the entry at `index` to `destination` (an offset before removal).
    pub fn move_entry(&mut self, index: usize, destination: usize) -> Result<(), String> {
        self.player_mut()?
            .edit_queue(|q| q.move_entries(&[index], destination))
            .map_err(|e| e.to_string())
    }

    /// Drops everything after the current track, which keeps playing.
    pub fn clear_up_next(&mut self) -> Result<(), String> {
        let result = self.player_mut()?.edit_queue(PlayQueue::clear_up_next);
        self.prune();
        result.map_err(|e| e.to_string())
    }

    /// Stops and empties the queue: signing out.
    pub fn stop_and_clear(&mut self) {
        if let Some(player) = self.player.as_mut() {
            let _ = player.stop();
            let _ = player.edit_queue(PlayQueue::clear);
        }
        self.meta.clear();
        self.position_ms = 0;
    }

    /// Forgets what is no longer queued.
    fn prune(&mut self) {
        let Some(queue) = self.player.as_ref().map(Player::queue) else {
            return;
        };
        let live: HashSet<EntryId> = queue.entries().iter().map(|e| e.id).collect();
        self.meta.retain(|id, _| live.contains(id));
    }
}

/// Matches the entries a queue edit created with the tracks it was given.
fn adopt<T: Clone>(
    meta: &mut HashMap<EntryId, Queued>,
    queue: &PlayQueue<T>,
    before: &HashSet<EntryId>,
    added: Vec<Queued>,
) {
    let fresh = queue
        .entries()
        .iter()
        .map(|e| e.id)
        .filter(|id| !before.contains(id));
    for (id, queued) in fresh.zip(added) {
        meta.insert(id, queued);
    }
}

/// The engine's blocking channel as a stream: a thread waits on it and
/// forwards. The thread ends when the engine (and so the sender) goes.
fn stream_events(source: &EventSource) -> impl Stream<Item = EngineEvent> + use<> {
    let receiver = source
        .receiver
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    let (mut sender, stream) = iced::futures::channel::mpsc::channel(256);
    if let Some(receiver) = receiver {
        let _ = std::thread::Builder::new()
            .name("aoide-events".into())
            .spawn(move || {
                while let Ok(event) = receiver.recv() {
                    if iced::futures::executor::block_on(sender.send(event)).is_err() {
                        break;
                    }
                }
            });
    }
    stream.boxed()
}

/// A random index below `len`, for Shuffle's first track.
pub fn random_index(len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let seed =
        std::collections::hash_map::RandomState::new().hash_one(std::time::SystemTime::now());
    (seed % len as u64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &str) -> Queued {
        Queued {
            meta: TrackMeta {
                id: id.into(),
                title: id.into(),
                artist: String::new(),
                album_artist: None,
                album: String::new(),
                album_id: None,
                artist_id: None,
                artwork: None,
                duration_ms: None,
                track_number: None,
                disc_number: None,
                year: None,
                genres: Vec::new(),
                favourite: false,
            },
            reason: None,
            source: PlaySource::Album,
        }
    }

    fn ids(
        queue: &PlayQueue<&'static str>,
        meta: &HashMap<EntryId, Queued>,
    ) -> Vec<(String, String)> {
        queue
            .entries()
            .iter()
            .map(|e| (e.track.to_owned(), meta[&e.id].meta.id.clone()))
            .collect()
    }

    #[test]
    fn new_entries_are_matched_to_their_tracks_after_every_kind_of_edit() {
        let mut queue: PlayQueue<&'static str> = PlayQueue::new();
        let mut map = HashMap::new();

        let before = HashSet::new();
        queue.load(vec!["a", "b", "c"], 1, false);
        adopt(
            &mut map,
            &queue,
            &before,
            vec![meta("a"), meta("b"), meta("c")],
        );

        let before: HashSet<EntryId> = queue.entries().iter().map(|e| e.id).collect();
        queue.play_next(vec!["x", "y"]);
        adopt(&mut map, &queue, &before, vec![meta("x"), meta("y")]);

        let before: HashSet<EntryId> = queue.entries().iter().map(|e| e.id).collect();
        queue.append(vec!["z"]);
        adopt(&mut map, &queue, &before, vec![meta("z")]);

        let before: HashSet<EntryId> = queue.entries().iter().map(|e| e.id).collect();
        queue.extend(vec!["w"]);
        adopt(&mut map, &queue, &before, vec![meta("w")]);

        // Shuffling afterwards moves entries, never their identity.
        queue.set_shuffled(true);
        for (track, id) in ids(&queue, &map) {
            assert_eq!(track, id);
        }
        assert_eq!(map.len(), 7);
    }

    #[test]
    fn replacing_keeps_hand_queued_tracks_and_matches_the_new_ones() {
        let mut queue: PlayQueue<&'static str> = PlayQueue::new();
        let mut map = HashMap::new();
        queue.load(vec!["a", "b"], 0, false);
        adopt(
            &mut map,
            &queue,
            &HashSet::new(),
            vec![meta("a"), meta("b")],
        );
        let before: HashSet<EntryId> = queue.entries().iter().map(|e| e.id).collect();
        queue.play_next(vec!["x"]);
        adopt(&mut map, &queue, &before, vec![meta("x")]);

        let before: HashSet<EntryId> = queue.entries().iter().map(|e| e.id).collect();
        queue.load(vec!["p", "q", "r"], 1, false);
        adopt(
            &mut map,
            &queue,
            &before,
            vec![meta("p"), meta("q"), meta("r")],
        );
        for (track, id) in ids(&queue, &map) {
            assert_eq!(track, id);
        }
    }

    #[test]
    fn a_random_index_is_in_range() {
        for len in [1, 2, 7, 100] {
            assert!(random_index(len) < len);
        }
        assert_eq!(random_index(0), 0);
    }

    #[test]
    fn without_a_device_actions_say_why() {
        let mut playback = Playback::without_output("no output device".into(), 1.0);
        let error = playback.toggle().unwrap_err();
        assert!(error.contains("no output device"), "{error}");
        assert!(playback.current().is_none());
        assert_eq!(playback.saved_form(), (Vec::new(), 0));
    }
}
