//! Handing finished feeds and specs to a reaper thread, so the render never frees.

use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::thread;

use super::render::Engine;
use super::{DecodeMode, REMEMBERED};
use crate::pipeline::{Feed, TrackSpec};

/// What the reaper thread frees, so the render never does.
pub(super) enum Garbage {
    Feed(#[allow(dead_code)] Box<dyn Feed>),
    Spec(#[allow(dead_code)] TrackSpec),
}

/// The reaper thread for `decode`, or none: inline decoding frees on the caller's
/// thread, which is not a device callback. Also none when no thread can be spawned;
/// garbage is then freed where it is dropped.
pub(super) fn spawn_reaper(decode: DecodeMode) -> Option<SyncSender<Garbage>> {
    match decode {
        DecodeMode::Inline => None,
        DecodeMode::Threaded => {
            let (tx, rx) = mpsc::sync_channel::<Garbage>(16);
            thread::Builder::new()
                .name("aoide-reaper".into())
                .spawn(move || while rx.recv().is_ok() {})
                .ok()
                .map(|_| tx)
        }
    }
}

impl Engine {
    pub(super) fn drop_decks(&mut self) {
        if let Some(deck) = self.primary.take() {
            self.reap(deck.feed);
        }
        if let Some(blend) = self.blend.take() {
            self.reap(blend.deck.feed);
        }
        if let Some(queued) = self.queued.take() {
            self.reap(queued.deck.feed);
        }
    }

    pub(super) fn reap(&mut self, feed: Box<dyn Feed>) {
        self.discard(Garbage::Feed(feed));
    }

    pub(super) fn reap_spec(&mut self, spec: TrackSpec) {
        self.discard(Garbage::Spec(spec));
    }

    fn discard(&mut self, garbage: Garbage) {
        if let Some(reaper) = &self.reaper {
            match reaper.try_send(garbage) {
                Ok(()) => {}
                Err(TrySendError::Full(garbage)) | Err(TrySendError::Disconnected(garbage)) => {
                    drop(garbage)
                }
            }
        }
    }

    /// Keeps `spec` among the last few tracks, without allocating.
    pub(super) fn remember(&mut self, spec: TrackSpec) {
        if self.remembered.len() == REMEMBERED
            && let Some(old) = self.remembered.pop_front()
        {
            self.reap_spec(old);
        }
        self.remembered.push_back(spec);
    }
}
