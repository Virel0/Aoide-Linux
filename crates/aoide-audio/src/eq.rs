//! The parametric equaliser: a preamp and up to [`MAX_BANDS`] peaking or
//! shelving bands, each a biquad from the RBJ Audio EQ Cookbook.
//!
//! The settings are plain data ([`EqCurve`], [`EqPreset`]) that serialise as
//! they are, for user presets and the config file. The engine takes them with
//! [`crate::EngineHandle::set_equalizer`]; `None` turns the equaliser off.
//!
//! Changes never click: every band's gain, frequency and Q glide towards what
//! was asked with a 15 ms time constant, the coefficients recomputed every 32
//! frames on the way. Turning the equaliser off is the same glide, to a flat
//! curve. A band at 0 dB is not run at all, so once a flat curve (or "off")
//! has been reached the render path does nothing to the samples and the output
//! is bit-for-bit what came in.

use std::f64::consts::PI;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// How many bands a curve may have.
pub const MAX_BANDS: usize = 10;
/// A band's gain, and the preamp, are held to ± this.
pub const GAIN_LIMIT_DB: f32 = 12.0;
/// The lowest band frequency; the highest is 20 kHz or 45 % of the rate,
/// whichever is lower.
pub const MIN_FREQ_HZ: f32 = 20.0;
/// The highest band frequency, before the 45 %-of-the-rate limit.
pub const MAX_FREQ_HZ: f32 = 20_000.0;
/// The narrowest a band's `q` is held to (widest bell).
pub const MIN_Q: f32 = 0.1;
/// The widest a band's `q` is held to (narrowest bell).
pub const MAX_Q: f32 = 10.0;

/// Frames between coefficient updates while a change is under way.
pub(crate) const CONTROL_FRAMES: usize = 32;
/// Time constant of the parameter glide.
const GLIDE_SECONDS: f64 = 0.015;
/// A band this close to 0 dB is treated as off.
const FLAT_DB: f64 = 0.005;

/// What a band does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BandKind {
    /// A bell around `freq_hz`, `q` wide.
    Peaking,
    /// Everything below `freq_hz` moved by the gain (half the gain at
    /// `freq_hz`); `q` sets the slope, 0.707 the gentlest without overshoot.
    LowShelf,
    /// The same above `freq_hz`.
    HighShelf,
}

/// One band.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EqBand {
    /// What the band does.
    pub kind: BandKind,
    /// Its centre or corner frequency, in hertz, held to [`MIN_FREQ_HZ`] …
    /// [`MAX_FREQ_HZ`] and below 45 % of the sample rate.
    pub freq_hz: f32,
    /// −12 … +12 dB.
    pub gain_db: f32,
    /// Its width or slope, held to [`MIN_Q`] … [`MAX_Q`].
    pub q: f32,
}

impl EqBand {
    /// A bell at `freq_hz` of `gain_db`, `q` wide.
    pub fn peaking(freq_hz: f32, gain_db: f32, q: f32) -> Self {
        Self {
            kind: BandKind::Peaking,
            freq_hz,
            gain_db,
            q,
        }
    }

    /// A low shelf with its corner at `freq_hz`.
    pub fn low_shelf(freq_hz: f32, gain_db: f32, q: f32) -> Self {
        Self {
            kind: BandKind::LowShelf,
            freq_hz,
            gain_db,
            q,
        }
    }

    /// A high shelf with its corner at `freq_hz`.
    pub fn high_shelf(freq_hz: f32, gain_db: f32, q: f32) -> Self {
        Self {
            kind: BandKind::HighShelf,
            freq_hz,
            gain_db,
            q,
        }
    }

    /// Its biquad at `sample_rate`, with the parameters held to their ranges.
    fn coefficients(&self, sample_rate: u32) -> Coefficients {
        let rate = f64::from(sample_rate);
        Coefficients::design(
            self.kind,
            clamp_freq(f64::from(self.freq_hz), rate),
            f64::from(self.gain_db.clamp(-GAIN_LIMIT_DB, GAIN_LIMIT_DB)),
            f64::from(self.q.clamp(MIN_Q, MAX_Q)),
            rate,
        )
    }
}

/// A whole equaliser setting: the preamp and the bands, in any order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EqCurve {
    /// −12 … +12 dB, applied before the bands.
    pub preamp_db: f32,
    /// The bands; at most [`MAX_BANDS`] are used.
    pub bands: Vec<EqBand>,
}

impl EqCurve {
    /// No preamp, no bands: what "off" glides to.
    pub fn flat() -> Self {
        Self::default()
    }

    /// Whether this curve leaves the sound untouched.
    pub fn is_flat(&self) -> bool {
        self.preamp_db == 0.0 && self.bands.iter().all(|band| band.gain_db == 0.0)
    }

    /// The curve's gain at `freq_hz`, in dB, preamp included: for drawing it,
    /// and what the render applies to a steady tone at that frequency.
    pub fn response_db(&self, freq_hz: f64, sample_rate: u32) -> f64 {
        let preamp = f64::from(self.preamp_db.clamp(-GAIN_LIMIT_DB, GAIN_LIMIT_DB));
        preamp
            + self
                .bands
                .iter()
                .take(MAX_BANDS)
                .filter(|band| band.gain_db != 0.0)
                .map(|band| {
                    band.coefficients(sample_rate)
                        .magnitude_db(freq_hz, f64::from(sample_rate))
                })
                .sum::<f64>()
    }

    /// The highest point of the curve between 20 Hz and 20 kHz, in dB: how
    /// far a boost can push a full-scale signal over 0 dBFS. A preamp of
    /// minus this keeps the curve from clipping anything that did not clip.
    pub fn peak_db(&self, sample_rate: u32) -> f64 {
        let nyquist = f64::from(sample_rate) / 2.0;
        // 1/24-octave steps.
        (0..=240)
            .map(|step| 20.0 * 2f64.powf(f64::from(step) / 24.0))
            .filter(|&freq| freq < nyquist)
            .map(|freq| self.response_db(freq, sample_rate))
            .fold(f64::NEG_INFINITY, f64::max)
    }
}

/// A named curve.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EqPreset {
    /// What the picker shows.
    pub name: String,
    /// The curve it applies.
    pub curve: EqCurve,
}

/// The ten band centres the built-in presets use, an octave apart.
pub const PRESET_FREQUENCIES_HZ: [f32; 10] = [
    32.0, 64.0, 125.0, 250.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0, 16_000.0,
];
/// About an octave wide, so neighbouring bands meet without a dip.
pub const PRESET_Q: f32 = 1.41;

const BUILT_IN: [(&str, [f32; 10]); 14] = [
    ("Flat", [0.0; 10]),
    (
        "Bass Boost",
        [6.0, 5.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    ),
    (
        "Bass Reducer",
        [-6.0, -5.0, -4.0, -2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    ),
    (
        "Treble Boost",
        [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 4.0, 5.0, 6.0],
    ),
    (
        "Treble Reducer",
        [0.0, 0.0, 0.0, 0.0, 0.0, -1.0, -2.0, -4.0, -5.0, -6.0],
    ),
    (
        "Vocal",
        [-2.0, -2.0, -1.0, 0.0, 2.0, 4.0, 4.0, 2.0, 0.0, -1.0],
    ),
    (
        "Spoken Word",
        [-4.0, -3.0, -1.0, 1.0, 3.0, 4.0, 4.0, 3.0, 1.0, -2.0],
    ),
    (
        "Loudness",
        [5.0, 4.0, 2.0, 0.0, -1.0, 0.0, 0.0, 1.0, 3.0, 4.0],
    ),
    (
        "Acoustic",
        [4.0, 4.0, 3.0, 1.0, 2.0, 2.0, 3.0, 3.0, 3.0, 2.0],
    ),
    (
        "Electronic",
        [4.0, 4.0, 1.0, 0.0, -2.0, 2.0, 1.0, 1.0, 4.0, 5.0],
    ),
    ("Rock", [5.0, 4.0, 3.0, 1.0, -1.0, -1.0, 1.0, 3.0, 4.0, 5.0]),
    (
        "Pop",
        [-1.0, -1.0, 0.0, 2.0, 4.0, 4.0, 2.0, 0.0, -1.0, -1.0],
    ),
    ("Jazz", [3.0, 2.0, 1.0, 2.0, -1.0, -1.0, 0.0, 1.0, 2.0, 3.0]),
    (
        "Classical",
        [4.0, 3.0, 2.0, 1.0, -1.0, -1.0, 0.0, 2.0, 3.0, 4.0],
    ),
];

/// The presets that come with the app: Flat, Bass Boost, Bass Reducer, Treble
/// Boost, Treble Reducer, Vocal, Spoken Word, Loudness, Acoustic, Electronic,
/// Rock, Pop, Jazz and Classical. Ten peaking bands an octave apart each, and
/// a preamp that takes the curve's highest point back to 0 dB, so a preset
/// never makes a full-scale track clip (it makes a boosted preset sound
/// quieter overall, which is the honest trade).
pub fn built_in_presets() -> Vec<EqPreset> {
    BUILT_IN
        .iter()
        .map(|(name, gains)| {
            let bands = if gains.iter().all(|&gain| gain == 0.0) {
                Vec::new()
            } else {
                PRESET_FREQUENCIES_HZ
                    .iter()
                    .zip(gains)
                    .map(|(&freq, &gain)| EqBand::peaking(freq, gain, PRESET_Q))
                    .collect()
            };
            let mut curve = EqCurve {
                preamp_db: 0.0,
                bands,
            };
            let peak = curve.peak_db(48_000);
            if peak > 0.0 {
                curve.preamp_db = -((peak * 10.0).ceil() / 10.0) as f32;
            }
            EqPreset {
                name: (*name).to_string(),
                curve,
            }
        })
        .collect()
}

/// A curve as the engine takes it: fixed-size, so it crosses to the render
/// thread without allocating there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EqParams {
    preamp_db: f32,
    bands: [Option<EqBand>; MAX_BANDS],
}

impl EqParams {
    pub(crate) fn flat() -> Self {
        Self {
            preamp_db: 0.0,
            bands: [None; MAX_BANDS],
        }
    }

    /// Checks a curve at the boundary: at most [`MAX_BANDS`] bands, every
    /// number finite. Values out of range are held to it.
    pub(crate) fn from_curve(curve: &EqCurve) -> Result<Self> {
        if curve.bands.len() > MAX_BANDS {
            return Err(Error::Settings(format!(
                "an equaliser has at most {MAX_BANDS} bands, not {}",
                curve.bands.len()
            )));
        }
        let finite = curve.preamp_db.is_finite()
            && curve
                .bands
                .iter()
                .all(|b| b.freq_hz.is_finite() && b.gain_db.is_finite() && b.q.is_finite());
        if !finite {
            return Err(Error::Settings(
                "an equaliser setting is not a number".into(),
            ));
        }
        let mut params = Self::flat();
        params.preamp_db = curve.preamp_db.clamp(-GAIN_LIMIT_DB, GAIN_LIMIT_DB);
        for (slot, band) in params.bands.iter_mut().zip(&curve.bands) {
            *slot = Some(EqBand {
                kind: band.kind,
                freq_hz: band.freq_hz.clamp(MIN_FREQ_HZ, MAX_FREQ_HZ),
                gain_db: band.gain_db.clamp(-GAIN_LIMIT_DB, GAIN_LIMIT_DB),
                q: band.q.clamp(MIN_Q, MAX_Q),
            });
        }
        Ok(params)
    }
}

fn clamp_freq(freq: f64, rate: f64) -> f64 {
    freq.clamp(
        f64::from(MIN_FREQ_HZ),
        f64::from(MAX_FREQ_HZ).min(rate * 0.45),
    )
}

/// A biquad, normalised so `a0` is one.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Coefficients {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Coefficients {
    const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    /// The RBJ Audio EQ Cookbook's peaking and shelving filters.
    fn design(kind: BandKind, freq: f64, gain_db: f64, q: f64, rate: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq / rate;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let (b0, b1, b2, a0, a1, a2) = match kind {
            BandKind::Peaking => (
                1.0 + alpha * a,
                -2.0 * cos,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cos,
                1.0 - alpha / a,
            ),
            BandKind::LowShelf => {
                let k = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cos + k),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                    a * ((a + 1.0) - (a - 1.0) * cos - k),
                    (a + 1.0) + (a - 1.0) * cos + k,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                    (a + 1.0) + (a - 1.0) * cos - k,
                )
            }
            BandKind::HighShelf => {
                let k = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cos + k),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                    a * ((a + 1.0) + (a - 1.0) * cos - k),
                    (a + 1.0) - (a - 1.0) * cos + k,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos),
                    (a + 1.0) - (a - 1.0) * cos - k,
                )
            }
        };
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// |H(e^jω)| in dB.
    fn magnitude_db(&self, freq: f64, rate: f64) -> f64 {
        let w = 2.0 * PI * freq / rate;
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let (nr, ni) = (
            self.b0 + self.b1 * c1 + self.b2 * c2,
            -(self.b1 * s1 + self.b2 * s2),
        );
        let (dr, di) = (
            1.0 + self.a1 * c1 + self.a2 * c2,
            -(self.a1 * s1 + self.a2 * s2),
        );
        10.0 * ((nr * nr + ni * ni) / (dr * dr + di * di)).log10()
    }
}

/// One band's running state: where its parameters are on their glide, and its
/// filter memory per channel (transposed direct form II).
#[derive(Clone, Copy)]
struct BandState {
    kind: BandKind,
    /// log2 of the frequency, so the glide is even in octaves.
    octave: f64,
    gain_db: f64,
    /// ln of Q.
    ln_q: f64,
    coefficients: Coefficients,
}

impl BandState {
    fn is_active(&self) -> bool {
        self.gain_db != 0.0
    }
}

/// The render-side equaliser. Allocates only in [`Equalizer::new`].
pub(crate) struct Equalizer {
    rate: f64,
    channels: usize,
    target: EqParams,
    bands: [BandState; MAX_BANDS],
    /// Filter memory, `[band][channel]`.
    memory: Vec<[f64; 2]>,
    preamp: f32,
    preamp_target: f32,
    /// Per-control-block glide factor.
    glide: f64,
    /// Per-frame glide factor for the preamp.
    preamp_glide: f32,
    /// Frames until the next coefficient update.
    countdown: usize,
    settled: bool,
}

impl Equalizer {
    pub(crate) fn new(sample_rate: u32, channels: usize) -> Self {
        let rate = f64::from(sample_rate);
        let idle = BandState {
            kind: BandKind::Peaking,
            octave: 1_000f64.log2(),
            gain_db: 0.0,
            ln_q: 1f64.ln(),
            coefficients: Coefficients::IDENTITY,
        };
        Self {
            rate,
            channels,
            target: EqParams::flat(),
            bands: [idle; MAX_BANDS],
            memory: vec![[0.0; 2]; MAX_BANDS * channels],
            preamp: 1.0,
            preamp_target: 1.0,
            glide: 1.0 - (-(CONTROL_FRAMES as f64) / (GLIDE_SECONDS * rate)).exp(),
            preamp_glide: (1.0 - (-1.0 / (GLIDE_SECONDS * rate)).exp()) as f32,
            countdown: 0,
            settled: true,
        }
    }

    pub(crate) fn set(&mut self, params: EqParams) {
        self.target = params;
        self.preamp_target = 10f32.powf(params.preamp_db / 20.0);
        if params.preamp_db == 0.0 {
            self.preamp_target = 1.0;
        }
        self.settled = false;
        self.countdown = 0;
    }

    /// Whether there is nothing to do: no band active, the preamp at unity,
    /// and nothing on its way.
    fn is_transparent(&self) -> bool {
        self.settled && self.preamp == 1.0 && !self.bands.iter().any(BandState::is_active)
    }

    /// Filters `out` (interleaved) in place.
    pub(crate) fn process(&mut self, out: &mut [f32]) {
        if self.is_transparent() {
            return;
        }
        let channels = self.channels;
        let mut at = 0;
        let frames = out.len() / channels;
        while at < frames {
            if self.countdown == 0 {
                if !self.settled {
                    self.step_glide();
                }
                self.countdown = CONTROL_FRAMES;
            }
            let span = self.countdown.min(frames - at);
            let region = &mut out[at * channels..(at + span) * channels];
            self.run(region);
            self.countdown -= span;
            at += span;
        }
        // Keep decaying memory out of the subnormal range.
        for memory in &mut self.memory {
            for value in memory {
                if value.abs() < 1e-20 {
                    *value = 0.0;
                }
            }
        }
    }

    fn run(&mut self, region: &mut [f32]) {
        let channels = self.channels;
        if self.preamp != 1.0 || self.preamp_target != 1.0 {
            for frame in region.chunks_exact_mut(channels) {
                if self.preamp != self.preamp_target {
                    self.preamp += (self.preamp_target - self.preamp) * self.preamp_glide;
                    // Within a thousandth of a decibel: f32 would never
                    // close the last of the gap.
                    if (self.preamp - self.preamp_target).abs() < 1e-4 {
                        self.preamp = self.preamp_target;
                    }
                }
                if self.preamp != 1.0 {
                    for sample in frame {
                        *sample *= self.preamp;
                    }
                }
            }
        }
        for (index, band) in self.bands.iter().enumerate() {
            if !band.is_active() {
                continue;
            }
            let c = band.coefficients;
            let memory = &mut self.memory[index * channels..(index + 1) * channels];
            for frame in region.chunks_exact_mut(channels) {
                for (sample, z) in frame.iter_mut().zip(memory.iter_mut()) {
                    let x = f64::from(*sample);
                    let y = c.b0 * x + z[0];
                    z[0] = c.b1 * x - c.a1 * y + z[1];
                    z[1] = c.b2 * x - c.a2 * y;
                    *sample = y as f32;
                }
            }
        }
    }

    /// Moves every band one control block along its glide.
    fn step_glide(&mut self) {
        let mut settled = self.preamp == self.preamp_target;
        for (index, (band, target)) in self.bands.iter_mut().zip(self.target.bands).enumerate() {
            let target = target.unwrap_or(EqBand::peaking(1_000.0, 0.0, 1.0));
            let goal_octave = clamp_freq(f64::from(target.freq_hz), self.rate).log2();
            let goal_ln_q = f64::from(target.q).ln();
            // A band changing kind first glides to 0 dB, where every kind is
            // the same (nothing), then switches.
            let goal_gain = if target.kind == band.kind {
                f64::from(target.gain_db)
            } else {
                0.0
            };
            let was_active = band.is_active();
            if !was_active {
                // Silent: take the new shape at once.
                band.kind = target.kind;
                band.octave = goal_octave;
                band.ln_q = goal_ln_q;
            }
            let moved = glide(&mut band.gain_db, goal_gain, self.glide, FLAT_DB)
                | glide(&mut band.octave, goal_octave, self.glide, 1e-4)
                | glide(&mut band.ln_q, goal_ln_q, self.glide, 1e-4);
            if band.gain_db.abs() < FLAT_DB && goal_gain == 0.0 {
                band.gain_db = 0.0;
            }
            if !band.is_active() {
                if was_active {
                    let memory =
                        &mut self.memory[index * self.channels..(index + 1) * self.channels];
                    memory.fill([0.0; 2]);
                }
                band.kind = target.kind;
                band.octave = goal_octave;
                band.ln_q = goal_ln_q;
                band.coefficients = Coefficients::IDENTITY;
            } else if moved || !was_active {
                band.coefficients = Coefficients::design(
                    band.kind,
                    band.octave.exp2(),
                    band.gain_db,
                    band.ln_q.exp(),
                    self.rate,
                );
            }
            let at_goal = band.kind == target.kind
                && band.gain_db == f64::from(target.gain_db)
                && band.octave == goal_octave
                && band.ln_q == goal_ln_q;
            settled &= at_goal || (!band.is_active() && target.gain_db == 0.0);
        }
        self.settled = settled;
    }
}

/// One step of a one-pole glide; snaps when within `snap`. Whether it moved.
fn glide(value: &mut f64, goal: f64, factor: f64, snap: f64) -> bool {
    if *value == goal {
        return false;
    }
    *value += (goal - *value) * factor;
    if (*value - goal).abs() < snap {
        *value = goal;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peaking_band_has_its_gain_at_its_centre() {
        let c = Coefficients::design(BandKind::Peaking, 1_000.0, 6.0, 1.0, 48_000.0);
        assert!((c.magnitude_db(1_000.0, 48_000.0) - 6.0).abs() < 1e-9);
    }

    #[test]
    fn a_band_at_zero_db_is_the_identity() {
        for kind in [BandKind::Peaking, BandKind::LowShelf, BandKind::HighShelf] {
            let c = Coefficients::design(kind, 1_000.0, 0.0, 0.7, 48_000.0);
            assert!((c.b0 - 1.0).abs() < 1e-12 && (c.b1 - c.a1).abs() < 1e-12);
            assert!((c.b2 - c.a2).abs() < 1e-12);
        }
    }

    #[test]
    fn too_many_bands_or_a_nan_are_refused() {
        let band = EqBand::peaking(100.0, 1.0, 1.0);
        let curve = EqCurve {
            preamp_db: 0.0,
            bands: vec![band; MAX_BANDS + 1],
        };
        assert!(EqParams::from_curve(&curve).is_err());
        let curve = EqCurve {
            preamp_db: f32::NAN,
            bands: vec![],
        };
        assert!(EqParams::from_curve(&curve).is_err());
    }
}
