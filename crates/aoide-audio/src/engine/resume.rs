//! Playing on from what was last heard after the output lost its buffer.

use super::blend::Blend;
use super::deck::Deck;
use super::render::Engine;
use super::timeline::Anchor;
use super::{Transport, make_feed};
use crate::pipeline::Feed;
use crate::queue::EntryId;

impl Engine {
    /// Plays on from what was last heard rather than from what was last
    /// rendered, after the output lost what it had buffered (its stream
    /// failed): the current track is opened again at the heard position, and
    /// what was chained after it is chained again, so the queue carries on.
    /// Then the output latency is zero until the next stream reports its own.
    ///
    /// Only for a track playing on its own: during a blend, or when a track
    /// boundary had been rendered but not yet heard, the engine simply plays
    /// on from what it rendered, losing at most one buffer of sound.
    ///
    /// Not for the device callback: it opens the track again.
    pub fn resume_from_heard(&mut self) {
        let unheard = self.latency.min(self.clock);
        self.latency = 0;
        if unheard == 0
            || self.transport != Transport::Playing
            || self
                .blend
                .as_ref()
                .is_some_and(|blend| blend.progress.is_some())
        {
            return;
        }
        let heard = self.clock - unheard;
        let Some((id, heard_ms)) = self.heard_on_primary(heard) else {
            return;
        };
        let Some(spec) = self.remembered.iter().rev().find(|spec| spec.id == id) else {
            return;
        };
        let Ok(mut feed) = make_feed(self.config, spec.clone(), heard_ms.round() as u64) else {
            return;
        };
        let blend = self.rechain(id, feed.as_mut());
        self.drop_decks();
        self.primary = Some(Deck::new(feed));
        self.blend = blend;
        // What had been rendered after the heard frame is rendered again.
        self.timeline.events.retain(|(at, _)| *at <= heard);
    }

    /// The track heard at clock `heard` and how far into its file, in
    /// milliseconds, when it was playing and is still the primary deck's.
    fn heard_on_primary(&self, heard: u64) -> Option<(EntryId, f64)> {
        let Some(Anchor {
            id: Some(id),
            ms,
            playing: true,
            clock,
        }) = self.timeline.at(heard)
        else {
            return None;
        };
        let current = self
            .primary
            .as_ref()
            .and_then(|deck| deck.track.as_ref())
            .map(|track| track.id);
        if current != Some(id) {
            return None;
        }
        let rate = f64::from(self.config.sample_rate);
        Some((id, ms + (heard - clock) as f64 * 1000.0 / rate))
    }

    /// Chains what followed track `id` onto `feed`, its fresh feed, again —
    /// unless it is `id` itself (the join to it has been heard and nothing
    /// follows it yet). A follower that blends in comes back as the blend to
    /// run; one that fails to open is dropped.
    fn rechain(&self, id: EntryId, feed: &mut dyn Feed) -> Option<Blend> {
        match &self.chained {
            Some((next, _)) if next.id == id => None,
            Some((next, None)) => {
                feed.chain(Some(next.clone()));
                None
            }
            Some((next, Some(overlap))) => {
                make_feed(self.config, next.clone(), 0)
                    .ok()
                    .map(|next_feed| Blend {
                        deck: Deck::new(next_feed),
                        overlap: *overlap,
                        progress: None,
                    })
            }
            None => None,
        }
    }
}
