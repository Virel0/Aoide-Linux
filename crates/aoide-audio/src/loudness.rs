//! How much to turn a track down so every record plays at the same level.
//!
//! The contract shared with the iOS app (`LoudnessNormalisation.swift`) and the
//! desktop app (`loudness.ts`); the sidecar measures the numbers. All three
//! must agree or one album plays at two levels on two devices.

/// ReplayGain 2.0's reference level. The one number every side must share.
pub const REFERENCE_LUFS: f64 = -18.0;

/// Decibels to apply, or `None` when there is nothing to do.
///
/// **Attenuation only**: a track at or below the reference is left alone,
/// peak and all. A master already over full scale is turned down until its
/// peak sits on 0 dBFS, but only among tracks this is turning down anyway.
///
/// Rounded to a hundredth of a decibel, half away from zero, as Swift's
/// `rounded()` does. (The desktop's `Math.round` rounds an exact half towards
/// positive infinity instead; no input in the parity table lands on a half.)
pub fn gain_db(loudness_lufs: Option<f64>, true_peak_dbfs: Option<f64>) -> Option<f64> {
    let loudness = loudness_lufs.filter(|value| value.is_finite())?;

    let gain = REFERENCE_LUFS - loudness;
    if gain >= 0.0 {
        return None;
    }

    let mut corrected = gain;
    if let Some(peak) = true_peak_dbfs.filter(|value| value.is_finite())
        && peak + corrected > 0.0
    {
        corrected = -peak;
    }

    let rounded = (corrected * 100.0).round() / 100.0;
    (rounded < 0.0).then_some(rounded)
}

/// A gain in decibels as a linear factor, never above one: the render path
/// only ever turns a track down.
pub fn linear_gain(db: Option<f64>) -> f32 {
    match db {
        Some(db) if db.is_finite() => 10f64.powf(db / 20.0).min(1.0) as f32,
        _ => 1.0,
    }
}
