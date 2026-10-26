//! The pause, resume and declick fades, counted in whole frames.

use crate::mix::curve;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum FadeShape {
    Linear,
    /// sin(p·π/2): constant power across a fade, so the music seems to ease
    /// off evenly rather than hang on and then drop.
    EqualPower,
}

/// A fade counted in whole frames, so where it ends is exact.
#[derive(Clone, Copy, Debug)]
pub(super) struct Fade {
    length: u32,
    /// 0 (silent) to `length` (full).
    pub(super) at: u32,
    shape: FadeShape,
}

impl Fade {
    pub(super) fn full(length: u32, shape: FadeShape) -> Self {
        Self {
            length,
            at: length,
            shape,
        }
    }

    pub(super) fn silent(length: u32, shape: FadeShape) -> Self {
        Self {
            length,
            at: 0,
            shape,
        }
    }

    pub(super) fn is_silent(&self) -> bool {
        self.at == 0
    }

    fn level(&self, at: u32) -> f32 {
        if at >= self.length {
            return 1.0;
        }
        let progress = f64::from(at) / f64::from(self.length);
        match self.shape {
            FadeShape::Linear => progress as f32,
            FadeShape::EqualPower => curve::incoming(progress) as f32,
        }
    }

    /// Another length and shape, from the level this one has reached, so
    /// changing fades midway does not jump.
    pub(super) fn reshape(&mut self, length: u32, shape: FadeShape) {
        if (length, shape) == (self.length, self.shape) {
            return;
        }
        let level = f64::from(self.level(self.at));
        let progress = match shape {
            FadeShape::Linear => level,
            FadeShape::EqualPower => level.asin() / std::f64::consts::FRAC_PI_2,
        };
        *self = Self {
            length,
            at: (progress * f64::from(length)).round() as u32,
            shape,
        };
    }

    /// The level for the next frame. Fading in, the first frame is already
    /// one step up and the `length`th is full; fading out, the first is
    /// still full and the `length`th one step above silence. The two are
    /// mirror images, and a fade-out of `length` frames sounds for exactly
    /// `length` frames.
    pub(super) fn step(&mut self, up: bool) -> f32 {
        if up {
            if self.at < self.length {
                self.at += 1;
            }
            self.level(self.at)
        } else if self.at > 0 {
            let level = self.level(self.at);
            self.at -= 1;
            level
        } else {
            0.0
        }
    }
}
