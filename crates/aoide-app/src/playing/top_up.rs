//! Infinity: topping the queue up as it runs short.

use aoide_audio::RepeatMode;
use aoide_store::PlaySource;
use iced::Task;

use super::{Destination, Pending};
use crate::app::{App, Message};
use crate::playback::Enqueue;
use crate::playback::infinity::{self, TopUp};
use crate::playback::tracks::TrackMeta;

impl App {
    /// Tops the queue up if Infinity is on and it is running short.
    pub(crate) fn check_infinity(&mut self) -> Task<Message> {
        let enabled = self.config.settings.infinity;
        let repeat_off = self.playback.repeat() == RepeatMode::Off;
        let current = self
            .playback
            .current()
            .map(|(entry, q)| (entry, q.meta.id.clone()));
        let ahead = self.playback.ahead();
        let length = self.playback.queue().map_or(0, |q| q.entries().len());
        let known = self.playback.known_ids();
        let queue = self.playback.up_next_ids();
        let store = self.store.clone();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        if !signed.infinity.should_fire(
            enabled,
            repeat_off,
            current.as_ref().map(|c| c.0),
            ahead,
            length,
        ) {
            return Task::none();
        }
        let Some((_, seed)) = current else {
            return Task::none();
        };
        if self.config.settings.infinity_albums {
            return self.top_up_with_albums();
        }
        let ask = infinity::Ask {
            seed,
            queue,
            known,
            use_sidecar: signed.next_available,
        };
        let client = signed.connection.client.clone();
        let sidecar = signed.sidecar.clone();
        Task::perform(
            infinity::top_up(client, sidecar, store, ask, aoide_store::now_millis()),
            |found| Message::ToppedUp(Box::new(found)),
        )
    }

    pub(crate) fn topped_up(&mut self, found: TopUp) -> Task<Message> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        signed.infinity.finished();
        if found.sidecar_absent {
            signed.next_available = false;
        }
        if found.items.is_empty() {
            return Task::none();
        }
        let pending = Pending {
            tracks: found.items.iter().map(TrackMeta::from_item).collect(),
            how: Destination::Play {
                how: Enqueue::Extend,
                source: PlaySource::Unknown,
                reasons: found.reasons,
            },
            generation: signed.play_generation,
        };
        self.measure_then_queue(pending)
    }

    /// Infinity was switched on: look at the queue now, not at the next track.
    pub(crate) fn infinity_switched(&mut self) -> Task<Message> {
        if let Some(signed) = self.session.as_mut() {
            signed.infinity.reset();
        }
        self.check_infinity()
    }
}
