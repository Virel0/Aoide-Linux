//! The commands a handle sends, and applying them at the start of a render.

use super::Transport;
use super::blend::Blend;
use super::deck::Deck;
use super::fade::{Fade, FadeShape};
use super::render::Engine;
use super::timeline::Anchor;
use crate::compressor::CompressorSettings;
use crate::eq::EqParams;
use crate::pipeline::{Feed, TrackSpec};

pub(super) enum Command {
    Load {
        feed: Box<dyn Feed>,
        spec: TrackSpec,
        play: bool,
    },
    SetNext(Option<NextPlan>),
    Play,
    Pause,
    Stop,
    Volume(f32),
    Equalizer(EqParams),
    Compressor(Option<CompressorSettings>),
}

pub(super) struct NextPlan {
    pub(super) spec: TrackSpec,
    pub(super) blend: Option<(Box<dyn Feed>, u64)>,
}

impl Engine {
    pub(super) fn apply(&mut self, command: Command) {
        match command {
            Command::Load { feed, spec, play } => self.on_load(feed, spec, play),
            Command::SetNext(plan) => self.on_set_next(plan),
            Command::Play => self.on_play(),
            Command::Pause => self.on_pause(),
            Command::Stop => self.on_stop(),
            Command::Volume(volume) => self.volume_target = volume,
            Command::Equalizer(params) => self.eq.set(params),
            Command::Compressor(settings) => self.compressor.set(settings),
        }
    }

    fn on_load(&mut self, feed: Box<dyn Feed>, spec: TrackSpec, play: bool) {
        let audible = self.transport == Transport::Playing && self.primary.is_some();
        self.drop_decks();
        self.primary = Some(Deck::new(feed));
        while let Some(old) = self.remembered.pop_front() {
            self.reap_spec(old);
        }
        self.remember(spec);
        if let Some((old, _)) = self.chained.take() {
            self.reap_spec(old);
        }
        if play {
            self.transport = Transport::Playing;
            // Replacing something audible fades the new one in;
            // starting from silence is sample-exact from frame one.
            self.fade = if audible {
                Fade::silent(self.declick_frames, FadeShape::Linear)
            } else {
                Fade::full(self.declick_frames, FadeShape::Linear)
            };
        } else {
            // Loaded and held: nothing sounds until play.
            self.transport = Transport::Paused;
            self.fade = Fade::silent(self.pause_frames, FadeShape::EqualPower);
        }
    }

    fn on_set_next(&mut self, plan: Option<NextPlan>) {
        let chained = plan.as_ref().map(|plan| {
            let overlap = plan.blend.as_ref().map(|(_, overlap)| (*overlap).max(1));
            (plan.spec.clone(), overlap)
        });
        if let Some((spec, _)) = &chained {
            self.remember(spec.clone());
        }
        if let Some((old, _)) = std::mem::replace(&mut self.chained, chained) {
            self.reap_spec(old);
        }
        // A blend not yet started is replaced; one under way plays
        // out, and what follows is chained to its incoming track.
        if let Some(blend) = self.blend.take_if(|blend| blend.progress.is_none()) {
            self.reap(blend.deck.feed);
        }
        if let Some(queued) = self.queued.take() {
            self.reap(queued.deck.feed);
        }
        self.chain_next(plan);
    }

    /// Joins `plan` to the track that will be playing when the current one
    /// ends: the incoming track of a blend under way, or else the primary
    /// deck's. With no deck at all, the plan's feed is reaped.
    fn chain_next(&mut self, plan: Option<NextPlan>) {
        let blending = self.blend.is_some();
        let target = match self.blend.as_mut() {
            Some(blend) => Some(&mut blend.deck),
            None => self.primary.as_mut(),
        };
        let Some(target) = target else {
            if let Some(NextPlan {
                blend: Some((feed, _)),
                ..
            }) = plan
            {
                self.reap(feed);
            }
            return;
        };
        let next = match plan {
            None => {
                target.feed.chain(None);
                None
            }
            Some(NextPlan { spec, blend: None }) => {
                target.feed.chain(Some(spec));
                None
            }
            Some(NextPlan {
                blend: Some((feed, overlap)),
                ..
            }) => {
                target.feed.chain(None);
                Some(Blend {
                    deck: Deck::new(feed),
                    overlap: overlap.max(1),
                    progress: None,
                })
            }
        };
        if blending {
            self.queued = next;
        } else {
            self.blend = next;
        }
    }

    fn on_play(&mut self) {
        if self.primary.is_some() && self.transport != Transport::Playing {
            self.transport = Transport::Playing;
            // From wherever a pause's fade had got to.
            self.fade.reshape(self.pause_frames, FadeShape::EqualPower);
            self.anchor_here(true);
        }
    }

    fn on_pause(&mut self) {
        if self.transport == Transport::Playing {
            // The fade runs out in render, which then pauses.
            self.transport = Transport::Paused;
            self.fade.reshape(self.pause_frames, FadeShape::EqualPower);
            if self.fade.is_silent() {
                self.anchor_here(false);
            }
        }
    }

    fn on_stop(&mut self) {
        self.drop_decks();
        self.transport = Transport::Stopped;
        self.timeline.anchor(Anchor {
            clock: self.clock,
            id: None,
            ms: 0.0,
            playing: false,
        });
    }
}
