//! A deck: one feed being played, the track it is in, and reading from it.

use std::sync::mpsc::SyncSender;

use super::EngineEvent;
use super::timeline::{Anchor, Timeline};
use crate::pipeline::{Feed, FeedRead, Marker};
use crate::queue::EntryId;

pub(super) struct DeckTrack {
    pub(super) id: EntryId,
    pub(super) gain: f32,
    pub(super) origin_ms: f64,
    pub(super) played: u64,
    pub(super) length: Option<u64>,
}

pub(super) struct Deck {
    pub(super) feed: Box<dyn Feed>,
    pub(super) track: Option<DeckTrack>,
    pub(super) finished: bool,
    pub(super) starved: bool,
}

impl Deck {
    pub(super) fn new(feed: Box<dyn Feed>) -> Self {
        Self {
            feed,
            track: None,
            finished: false,
            starved: false,
        }
    }

    /// Output frames left in the current track, when its length is known.
    pub(super) fn remaining(&self) -> Option<u64> {
        let track = self.track.as_ref()?;
        Some(track.length?.saturating_sub(track.played))
    }

    /// Takes `frames` frames just read into `region`: at the current track's
    /// gain, counted as played. Audio before any track has started is
    /// silenced.
    fn take_audio(&mut self, region: &mut [f32], frames: usize) {
        match self.track.as_mut() {
            Some(track) => {
                if track.gain != 1.0 {
                    for sample in region.iter_mut() {
                        *sample *= track.gain;
                    }
                }
                track.played += frames as u64;
            }
            None => region.fill(0.0),
        }
    }
}

pub(super) fn position_ms(track: &DeckTrack, rate: u32) -> f64 {
    track.origin_ms + track.played as f64 * 1000.0 / f64::from(rate)
}

/// Reads a deck's audio into `out` (overwriting it), handling the markers met
/// on the way at their exact frame. `foreground` decks move the heard
/// position; a track fading out under a blend only reports its end. Returns
/// the frames of audio written; the rest of `out` is silence (starved or
/// finished).
#[allow(clippy::too_many_arguments)]
pub(super) fn pull(
    deck: &mut Deck,
    out: &mut [f32],
    channels: usize,
    timeline: &mut Timeline,
    clock: u64,
    foreground: bool,
    rate: u32,
    events: &SyncSender<EngineEvent>,
) -> usize {
    let mut written = 0;
    let frames_total = out.len() / channels;
    while written < frames_total && !deck.finished {
        let at = clock + written as u64;
        match deck.feed.read(&mut out[written * channels..]) {
            FeedRead::Audio(frames) => {
                if deck.starved {
                    deck.starved = false;
                    if foreground {
                        anchor_track(timeline, deck, at, rate, true);
                    }
                }
                deck.take_audio(
                    &mut out[written * channels..(written + frames) * channels],
                    frames,
                );
                written += frames;
            }
            FeedRead::Marker(marker) => on_marker(deck, marker, timeline, at, foreground, events),
            FeedRead::Starved => {
                if !deck.starved {
                    deck.starved = true;
                    if foreground {
                        anchor_track(timeline, deck, at, rate, false);
                    }
                }
                break;
            }
            FeedRead::Finished => deck.finished = true,
        }
    }
    out[written * channels..].fill(0.0);
    written
}

/// Anchors the heard position at `clock` to where the deck's track has got
/// to, when it is in one.
fn anchor_track(timeline: &mut Timeline, deck: &Deck, clock: u64, rate: u32, playing: bool) {
    if let Some(track) = &deck.track {
        timeline.anchor(Anchor {
            clock,
            id: Some(track.id),
            ms: position_ms(track, rate),
            playing,
        });
    }
}

/// A marker met at `at`: a track starting or ending on the deck, or an error.
fn on_marker(
    deck: &mut Deck,
    marker: Marker,
    timeline: &mut Timeline,
    at: u64,
    foreground: bool,
    events: &SyncSender<EngineEvent>,
) {
    match marker {
        Marker::Start {
            id,
            gain,
            origin_ms,
            length,
        } => {
            deck.track = Some(DeckTrack {
                id,
                gain,
                origin_ms,
                played: 0,
                length,
            });
            if foreground {
                timeline.anchor(Anchor {
                    clock: at,
                    id: Some(id),
                    ms: origin_ms,
                    playing: true,
                });
            }
            timeline.event(at, EngineEvent::TrackStarted { id }, events);
        }
        Marker::End { id } => {
            if deck.track.as_ref().is_some_and(|track| track.id == id) {
                deck.track = None;
            }
            timeline.event(at, EngineEvent::TrackEnded { id }, events);
        }
        Marker::Error { id, message } => {
            timeline.event(at, EngineEvent::Error { id, message }, events);
        }
    }
}
