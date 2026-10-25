//! Handing playback between devices: publishing this device's queue, and picking up
//! another's.
//!
//! **Publishing.** [`QueueBroadcast`] is the one thing the player tells about every
//! change — a new queue, a reorder, the next track, a seek — wherever in the app it
//! happened. It waits a moment for a burst of changes to settle and then writes one
//! `queueState` row through [`Store::save_queue_if_changed`]. It is not tied to any
//! screen: a queue that is only written while a particular page is open is wrong
//! exactly when somebody reaches for their other device.
//!
//! **Reading.** [`ResumableQueue`] is another device's queue as something to offer.
//! The sidecar's `GET /aoide/queue` is the better source (its freshness is the
//! server's clock); this module holds the fallback built from the rows the op log
//! already brought, [`Store::local_resumable_queues`].
//!
//! **Picking up.** [`ResumableQueue::pick_up`] keeps the whole list, earlier tracks
//! included, and starts at the track the other device was on — the iOS
//! `ResumeGrid.pickUp` / `QueueRestorer` and the desktop `pick-up.ts` rule.

use crate::Millis;
use crate::error::Result;
use crate::models::QueueState;
use crate::queue::QueuePolicy;
use crate::store::Store;

// MARK: - Reading

/// Whose clock a [`ResumableQueue`]'s age was measured on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueueFreshness {
    /// The server's (`ageSeconds` from `GET /aoide/queue`). Trustworthy across devices.
    Server,
    /// The writing device's own `updatedAt`, read against this device's clock — the
    /// only reading available offline. A device with a wrong clock can look fresher
    /// than it is.
    Writer,
}

/// Another device's queue, as something to offer on Home or in a "Pick up" menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumableQueue {
    pub device_id: String,
    /// What to call it: "Pick up from {device_name}".
    pub device_name: String,
    /// In the other device's order. Never empty.
    pub track_ids: Vec<String>,
    /// Index into `track_ids` of the track the other device was on.
    pub position: usize,
    /// How far into that track it was.
    pub elapsed_ms: i64,
    /// How long ago the row was written, as of `observed_at`.
    pub age_ms: Millis,
    /// This device's clock when `age_ms` was measured. Ages are only meaningful
    /// relative to it, so anything comparing them to local timestamps starts here.
    pub observed_at: Millis,
    pub freshness: QueueFreshness,
}

impl ResumableQueue {
    /// Another device's stored row, aged on the writer's clock. `None` for an empty
    /// queue (a signed-out device keeps one).
    pub fn from_local(state: &QueueState, now: Millis) -> Option<Self> {
        let track_ids = state.track_id_list();
        if track_ids.is_empty() {
            return None;
        }
        let position = clamp_position(state.position, track_ids.len());
        Some(Self {
            device_id: state.device_id.clone(),
            device_name: state.device_name.clone(),
            track_ids,
            position,
            elapsed_ms: state.elapsed_ms.max(0),
            age_ms: now.saturating_sub(state.updated_at).max(0),
            observed_at: now,
            freshness: QueueFreshness::Writer,
        })
    }

    /// When the other device was last playing, on **this** device's clock.
    pub fn played_at(&self) -> Millis {
        self.observed_at.saturating_sub(self.age_ms)
    }

    /// The track the other device was on.
    pub fn current_track_id(&self) -> Option<&str> {
        self.track_ids.get(self.position).map(String::as_str)
    }

    /// The tile's title.
    pub fn title(&self) -> String {
        format!("Pick up from {}", self.device_name)
    }

    /// The queue to play here, given which of its ids this server can still play.
    ///
    /// Every playable track is kept in the other device's order — earlier ones too,
    /// so Previous still reaches the start of what they were listening to — and
    /// playback starts at the track that device was on, found **by id**, because a
    /// track this library no longer has would otherwise shift every index after it.
    /// If that very track is gone, it starts at the next playable one after it (or
    /// the last before it, when nothing follows) from the top, since the elapsed time
    /// belonged to the missing track. `None` when nothing is playable.
    pub fn pick_up(&self, playable: impl Fn(&str) -> bool) -> Option<PickUp> {
        let mut track_ids = Vec::with_capacity(self.track_ids.len());
        let mut start = None;
        let mut start_is_named_track = false;
        for (index, id) in self.track_ids.iter().enumerate() {
            if !playable(id) {
                continue;
            }
            if start.is_none() && index >= self.position {
                start = Some(track_ids.len());
                start_is_named_track = index == self.position;
            }
            track_ids.push(id.clone());
        }
        if track_ids.is_empty() {
            return None;
        }
        let start_index = start.unwrap_or(track_ids.len() - 1);
        Some(PickUp {
            track_ids,
            start_index,
            elapsed_ms: if start_is_named_track {
                self.elapsed_ms
            } else {
                0
            },
        })
    }
}

/// What [`ResumableQueue::pick_up`] decided: play `track_ids`, starting at
/// `start_index`, seeked to `elapsed_ms`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickUp {
    pub track_ids: Vec<String>,
    pub start_index: usize,
    pub elapsed_ms: i64,
}

pub(crate) fn clamp_position(position: i64, len: usize) -> usize {
    usize::try_from(position.max(0))
        .unwrap_or(usize::MAX)
        .min(len.saturating_sub(1))
}

impl Store {
    /// Other devices' queues from the rows sync already brought, newest first by the
    /// writers' clocks, within [`QueuePolicy::RESUME_WINDOW`], empty ones left out.
    /// The fallback for when `GET /aoide/queue` cannot be reached.
    pub fn local_resumable_queues(&self, now: Millis) -> Result<Vec<ResumableQueue>> {
        Ok(self
            .resumable_queues(now, QueuePolicy::RESUME_WINDOW)?
            .iter()
            .filter_map(|state| ResumableQueue::from_local(state, now))
            .collect())
    }
}

// MARK: - Publishing

/// What the player is doing, as the queue row records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueSnapshot {
    /// This computer's name, as other devices will show it.
    pub device_name: String,
    /// The whole queue in play order, played tracks included. Windowed to
    /// [`QueuePolicy::MAX_TRACKS`] when written.
    pub track_ids: Vec<String>,
    /// Index of the current track in `track_ids`.
    pub position: i64,
    pub elapsed_ms: i64,
}

impl QueueSnapshot {
    /// Elapsed time is not identity: a write per position tick would be a row every
    /// second, and resuming a few seconds early is invisible.
    fn same_queue(&self, other: &QueueSnapshot) -> bool {
        self.track_ids == other.track_ids
            && self.position == other.position
            && self.device_name == other.device_name
    }
}

/// Publishes this device's queue as `queueState`, debounced.
///
/// Call [`note`](Self::note) on **every** queue or track change and on progress ticks,
/// from wherever playback state lives — never from a screen. Then call
/// [`flush_due`](Self::flush_due) when [`due_at`](Self::due_at) passes (a timer, or the
/// next event loop tick), and [`flush_now`](Self::flush_now) before quitting or
/// signing out. A flush writes the row and its op in one transaction; the op reaches
/// the server with the next push (see `SyncEngine::publish_queue` in aoide-sidecar
/// for a push of just this row).
///
/// A queue change waits [`DEBOUNCE_MS`](Self::DEBOUNCE_MS) for the burst to settle —
/// five skips in a row are one write — but never more than
/// [`MAX_WAIT_MS`](Self::MAX_WAIT_MS) from the first change, so a stream of edits
/// cannot starve it. An elapsed-only change rides along; the store skips a write that
/// moved elapsed by less than [`QueuePolicy::ELAPSED_THRESHOLD_MS`].
#[derive(Debug, Clone, Default)]
pub struct QueueBroadcast {
    pending: Option<QueueSnapshot>,
    first_noted_at: Option<Millis>,
    due_at: Option<Millis>,
}

impl QueueBroadcast {
    pub const DEBOUNCE_MS: Millis = 1_500;
    pub const MAX_WAIT_MS: Millis = 10_000;

    pub fn new() -> Self {
        Self::default()
    }

    /// Records the player's state as of `now`.
    pub fn note(&mut self, snapshot: QueueSnapshot, now: Millis) {
        let changed = self
            .pending
            .as_ref()
            .is_none_or(|pending| !pending.same_queue(&snapshot));
        let first = *self.first_noted_at.get_or_insert(now);
        if changed || self.due_at.is_none() {
            self.due_at = Some((now + Self::DEBOUNCE_MS).min(first + Self::MAX_WAIT_MS));
        }
        self.pending = Some(snapshot);
    }

    /// When the waiting snapshot should be written; `None` when nothing waits.
    pub fn due_at(&self) -> Option<Millis> {
        self.due_at
    }

    /// Writes the waiting snapshot if its time has come. Reports whether a row was
    /// written (an unchanged queue writes nothing).
    pub fn flush_due(&mut self, store: &Store, now: Millis) -> Result<bool> {
        match self.due_at {
            Some(due) if due <= now => self.flush_now(store, now),
            _ => Ok(false),
        }
    }

    /// Writes the waiting snapshot now, whatever the debounce says.
    pub fn flush_now(&mut self, store: &Store, now: Millis) -> Result<bool> {
        let Some(snapshot) = self.pending.take() else {
            return Ok(false);
        };
        let written = store.save_queue_if_changed(
            &snapshot.device_name,
            &snapshot.track_ids,
            snapshot.position,
            snapshot.elapsed_ms,
            now,
        );
        if written.is_err() {
            // Kept for the next try rather than lost with the failed write.
            self.pending = Some(snapshot);
            self.due_at = Some(now + Self::DEBOUNCE_MS);
        } else {
            self.due_at = None;
            self.first_noted_at = None;
        }
        written
    }
}
