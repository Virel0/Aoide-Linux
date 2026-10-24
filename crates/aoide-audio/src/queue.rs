//! The queue model, with no dependency on a player.
//!
//! A port of `PlayQueue.swift` in the iOS app: shuffle keeping the current song,
//! repeat-one applying to a natural ending but not to a skip, the manual lane
//! (Play Next and Play Last) that plays before the album resumes, and the render
//! window the engine reads to decode ahead.

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicU64, Ordering};

/// One slot's identity. A playlist may hold the same song twice, and "remove
/// this one" needs to tell the two slots apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntryId(pub u64);

impl EntryId {
    /// A fresh identity, unique within this process.
    pub fn fresh() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        EntryId(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// One slot in the queue.
#[derive(Clone, Debug, PartialEq)]
pub struct QueueEntry<T> {
    pub id: EntryId,
    pub track: T,
    /// Put here on purpose, by Play Next or Play Last, rather than arriving as
    /// part of whatever album or playlist is running.
    pub is_manual: bool,
}

impl<T> QueueEntry<T> {
    fn new(track: T, is_manual: bool) -> Self {
        Self {
            id: EntryId::fresh(),
            track,
            is_manual,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RepeatMode {
    #[default]
    Off,
    /// Loop the whole queue.
    All,
    /// Loop the current track.
    One,
}

/// What the caller should do after a queue movement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdvanceOutcome {
    /// Position changed: play whatever is now current.
    Moved,
    /// Repeat-one is on: play the same track again from the top.
    RepeatCurrent,
    /// Nothing left to play.
    Ended,
}

/// One thing to load into the player ahead of when it is needed. `occurrence`
/// is non-zero only under repeat-one, where one entry is rendered twice.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderSlot<T> {
    pub entry_id: EntryId,
    pub track: T,
    pub occurrence: usize,
}

#[derive(Clone, Debug)]
pub struct PlayQueue<T> {
    entries: Vec<QueueEntry<T>>,
    /// The pre-shuffle order, kept so turning shuffle off restores it.
    unshuffled: Option<Vec<QueueEntry<T>>>,
    position: Option<usize>,
    pub repeat_mode: RepeatMode,
    rng: SplitMix,
}

impl<T> Default for PlayQueue<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            unshuffled: None,
            position: None,
            repeat_mode: RepeatMode::Off,
            rng: SplitMix::seeded(),
        }
    }
}

impl<T: Clone> PlayQueue<T> {
    pub fn new() -> Self {
        Self::default()
    }

    // Reading

    /// Entries in the order they will play.
    pub fn entries(&self) -> &[QueueEntry<T>] {
        &self.entries
    }

    pub fn position(&self) -> Option<usize> {
        self.position
    }

    pub fn is_shuffled(&self) -> bool {
        self.unshuffled.is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn current(&self) -> Option<&QueueEntry<T>> {
        self.entries.get(self.position?)
    }

    pub fn current_track(&self) -> Option<&T> {
        self.current().map(|entry| &entry.track)
    }

    /// Everything after the current track, in play order.
    pub fn up_next(&self) -> &[QueueEntry<T>] {
        match self.position {
            Some(position) if position + 1 < self.entries.len() => &self.entries[position + 1..],
            _ => &[],
        }
    }

    /// The run of hand-queued entries immediately after the current one. Once
    /// one has played it falls behind the playhead and leaves the lane.
    pub fn lane(&self) -> &[QueueEntry<T>] {
        let up_next = self.up_next();
        let count = up_next.iter().take_while(|entry| entry.is_manual).count();
        &up_next[..count]
    }

    /// What follows the lane: the rest of whatever was playing.
    pub fn context_up_next(&self) -> &[QueueEntry<T>] {
        &self.up_next()[self.lane().len()..]
    }

    /// Index just past the lane, which is where Play Last inserts.
    fn lane_end(&self) -> usize {
        match self.position {
            Some(position) => position + 1 + self.lane().len(),
            None => self.entries.len(),
        }
    }

    /// The next `count` things that will play, current first, by walking a copy
    /// through the same path a natural ending takes.
    pub fn render_window(&self, count: usize) -> Vec<RenderSlot<T>> {
        let Some(current) = self.current() else {
            return Vec::new();
        };
        if count == 0 {
            return Vec::new();
        }
        let mut occurrences: HashMap<EntryId, usize> = HashMap::from([(current.id, 0)]);
        let mut window = vec![RenderSlot {
            entry_id: current.id,
            track: current.track.clone(),
            occurrence: 0,
        }];
        let mut probe = self.clone();
        while window.len() < count {
            if probe.advance_automatically() == AdvanceOutcome::Ended {
                break;
            }
            let Some(entry) = probe.current() else { break };
            let n = occurrences.get(&entry.id).map_or(0, |n| n + 1);
            occurrences.insert(entry.id, n);
            window.push(RenderSlot {
                entry_id: entry.id,
                track: entry.track.clone(),
                occurrence: n,
            });
        }
        window
    }

    // Loading

    /// Replaces the queue. `index` is into `tracks` as given, before shuffling.
    /// What the listener queued by hand outlives the thing that was playing.
    pub fn load(&mut self, tracks: Vec<T>, index: usize, shuffled: bool) {
        let kept: Vec<QueueEntry<T>> = self.lane().to_vec();
        let new_entries: Vec<QueueEntry<T>> = tracks
            .into_iter()
            .map(|track| QueueEntry::new(track, false))
            .collect();
        self.position = if new_entries.is_empty() {
            None
        } else {
            Some(index.min(new_entries.len() - 1))
        };
        self.entries = new_entries;
        self.unshuffled = None;
        if shuffled {
            self.set_shuffled(true);
        }
        if kept.is_empty() {
            return;
        }
        match self.position {
            Some(position) => {
                splice_in(&mut self.entries, position + 1, &kept);
                if let Some(saved) = self.unshuffled.as_mut() {
                    let at = (position + 1).min(saved.len());
                    splice_in(saved, at, &kept);
                }
            }
            None => {
                self.entries = kept;
                self.position = Some(0);
            }
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.unshuffled = None;
        self.position = None;
    }

    /// Drops everything after the current track, which keeps playing.
    pub fn clear_up_next(&mut self) {
        let Some(position) = self.position else {
            return;
        };
        let kept: Vec<EntryId> = self.entries[..=position]
            .iter()
            .map(|entry| entry.id)
            .collect();
        self.entries.retain(|entry| kept.contains(&entry.id));
        if let Some(saved) = self.unshuffled.as_mut() {
            saved.retain(|entry| kept.contains(&entry.id));
        }
    }

    // Movement

    /// The current track finished on its own. Repeat-one applies here only.
    pub fn advance_automatically(&mut self) -> AdvanceOutcome {
        if self.position.is_none() {
            return AdvanceOutcome::Ended;
        }
        if self.repeat_mode == RepeatMode::One {
            return AdvanceOutcome::RepeatCurrent;
        }
        self.move_forward(self.repeat_mode == RepeatMode::All)
    }

    /// The user pressed next. Wraps under any repeat mode, so repeat-one on the
    /// last track is not a trap.
    pub fn skip_to_next(&mut self) -> AdvanceOutcome {
        if self.position.is_none() {
            return AdvanceOutcome::Ended;
        }
        self.move_forward(self.repeat_mode != RepeatMode::Off)
    }

    /// The user pressed previous. Whether to restart the current track instead
    /// is the player's call, since it depends on elapsed time.
    pub fn skip_to_previous(&mut self) -> AdvanceOutcome {
        let Some(position) = self.position else {
            return AdvanceOutcome::Ended;
        };
        if position > 0 {
            self.position = Some(position - 1);
            return AdvanceOutcome::Moved;
        }
        if self.repeat_mode != RepeatMode::Off && !self.entries.is_empty() {
            self.position = Some(self.entries.len() - 1);
            return AdvanceOutcome::Moved;
        }
        AdvanceOutcome::Ended
    }

    fn move_forward(&mut self, wrapping: bool) -> AdvanceOutcome {
        let Some(position) = self.position else {
            return AdvanceOutcome::Ended;
        };
        if position + 1 < self.entries.len() {
            self.position = Some(position + 1);
            return AdvanceOutcome::Moved;
        }
        if wrapping && !self.entries.is_empty() {
            self.position = Some(0);
            return AdvanceOutcome::Moved;
        }
        AdvanceOutcome::Ended
    }

    /// Jump straight to a slot.
    pub fn jump(&mut self, entry_id: EntryId) -> bool {
        match self.entries.iter().position(|entry| entry.id == entry_id) {
            Some(index) => {
                self.position = Some(index);
                true
            }
            None => false,
        }
    }

    // Shuffle

    /// On keeps the current song playing and shuffles everything else around
    /// it, leaving the lane in place; off restores the original order and
    /// resumes from wherever the current song sits in it.
    pub fn set_shuffled(&mut self, shuffled: bool) {
        if shuffled == self.is_shuffled() {
            return;
        }
        if shuffled {
            let original = self.entries.clone();
            let Some(current) = self.current().cloned() else {
                self.unshuffled = Some(original);
                let mut entries = std::mem::take(&mut self.entries);
                self.rng.shuffle(&mut entries);
                self.entries = entries;
                return;
            };
            let kept_lane: Vec<QueueEntry<T>> = self.lane().to_vec();
            let untouched: Vec<EntryId> = std::iter::once(current.id)
                .chain(kept_lane.iter().map(|entry| entry.id))
                .collect();
            let mut rest: Vec<QueueEntry<T>> = self
                .entries
                .iter()
                .filter(|entry| !untouched.contains(&entry.id))
                .cloned()
                .collect();
            self.rng.shuffle(&mut rest);
            let mut entries = vec![current];
            entries.extend(kept_lane);
            entries.extend(rest);
            self.entries = entries;
            self.position = Some(0);
            self.unshuffled = Some(original);
        } else {
            let Some(original) = self.unshuffled.take() else {
                return;
            };
            let current_id = self.current().map(|entry| entry.id);
            self.entries = original;
            if let Some(index) =
                current_id.and_then(|id| self.entries.iter().position(|entry| entry.id == id))
            {
                self.position = Some(index);
            }
        }
    }

    /// Reshuffles what follows the lane, leaving the current track playing.
    pub fn reshuffle(&mut self) {
        let Some(position) = self.position else {
            return;
        };
        if !self.is_shuffled() || position + 1 >= self.entries.len() {
            return;
        }
        let start = self.lane_end();
        if start >= self.entries.len() {
            return;
        }
        let mut tail = self.entries.split_off(start);
        self.rng.shuffle(&mut tail);
        self.entries.extend(tail);
    }

    // Editing

    /// Play Next: the front of the lane, ahead of anything queued earlier.
    pub fn play_next(&mut self, tracks: Vec<T>) {
        if tracks.is_empty() {
            return;
        }
        let Some(position) = self.position else {
            self.load(tracks, 0, false);
            return;
        };
        let new_entries: Vec<QueueEntry<T>> = tracks
            .into_iter()
            .map(|track| QueueEntry::new(track, true))
            .collect();
        splice_in(&mut self.entries, position + 1, &new_entries);
        // Mirror the placement, not just the membership, so turning shuffle
        // off does not fling the song to the end.
        let current_id = self.current().map(|entry| entry.id);
        if let Some(saved) = self.unshuffled.as_mut() {
            match current_id.and_then(|id| saved.iter().position(|entry| entry.id == id)) {
                Some(anchor) => splice_in(saved, anchor + 1, &new_entries),
                None => saved.extend(new_entries),
            }
        }
    }

    /// Songs found to keep the queue going: the back of the queue, behind the
    /// lane, shuffled like context.
    pub fn extend(&mut self, tracks: Vec<T>) {
        if tracks.is_empty() {
            return;
        }
        let new_entries: Vec<QueueEntry<T>> = tracks
            .into_iter()
            .map(|track| QueueEntry::new(track, false))
            .collect();
        self.entries.extend(new_entries.iter().cloned());
        if let Some(saved) = self.unshuffled.as_mut() {
            saved.extend(new_entries);
        }
        if self.position.is_none() {
            self.position = Some(0);
        }
    }

    /// Play Last: the back of the lane, not the back of the queue.
    pub fn append(&mut self, tracks: Vec<T>) {
        if tracks.is_empty() {
            return;
        }
        let new_entries: Vec<QueueEntry<T>> = tracks
            .into_iter()
            .map(|track| QueueEntry::new(track, true))
            .collect();
        if self.position.is_none() {
            self.entries.extend(new_entries.iter().cloned());
            if let Some(saved) = self.unshuffled.as_mut() {
                saved.extend(new_entries);
            }
            self.position = Some(0);
            return;
        }
        let at = self.lane_end();
        let current_id = self.current().map(|entry| entry.id);
        let last_lane_id = self.lane().last().map(|entry| entry.id);
        splice_in(&mut self.entries, at, &new_entries);
        if let Some(saved) = self.unshuffled.as_mut() {
            let find = |id: EntryId| saved.iter().position(|entry| entry.id == id);
            let anchor = current_id.and_then(find);
            let lane_anchor = last_lane_id.and_then(find);
            let at = lane_anchor
                .or(anchor)
                .map_or(saved.len(), |index| index + 1);
            splice_in(saved, at, &new_entries);
        }
    }

    /// Removing the current entry leaves the position on whatever slid into
    /// its place, so playback continues rather than stopping.
    pub fn remove(&mut self, entry_id: EntryId) {
        let Some(index) = self.entries.iter().position(|entry| entry.id == entry_id) else {
            return;
        };
        self.entries.remove(index);
        if let Some(saved) = self.unshuffled.as_mut() {
            saved.retain(|entry| entry.id != entry_id);
        }
        let Some(position) = self.position else {
            return;
        };
        if self.entries.is_empty() {
            self.position = None;
        } else if index < position {
            self.position = Some(position - 1);
        } else if index == position {
            self.position = Some(position.min(self.entries.len() - 1));
        }
    }

    /// Reorders by drag. `destination` is an offset in the list as it looks
    /// before the removals. Ignores the whole gesture if any index is out of
    /// range, since a different reorder than the one drawn is worse than none.
    pub fn move_entries(&mut self, source: &[usize], destination: usize) {
        if source.iter().any(|&index| index >= self.entries.len()) {
            return;
        }
        if destination > self.entries.len() {
            return;
        }
        let mut source: Vec<usize> = source.to_vec();
        source.sort_unstable();
        source.dedup();

        let current_id = self.current().map(|entry| entry.id);
        let moving: Vec<QueueEntry<T>> = source
            .iter()
            .map(|&index| self.entries[index].clone())
            .collect();
        for &index in source.iter().rev() {
            self.entries.remove(index);
        }
        let before = source.iter().filter(|&&index| index < destination).count();
        let at = destination.saturating_sub(before).min(self.entries.len());
        splice_in(&mut self.entries, at, &moving);
        if let Some(index) =
            current_id.and_then(|id| self.entries.iter().position(|entry| entry.id == id))
        {
            self.position = Some(index);
        }
    }
}

fn splice_in<T: Clone>(entries: &mut Vec<QueueEntry<T>>, at: usize, new: &[QueueEntry<T>]) {
    entries.splice(at..at, new.iter().cloned());
}

/// A small shuffling source seeded from the standard library's per-process
/// random state, so the queue needs no random-number crate.
#[derive(Clone, Debug)]
struct SplitMix(u64);

impl SplitMix {
    fn seeded() -> Self {
        SplitMix(RandomState::new().hash_one(EntryId::fresh().0))
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Fisher-Yates.
    fn shuffle<V>(&mut self, items: &mut [V]) {
        for i in (1..items.len()).rev() {
            let j = (self.next() % (i as u64 + 1)) as usize;
            items.swap(i, j);
        }
    }
}
