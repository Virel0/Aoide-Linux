//! What has been rendered, mapped to what is heard, and publishing it.

use std::sync::atomic::Ordering;
use std::sync::mpsc::SyncSender;

use super::deck::position_ms;
use super::render::Engine;
use super::{ANCHORS, EngineEvent, PENDING_EVENTS, POSITION_EVERY_MS, Transport};
use crate::queue::EntryId;

#[derive(Clone, Copy)]
pub(super) struct Anchor {
    pub(super) clock: u64,
    pub(super) id: Option<EntryId>,
    pub(super) ms: f64,
    pub(super) playing: bool,
}

/// What has been rendered, mapped to what will be heard once the output's
/// latency has passed.
pub(super) struct Timeline {
    anchors: [Anchor; ANCHORS],
    anchor_count: usize,
    next_anchor: usize,
    pub(super) events: Vec<(u64, EngineEvent)>,
    last_position_ms: f64,
    last_position_id: Option<EntryId>,
}

impl Timeline {
    pub(super) fn new() -> Self {
        Self {
            anchors: [Anchor {
                clock: 0,
                id: None,
                ms: 0.0,
                playing: false,
            }; ANCHORS],
            anchor_count: 0,
            next_anchor: 0,
            events: Vec::with_capacity(PENDING_EVENTS),
            last_position_ms: f64::NEG_INFINITY,
            last_position_id: None,
        }
    }

    pub(super) fn anchor(&mut self, anchor: Anchor) {
        self.anchors[self.next_anchor] = anchor;
        self.next_anchor = (self.next_anchor + 1) % ANCHORS;
        self.anchor_count = (self.anchor_count + 1).min(ANCHORS);
    }

    pub(super) fn event(&mut self, clock: u64, event: EngineEvent, out: &SyncSender<EngineEvent>) {
        if self.events.len() == self.events.capacity() {
            // Too many in one latency window: send the oldest now.
            let (_, oldest) = self.events.remove(0);
            let _ = out.try_send(oldest);
        }
        self.events.push((clock, event));
    }

    /// The anchor in force at `clock`.
    pub(super) fn at(&self, clock: u64) -> Option<Anchor> {
        (0..self.anchor_count)
            .map(|back| self.anchors[(self.next_anchor + ANCHORS - 1 - back) % ANCHORS])
            .find(|anchor| anchor.clock <= clock)
    }
}

impl Engine {
    /// Records the foreground track's position at the current clock.
    pub(super) fn anchor_here(&mut self, playing: bool) {
        let deck = match &self.blend {
            Some(blend) if blend.progress.is_some() => Some(&blend.deck),
            _ => self.primary.as_ref(),
        };
        let (id, ms) = match deck.and_then(|deck| deck.track.as_ref()) {
            Some(track) => (Some(track.id), position_ms(track, self.config.sample_rate)),
            None => (None, 0.0),
        };
        self.timeline.anchor(Anchor {
            clock: self.clock,
            id,
            ms,
            playing,
        });
    }

    /// Sends what has become audible and updates the shared position.
    pub(super) fn publish(&mut self) {
        let heard = self.clock.saturating_sub(self.latency);
        self.tap.publish(self.clock, heard);
        let events = &mut self.timeline.events;
        let due = events
            .iter()
            .take_while(|(clock, _)| *clock <= heard)
            .count();
        for (_, event) in events.drain(..due) {
            let _ = self.events.try_send(event);
        }

        let anchor = self.timeline.at(heard);
        let (id, ms) = match anchor {
            Some(Anchor {
                id: Some(id),
                ms,
                playing,
                clock,
            }) => {
                let elapsed = if playing {
                    (heard - clock) as f64 * 1000.0 / f64::from(self.config.sample_rate)
                } else {
                    0.0
                };
                (Some(id), ms + elapsed)
            }
            _ => (None, 0.0),
        };
        self.share(id, ms);
        if let Some(id) = id {
            let moved = self.timeline.last_position_id != Some(id)
                || (ms - self.timeline.last_position_ms).abs() >= POSITION_EVERY_MS;
            if moved {
                self.timeline.last_position_id = Some(id);
                self.timeline.last_position_ms = ms;
                let _ = self
                    .events
                    .try_send(EngineEvent::Position { id, ms: ms as u64 });
            }
        }
    }

    /// Stores the heard track, its position and the transport where the handle reads
    /// them.
    fn share(&self, id: Option<EntryId>, ms: f64) {
        self.shared
            .current
            .store(id.map_or(0, |id| id.0), Ordering::Relaxed);
        self.shared.position_ms.store(ms as u64, Ordering::Relaxed);
        self.shared.transport.store(
            match self.transport {
                Transport::Stopped => 0,
                Transport::Playing => 1,
                Transport::Paused => 2,
            },
            Ordering::Relaxed,
        );
    }
}
