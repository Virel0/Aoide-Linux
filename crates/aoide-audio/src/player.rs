//! The queue, the crossfade planner and the transport rules, on top of the
//! engine.
//!
//! The player has no thread of its own. The application forwards every
//! [`EngineEvent`] it receives to [`Player::handle`], which is how the queue
//! learns that a track boundary has been heard and the next track gets
//! chained. Everything else returns at once.

use crate::decode::MediaSource;
use crate::engine::{EngineEvent, EngineHandle, Transport};
use crate::error::Result;
use crate::mix::{MixTrack, MixTransition, SoundBounds, plan_transition};
use crate::pipeline::TrackSpec;
use crate::queue::{AdvanceOutcome, EntryId, PlayQueue, QueueEntry};

/// Pressing previous within this much of the start goes to the previous
/// track; later, it restarts the current one. Matches the iOS app.
pub const RESTART_THRESHOLD_MS: u64 = 3_000;

/// One playable track and what the planner knows about it.
#[derive(Clone, Debug)]
pub struct PlayerTrack {
    /// The caller's own name for this track, opaque to the player: the
    /// Jellyfin id and a per-entry counter, say. It goes wherever the track
    /// goes (shuffle, moves, Play Next, a replaced queue keeping its lane), so
    /// the caller finds what it knows about an entry from `entry.track.key`
    /// rather than by working out which entries an edit created.
    pub key: String,
    /// Where its bytes come from.
    pub source: MediaSource,
    /// Album, track number, length, sound bounds and tempo, for the planner;
    /// its `sound` is also what the engine plays.
    pub mix: MixTrack,
    /// Loudness levelling, from [`crate::loudness::gain_db`]. Applied unless
    /// [`Player::set_level_volume`] turned Level volume off, and standing
    /// down for a file with ReplayGain tags of its own once it was turned on.
    pub gain_db: Option<f64>,
}

/// The player's own settings, apart from the queue and the engine's sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerSettings {
    /// The Crossfade toggle, off by default.
    pub crossfade: bool,
    /// Keep album runs gapless whatever Crossfade says.
    pub album_lock: bool,
}

impl Default for PlayerSettings {
    fn default() -> Self {
        Self {
            crossfade: false,
            album_lock: true,
        }
    }
}

/// The queue, the crossfade planner and the transport rules over an [`EngineHandle`].
/// Feed it every [`EngineEvent`] with [`Self::handle`].
pub struct Player {
    engine: EngineHandle,
    queue: PlayQueue<PlayerTrack>,
    settings: PlayerSettings,
    /// What has been handed to the engine to follow the current track.
    planned: Option<(EntryId, MixTransition)>,
    failures: usize,
    /// The Level volume switch; `None` until the app says, which applies
    /// each track's `gain_db` and ignores tags, as before the switch was
    /// the player's.
    level_volume: Option<bool>,
}

impl Player {
    /// A player with an empty queue, driving `engine`.
    pub fn new(engine: EngineHandle) -> Self {
        Self {
            engine,
            queue: PlayQueue::new(),
            settings: PlayerSettings::default(),
            planned: None,
            failures: 0,
            level_volume: None,
        }
    }

    /// The engine it drives.
    pub fn engine(&self) -> &EngineHandle {
        &self.engine
    }

    /// The queue as it stands.
    pub fn queue(&self) -> &PlayQueue<PlayerTrack> {
        &self.queue
    }

    /// Edits the queue (play next, play last, shuffle, remove, reorder...),
    /// then re-plans what follows the current track.
    ///
    /// # Errors
    ///
    /// As [`EngineHandle::load`]: the engine has gone, or no decode thread could be started.
    pub fn edit_queue(&mut self, edit: impl FnOnce(&mut PlayQueue<PlayerTrack>)) -> Result<()> {
        let before = self.queue.current().map(|entry| entry.id);
        edit(&mut self.queue);
        if self.queue.current().map(|entry| entry.id) != before {
            return self.start_current(true);
        }
        self.replan()
    }

    /// The settings as they stand.
    pub fn settings(&self) -> PlayerSettings {
        self.settings
    }

    /// Changes the settings and plans what follows the current track again.
    ///
    /// # Errors
    ///
    /// As [`EngineHandle::load`]: the engine has gone, or no decode thread could be started.
    pub fn set_settings(&mut self, settings: PlayerSettings) -> Result<()> {
        self.settings = settings;
        self.replan()
    }

    /// The Level volume switch, over both of its sources: a file's own
    /// ReplayGain tags and the sidecar's measurement in each track's
    /// `gain_db`. On, a file with tags plays by them and the measurement
    /// stands down for it (the desktop's rule); a file without plays by the
    /// measurement. Off, neither applies. Like [`Player::set_measurements`],
    /// the current track keeps what it was loaded with until it is loaded
    /// again; what follows it is planned again at once.
    ///
    /// # Errors
    ///
    /// As [`EngineHandle::load`]: the engine has gone, or no decode thread could be started.
    pub fn set_level_volume(&mut self, on: bool) -> Result<()> {
        if self.level_volume == Some(on) {
            return Ok(());
        }
        self.level_volume = Some(on);
        self.planned = None;
        self.replan()
    }

    /// Replaces the queue with `tracks` and plays from `index`.
    ///
    /// # Errors
    ///
    /// As [`EngineHandle::load`]: the engine has gone, or no decode thread could be started.
    pub fn play_tracks(
        &mut self,
        tracks: Vec<PlayerTrack>,
        index: usize,
        shuffled: bool,
    ) -> Result<()> {
        self.queue.load(tracks, index, shuffled);
        self.failures = 0;
        self.start_current(true)
    }

    /// Replaces the queue with `tracks` and holds at `index`, `start_ms` into
    /// it, paused: nothing sounds until [`Player::play`]. What follows is
    /// planned as usual. For putting back a queue saved at the last quit.
    ///
    /// # Errors
    ///
    /// As [`EngineHandle::load`]: the engine has gone, or no decode thread could be started.
    pub fn load_tracks(
        &mut self,
        tracks: Vec<PlayerTrack>,
        index: usize,
        shuffled: bool,
        start_ms: u64,
    ) -> Result<()> {
        self.queue.load(tracks, index, shuffled);
        self.failures = 0;
        self.load_current(start_ms, false)
    }

    /// Plays: resumes a paused track, or starts the current entry again after the queue
    /// ended or was stopped.
    ///
    /// # Errors
    ///
    /// As [`EngineHandle::load`]: the engine has gone, or no decode thread could be started.
    pub fn play(&mut self) -> Result<()> {
        match self.engine.transport() {
            Transport::Stopped if self.queue.current().is_some() => self.start_current(true),
            _ => self.engine.play(),
        }
    }

    /// Pauses the engine.
    ///
    /// # Errors
    ///
    /// [`Error::Output`](crate::Error::Output) when the engine has been dropped.
    pub fn pause(&mut self) -> Result<()> {
        self.engine.pause()
    }

    /// Stops the engine and forgets what was planned next; the queue stays.
    ///
    /// # Errors
    ///
    /// [`Error::Output`](crate::Error::Output) when the engine has been dropped.
    pub fn stop(&mut self) -> Result<()> {
        self.planned = None;
        self.engine.stop()
    }

    /// Master volume, 0 to 1.
    ///
    /// # Errors
    ///
    /// [`Error::Output`](crate::Error::Output) when the engine has been dropped.
    pub fn set_volume(&mut self, volume: f32) -> Result<()> {
        self.engine.set_volume(volume)
    }

    /// The heard position in the current track, in milliseconds into the file.
    pub fn position_ms(&self) -> u64 {
        self.engine.position_ms()
    }

    /// Seeks the current track to `ms` milliseconds into the file, playing on if it was
    /// playing. Nothing queued, nothing happens.
    ///
    /// # Errors
    ///
    /// As [`EngineHandle::load`]: the engine has gone, or no decode thread could be started.
    pub fn seek(&mut self, ms: u64) -> Result<()> {
        let Some(entry) = self.queue.current().cloned() else {
            return Ok(());
        };
        let playing = self.engine.transport() == Transport::Playing;
        self.engine.load(self.spec(&entry), ms, playing)?;
        self.planned = None;
        self.replan()
    }

    /// Moves to the next entry and plays it; at the end of the queue, nothing happens.
    ///
    /// # Errors
    ///
    /// As [`EngineHandle::load`]: the engine has gone, or no decode thread could be started.
    pub fn next_track(&mut self) -> Result<()> {
        match self.queue.skip_to_next() {
            AdvanceOutcome::Ended => Ok(()),
            _ => self.start_current(true),
        }
    }

    /// Restarts the current track when more than a few seconds in, and goes
    /// back one otherwise (restarting the first track, which has nothing
    /// before it).
    ///
    /// # Errors
    ///
    /// As [`EngineHandle::load`]: the engine has gone, or no decode thread could be started.
    pub fn previous_track(&mut self) -> Result<()> {
        if self.position_ms() > RESTART_THRESHOLD_MS {
            return self.seek(0);
        }
        match self.queue.skip_to_previous() {
            AdvanceOutcome::Ended => self.seek(0),
            _ => self.start_current(true),
        }
    }

    /// Sets an entry's sound bounds (Skip silence) and loudness gain after it
    /// was queued, for measurements that arrive late. If it is the track
    /// planned next, or the current one (whose end decides the handover),
    /// what follows is planned again. The current track itself keeps what it
    /// was loaded with until it is loaded again, by a seek or a repeat, so
    /// nothing audible jumps. Returns whether the entry is queued.
    ///
    /// # Errors
    ///
    /// As [`EngineHandle::load`]: the engine has gone, or no decode thread could be started.
    pub fn set_measurements(
        &mut self,
        entry: EntryId,
        sound: Option<SoundBounds>,
        gain_db: Option<f64>,
    ) -> Result<bool> {
        let found = self.queue.edit_track(entry, |track| {
            track.mix.sound = sound;
            track.gain_db = gain_db;
        });
        if !found {
            return Ok(false);
        }
        let current = self.queue.current().map(|entry| entry.id);
        let planned = self.planned.map(|(id, _)| id);
        if current == Some(entry) || planned == Some(entry) {
            self.planned = None;
            self.replan()?;
        }
        Ok(true)
    }

    /// The entry holding the track with this key.
    pub fn entry_by_key(&self, key: &str) -> Option<&QueueEntry<PlayerTrack>> {
        self.queue
            .entries()
            .iter()
            .find(|entry| entry.track.key == key)
    }

    /// Plays `entry`, if it is in the queue.
    ///
    /// # Errors
    ///
    /// As [`EngineHandle::load`]: the engine has gone, or no decode thread could be started.
    pub fn jump(&mut self, entry: EntryId) -> Result<()> {
        if self.queue.jump(entry) {
            self.start_current(true)
        } else {
            Ok(())
        }
    }

    /// Feeds an engine event to the queue. Call it for every event.
    ///
    /// # Errors
    ///
    /// As [`EngineHandle::load`]: the engine has gone, or no decode thread could be started.
    pub fn handle(&mut self, event: &EngineEvent) -> Result<()> {
        match event {
            EngineEvent::TrackStarted { id } => {
                if self.queue.current().is_some_and(|entry| entry.id == *id)
                    && self.planned.is_none_or(|(planned, _)| planned != *id)
                {
                    return Ok(());
                }
                if self.planned.is_some_and(|(planned, _)| planned == *id) {
                    self.queue.advance_automatically();
                } else if !self.queue.jump(*id) {
                    return Ok(());
                }
                self.failures = 0;
                self.planned = None;
                self.replan()
            }
            EngineEvent::Error { .. } => {
                self.failures += 1;
                Ok(())
            }
            EngineEvent::Finished => {
                // The chain did not carry on: the next track was not ready in
                // time, or the current one failed. Move on, but not round and
                // round a queue where nothing plays.
                self.planned = None;
                if self.failures > self.queue.entries().len() {
                    return Ok(());
                }
                match self.queue.advance_automatically() {
                    AdvanceOutcome::Ended => Ok(()),
                    _ => self.start_current(true),
                }
            }
            _ => Ok(()),
        }
    }

    fn start_current(&mut self, play: bool) -> Result<()> {
        self.load_current(0, play)
    }

    fn load_current(&mut self, start_ms: u64, play: bool) -> Result<()> {
        self.planned = None;
        let Some(entry) = self.queue.current().cloned() else {
            return self.engine.stop();
        };
        self.engine.load(self.spec(&entry), start_ms, play)?;
        self.replan()
    }

    /// Tells the engine what follows the current track, and how.
    fn replan(&mut self) -> Result<()> {
        let window = self.queue.render_window(2);
        let (Some(current), Some(next)) = (window.first(), window.get(1)) else {
            if self.planned.take().is_some() {
                self.engine.set_next(None)?;
            }
            return Ok(());
        };
        let transition = plan_transition(
            &current.track.mix,
            &next.track.mix,
            self.settings.crossfade,
            self.settings.album_lock,
        );
        if self.planned == Some((next.entry_id, transition)) {
            return Ok(());
        }
        let entry = QueueEntry {
            id: next.entry_id,
            track: next.track.clone(),
            is_manual: false,
        };
        self.engine
            .set_next(Some((self.spec(&entry), transition)))?;
        self.planned = Some((next.entry_id, transition));
        Ok(())
    }

    /// What has been planned after the current track.
    pub fn planned(&self) -> Option<(EntryId, MixTransition)> {
        self.planned
    }

    fn spec(&self, entry: &QueueEntry<PlayerTrack>) -> TrackSpec {
        let gain_db = match self.level_volume {
            Some(false) => None,
            _ => entry.track.gain_db,
        };
        TrackSpec::new(entry.id, entry.track.source.clone())
            .with_bounds(entry.track.mix.sound)
            .with_gain_db(gain_db)
            .with_replay_gain(self.level_volume == Some(true))
    }
}
