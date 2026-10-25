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
pub mod sleep_timer;
pub mod sound;
pub mod tracks;

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hash, Hasher};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use aoide_audio::{
    EngineEvent, EntryId, Output, PlayQueue, Player, PlayerSettings, PlayerTrack, RepeatMode,
    SoundBounds, Transport,
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
    /// What each queued track is, by the key it was queued with.
    meta: HashMap<String, Queued>,
    /// Makes every queued track's key unique, the same song twice included.
    next_key: u64,
    position_ms: u64,
    volume: f32,
}

impl Playback {
    /// Opens the chosen output device (the default when it can't be). Without
    /// one the app still runs; it says why nothing plays.
    pub fn start(volume: f32, device: aoide_audio::DeviceChoice) -> Self {
        match Output::start_on(device) {
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
                    next_key: 0,
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
            next_key: 0,
            position_ms: 0,
            volume,
        }
    }

    /// A player on an engine the caller renders, for tests.
    #[cfg(test)]
    pub fn on_engine(handle: aoide_audio::EngineHandle) -> Self {
        Self {
            _output: None,
            player: Some(Player::new(handle)),
            events: None,
            unavailable: None,
            meta: HashMap::new(),
            next_key: 0,
            position_ms: 0,
            volume: 1.0,
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

    /// What the entry with this id is.
    pub fn queued(&self, entry: EntryId) -> Option<&Queued> {
        let entry = self.queue()?.entries().iter().find(|e| e.id == entry)?;
        self.meta.get(&entry.track.key)
    }

    /// What the entry at this place in the queue is.
    pub fn queued_at(&self, index: usize) -> Option<&Queued> {
        let entry = self.queue()?.entries().get(index)?;
        self.meta.get(&entry.track.key)
    }

    /// The current entry and what it is.
    pub fn current(&self) -> Option<(EntryId, &Queued)> {
        let entry = self.queue()?.current()?;
        Some((entry.id, self.meta.get(&entry.track.key)?))
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
            .filter_map(|e| self.meta.get(&e.track.key).map(|q| q.meta.id.clone()))
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
                    .filter_map(|e| self.meta.get(&e.track.key).map(|m| m.meta.id.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every queued Jellyfin id, in queue order, each once.
    pub fn queued_ids(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.queue()
            .map(|q| {
                q.entries()
                    .iter()
                    .filter_map(|e| self.meta.get(&e.track.key))
                    .map(|m| m.meta.id.clone())
                    .filter(|id| seen.insert(id.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Hands late measurements to the queued entries of these tracks:
    /// `measured` says what a track should now play with.
    pub fn apply_measurements(
        &mut self,
        ids: &HashSet<String>,
        measured: impl Fn(&str) -> (Option<SoundBounds>, Option<f64>),
    ) -> Result<(), String> {
        let Some(player) = self.player.as_mut() else {
            return Ok(());
        };
        let changes: Vec<(EntryId, Option<SoundBounds>, Option<f64>)> = player
            .queue()
            .entries()
            .iter()
            .filter_map(|e| {
                let id = &self.meta.get(&e.track.key)?.meta.id;
                if !ids.contains(id) {
                    return None;
                }
                let (sound, gain) = measured(id);
                let unchanged = e.track.mix.sound == sound && e.track.gain_db == gain;
                (!unchanged).then_some((e.id, sound, gain))
            })
            .collect();
        for (entry, sound, gain) in changes {
            player
                .set_measurements(entry, sound, gain)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
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
        self.player_mut()?;
        let tracks = keyed(&mut self.next_key, &mut self.meta, built, source, reasons);
        let Some(player) = self.player.as_mut() else {
            return Ok(());
        };
        let result = match how {
            Enqueue::Replace { start, shuffle } => {
                self.position_ms = 0;
                player.play_tracks(tracks, start, shuffle)
            }
            Enqueue::Next => player.edit_queue(|q| q.play_next(tracks)),
            Enqueue::Last => player.edit_queue(|q| q.append(tracks)),
            Enqueue::Extend => player.edit_queue(|q| q.extend(tracks)),
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
        self.player_mut()?;
        let tracks = keyed(
            &mut self.next_key,
            &mut self.meta,
            built,
            PlaySource::Unknown,
            &HashMap::new(),
        );
        let Some(player) = self.player.as_mut() else {
            return Ok(());
        };
        let result = player.load_tracks(tracks, position, false, elapsed_ms);
        self.position_ms = elapsed_ms;
        self.prune();
        result.map_err(|e| e.to_string())
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
        let live: HashSet<&str> = queue
            .entries()
            .iter()
            .map(|e| e.track.key.as_str())
            .collect();
        self.meta.retain(|key, _| live.contains(key.as_str()));
    }
}

/// Gives each built track a key no other queued track has, and records what
/// it is under that key.
fn keyed(
    next_key: &mut u64,
    meta: &mut HashMap<String, Queued>,
    built: Vec<BuiltTrack>,
    source: PlaySource,
    reasons: &HashMap<String, String>,
) -> Vec<PlayerTrack> {
    built
        .into_iter()
        .map(|b| {
            *next_key += 1;
            let mut track = b.track;
            track.key = format!("{}#{next_key}", b.meta.id);
            meta.insert(
                track.key.clone(),
                Queued {
                    reason: reasons.get(&b.meta.id).cloned(),
                    meta: b.meta,
                    source,
                },
            );
            track
        })
        .collect()
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

    fn built(id: &str) -> BuiltTrack {
        let url =
            aoide_jellyfin::Url::parse(&format!("https://music.example.net/Audio/{id}")).unwrap();
        BuiltTrack {
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
            track: PlayerTrack {
                key: id.into(),
                source: aoide_audio::MediaSource::http(url.as_str()),
                mix: aoide_audio::MixTrack::default(),
                gain_db: None,
            },
            url,
        }
    }

    #[test]
    fn every_queued_track_gets_its_own_key_even_the_same_song_twice() {
        let mut next = 0;
        let mut meta = HashMap::new();
        let reasons = HashMap::from([("b".to_owned(), "Like what you played".to_owned())]);
        let tracks = keyed(
            &mut next,
            &mut meta,
            vec![built("a"), built("b"), built("a")],
            PlaySource::Album,
            &reasons,
        );
        let keys: HashSet<&str> = tracks.iter().map(|t| t.key.as_str()).collect();
        assert_eq!(keys.len(), 3);
        assert_eq!(meta.len(), 3);
        for track in &tracks {
            let id = &meta[&track.key].meta.id;
            assert!(track.key.starts_with(&format!("{id}#")), "{}", track.key);
        }
        assert_eq!(
            meta[&tracks[1].key].reason.as_deref(),
            Some("Like what you played")
        );
        assert_eq!(meta[&tracks[0].key].reason, None);
        let more = keyed(
            &mut next,
            &mut meta,
            vec![built("a")],
            PlaySource::Album,
            &reasons,
        );
        assert!(!keys.contains(more[0].key.as_str()));
    }

    #[test]
    fn keys_find_their_tracks_after_any_edit() {
        let mut next = 0;
        let mut meta = HashMap::new();
        let mut queue: PlayQueue<PlayerTrack> = PlayQueue::new();
        let none = HashMap::new();
        let album = keyed(
            &mut next,
            &mut meta,
            ["a", "b", "c", "d"].map(built).to_vec(),
            PlaySource::Album,
            &none,
        );
        queue.load(album, 1, true);
        queue.play_next(keyed(
            &mut next,
            &mut meta,
            vec![built("x")],
            PlaySource::Unknown,
            &none,
        ));
        queue.append(keyed(
            &mut next,
            &mut meta,
            vec![built("y")],
            PlaySource::Unknown,
            &none,
        ));
        queue.extend(keyed(
            &mut next,
            &mut meta,
            vec![built("z")],
            PlaySource::Unknown,
            &none,
        ));
        queue.set_shuffled(false);
        queue.move_entries(&[0], 3);
        for entry in queue.entries() {
            let queued = &meta[&entry.track.key];
            assert!(entry.track.key.starts_with(&format!("{}#", queued.meta.id)));
        }
        assert_eq!(queue.entries().len(), 7);
    }

    #[test]
    fn late_measurements_reach_every_queued_copy_of_their_tracks_only() {
        let (_engine, handle, _events) = aoide_audio::Engine::new(aoide_audio::EngineConfig {
            sample_rate: 48_000,
            channels: 2,
            decode: aoide_audio::DecodeMode::Inline,
        });
        let mut playback = Playback::on_engine(handle);
        playback
            .enqueue(
                ["a", "b", "a", "c"].map(built).to_vec(),
                Enqueue::Replace {
                    start: 0,
                    shuffle: false,
                },
                PlaySource::Album,
                &HashMap::new(),
            )
            .unwrap();
        assert_eq!(playback.queued_ids(), ["a", "b", "c"]);
        let bounds = Some(SoundBounds::new(300, 9_000));
        let ids = HashSet::from(["a".to_owned(), "c".to_owned()]);
        playback
            .apply_measurements(&ids, |id| {
                if id == "a" {
                    (bounds, Some(-3.0))
                } else {
                    (None, None)
                }
            })
            .unwrap();
        let heard: Vec<(String, Option<SoundBounds>, Option<f64>)> = playback
            .queue()
            .unwrap()
            .entries()
            .iter()
            .map(|e| {
                (
                    playback.queued(e.id).unwrap().meta.id.clone(),
                    e.track.mix.sound,
                    e.track.gain_db,
                )
            })
            .collect();
        assert_eq!(
            heard,
            [
                ("a".into(), bounds, Some(-3.0)),
                ("b".into(), None, None),
                ("a".into(), bounds, Some(-3.0)),
                ("c".into(), None, None),
            ]
        );
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
