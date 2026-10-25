//! The compressor: feed-forward, stereo-linked, with a soft knee.
//!
//! Each frame's level is the loudest of its channels (so the stereo image
//! never shifts), in dBFS. The gain computer turns that into a gain
//! reduction from the threshold, ratio and knee (Giannoulis, Massberg and
//! Reiss, "Digital Dynamic Range Compressor Design", JAES 2012: the
//! quadratic soft knee), and the reduction is smoothed in decibels with
//! separate attack and release time constants: it moves `1 − 1/e` of the way
//! to a new target in one time constant. One gain, the smoothed reduction and
//! the makeup gain, multiplies every channel.
//!
//! The engine takes [`CompressorSettings`] with
//! [`crate::EngineHandle::set_compressor`]; `None` turns it off. Off lets the
//! current reduction release and the makeup glide to 0 dB, and from then on
//! the compressor is not run at all: the output is bit-for-bit the input.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub const THRESHOLD_RANGE_DB: (f32, f32) = (-60.0, 0.0);
pub const RATIO_RANGE: (f32, f32) = (1.0, 20.0);
pub const ATTACK_RANGE_MS: (f32, f32) = (0.1, 200.0);
pub const RELEASE_RANGE_MS: (f32, f32) = (10.0, 3_000.0);
pub const MAKEUP_RANGE_DB: (f32, f32) = (0.0, 24.0);
pub const KNEE_RANGE_DB: (f32, f32) = (0.0, 24.0);

/// Time constant of the makeup gain's glide when it changes.
const MAKEUP_GLIDE_SECONDS: f64 = 0.015;
/// A reduction this small (about 0.01 % in level) is none: an exponential
/// release never reaches zero on its own.
const NO_REDUCTION_DB: f64 = 1e-3;
/// The level of digital silence, so the logarithm has something to take.
const FLOOR_DB: f64 = -200.0;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompressorSettings {
    /// −60 … 0 dBFS: where reduction starts (the middle of the knee).
    pub threshold_db: f32,
    /// 1 … 20: decibels in over the threshold for each one out.
    pub ratio: f32,
    /// 0.1 … 200 ms: the time constant of a reduction growing.
    pub attack_ms: f32,
    /// 10 … 3000 ms: the time constant of a reduction letting go.
    pub release_ms: f32,
    /// 0 … 24 dB of gain after the reduction.
    pub makeup_db: f32,
    /// 0 … 24 dB: the width of the soft knee around the threshold; 0 is a
    /// hard knee.
    pub knee_db: f32,
}

impl Default for CompressorSettings {
    fn default() -> Self {
        Self {
            threshold_db: -20.0,
            ratio: 3.0,
            attack_ms: 10.0,
            release_ms: 150.0,
            makeup_db: 0.0,
            knee_db: 6.0,
        }
    }
}

impl CompressorSettings {
    /// Checks the settings at the boundary: every number finite. Values out of
    /// range are held to it.
    pub fn validated(&self) -> Result<Self> {
        let values = [
            self.threshold_db,
            self.ratio,
            self.attack_ms,
            self.release_ms,
            self.makeup_db,
            self.knee_db,
        ];
        if !values.iter().all(|value| value.is_finite()) {
            return Err(Error::Settings(
                "a compressor setting is not a number".into(),
            ));
        }
        let clamp = |value: f32, (low, high): (f32, f32)| value.clamp(low, high);
        Ok(Self {
            threshold_db: clamp(self.threshold_db, THRESHOLD_RANGE_DB),
            ratio: clamp(self.ratio, RATIO_RANGE),
            attack_ms: clamp(self.attack_ms, ATTACK_RANGE_MS),
            release_ms: clamp(self.release_ms, RELEASE_RANGE_MS),
            makeup_db: clamp(self.makeup_db, MAKEUP_RANGE_DB),
            knee_db: clamp(self.knee_db, KNEE_RANGE_DB),
        })
    }

    /// The gain reduction, in dB (zero or more), for a steady input level:
    /// the static curve, before makeup.
    pub fn reduction_db(&self, input_db: f64) -> f64 {
        let threshold = f64::from(self.threshold_db);
        let ratio = f64::from(self.ratio);
        let knee = f64::from(self.knee_db);
        let over = input_db - threshold;
        let output = if 2.0 * over < -knee {
            input_db
        } else if knee > 0.0 && 2.0 * over.abs() <= knee {
            input_db + (1.0 / ratio - 1.0) * (over + knee / 2.0).powi(2) / (2.0 * knee)
        } else {
            threshold + over / ratio
        };
        (input_db - output).max(0.0)
    }

    /// What comes out for a steady input level, makeup included, in dBFS.
    pub fn output_db(&self, input_db: f64) -> f64 {
        input_db - self.reduction_db(input_db) + f64::from(self.makeup_db)
    }
}

/// A named setting.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompressorPreset {
    pub name: String,
    pub settings: CompressorSettings,
}

/// The presets that come with the app: Gentle, Moderate, Heavy and Late
/// Night (quiet passages up, loud ones down, for listening at low volume).
/// Makeup gain is what brings a typical master back to about the level it
/// had; positive makeup can clip a track that peaks at full scale.
pub fn built_in_presets() -> Vec<CompressorPreset> {
    let preset = |name: &str, threshold, ratio, attack, release, makeup, knee| CompressorPreset {
        name: name.to_string(),
        settings: CompressorSettings {
            threshold_db: threshold,
            ratio,
            attack_ms: attack,
            release_ms: release,
            makeup_db: makeup,
            knee_db: knee,
        },
    };
    vec![
        preset("Gentle", -18.0, 2.0, 20.0, 250.0, 2.0, 6.0),
        preset("Moderate", -20.0, 3.0, 10.0, 150.0, 4.0, 6.0),
        preset("Heavy", -24.0, 6.0, 5.0, 100.0, 7.0, 4.0),
        preset("Late Night", -32.0, 4.0, 5.0, 300.0, 10.0, 8.0),
    ]
}

/// The render-side compressor. Allocates nothing.
pub(crate) struct Compressor {
    rate: f64,
    channels: usize,
    settings: Option<CompressorSettings>,
    attack: f64,
    release: f64,
    /// The smoothed gain reduction, dB.
    reduction: f64,
    makeup: f64,
    makeup_target: f64,
    makeup_glide: f64,
}

impl Compressor {
    pub(crate) fn new(sample_rate: u32, channels: usize) -> Self {
        let rate = f64::from(sample_rate);
        Self {
            rate,
            channels,
            settings: None,
            attack: 0.0,
            release: 0.0,
            reduction: 0.0,
            makeup: 1.0,
            makeup_target: 1.0,
            makeup_glide: 1.0 - (-1.0 / (MAKEUP_GLIDE_SECONDS * rate)).exp(),
        }
    }

    /// Takes validated settings, or `None` for off.
    pub(crate) fn set(&mut self, settings: Option<CompressorSettings>) {
        self.settings = settings;
        if let Some(settings) = settings {
            self.attack = coefficient(settings.attack_ms, self.rate);
            self.release = coefficient(settings.release_ms, self.rate);
            self.makeup_target = 10f64.powf(f64::from(settings.makeup_db) / 20.0);
        } else {
            // Off keeps the release it had, so what is held lets go as it
            // would have.
            self.makeup_target = 1.0;
        }
    }

    fn is_transparent(&self) -> bool {
        self.settings.is_none() && self.reduction == 0.0 && self.makeup == 1.0
    }

    /// Compresses `out` (interleaved) in place.
    pub(crate) fn process(&mut self, out: &mut [f32]) {
        if self.is_transparent() {
            return;
        }
        for frame in out.chunks_exact_mut(self.channels) {
            let target = match &self.settings {
                Some(settings) => {
                    let peak = frame.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
                    let level = if peak > 0.0 {
                        20.0 * f64::from(peak).log10()
                    } else {
                        FLOOR_DB
                    };
                    settings.reduction_db(level)
                }
                None => 0.0,
            };
            let coefficient = if target > self.reduction {
                self.attack
            } else {
                self.release
            };
            self.reduction = coefficient * self.reduction + (1.0 - coefficient) * target;
            if self.reduction < NO_REDUCTION_DB && target == 0.0 {
                self.reduction = 0.0;
            }
            if self.makeup != self.makeup_target {
                self.makeup += (self.makeup_target - self.makeup) * self.makeup_glide;
                if (self.makeup - self.makeup_target).abs() < 1e-7 {
                    self.makeup = self.makeup_target;
                }
            }
            if self.reduction == 0.0 && self.makeup == 1.0 {
                continue;
            }
            let gain = (10f64.powf(-self.reduction / 20.0) * self.makeup) as f32;
            for sample in frame {
                *sample *= gain;
            }
        }
    }
}

/// The one-pole coefficient for a time constant.
fn coefficient(ms: f32, rate: f64) -> f64 {
    (-1.0 / (f64::from(ms) / 1000.0 * rate)).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_knee_meets_both_straight_lines() {
        let settings = CompressorSettings {
            threshold_db: -20.0,
            ratio: 4.0,
            knee_db: 10.0,
            ..Default::default()
        };
        // At its edges the knee is continuous with what lies either side.
        assert!(settings.reduction_db(-25.0).abs() < 1e-12);
        let above = -15.0 - (-20.0 + 5.0 / 4.0);
        assert!((settings.reduction_db(-15.0) - above).abs() < 1e-12);
    }

    #[test]
    fn a_nan_is_refused_and_ranges_hold() {
        let nan = CompressorSettings {
            ratio: f32::NAN,
            ..Default::default()
        };
        assert!(nan.validated().is_err());
        let wild = CompressorSettings {
            ratio: 1_000.0,
            attack_ms: 0.0,
            ..Default::default()
        }
        .validated()
        .unwrap();
        assert_eq!((wild.ratio, wild.attack_ms), (20.0, 0.1));
    }
}
