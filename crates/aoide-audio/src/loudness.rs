//! How much to turn a track down so every record plays at the same level.
//!
//! The contract shared with the iOS app (`LoudnessNormalisation.swift`) and the
//! desktop app (`loudness.ts`); Melete measures the numbers. All three
//! must agree or one album plays at two levels on two devices.
//!
//! Level volume is one switch over two sources, as on the desktop: the
//! ReplayGain tags a file carries ([`ReplayGainTags`], read by the decoder)
//! and Melete's measurement ([`gain_db`]). A file with its own
//! ReplayGain plays by its tags and the measurement stands down, so nothing
//! is levelled twice; [`level_gain`] is that whole decision.

/// ReplayGain 2.0's reference level. The one number every side must share.
pub const REFERENCE_LUFS: f64 = -18.0;

/// Decibels to apply, or `None` when there is nothing to do.
///
/// **Attenuation only**: a track at or below the reference is left alone,
/// peak and all. A master already over full scale is turned down until its
/// peak sits on 0 dBFS, but only among tracks this is turning down anyway.
///
/// Rounded to a hundredth of a decibel, an exact half away from zero, as
/// Swift's `rounded()` does and the desktop now does too (the parity table's
/// −17.875 LUFS row is −0.125 dB exactly, and −0.13 on all three).
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

/// The ReplayGain a file's tags give it (`REPLAYGAIN_TRACK_GAIN`,
/// `_TRACK_PEAK`, `_ALBUM_GAIN`, `_ALBUM_PEAK`): gains in dB, peaks as a
/// linear fraction of full scale. Anything missing or unreadable is `None`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReplayGainTags {
    /// The track's gain, in dB.
    pub track_gain_db: Option<f64>,
    /// The track's peak, as a fraction of full scale.
    pub track_peak: Option<f64>,
    /// The album's gain, in dB.
    pub album_gain_db: Option<f64>,
    /// The album's peak, as a fraction of full scale.
    pub album_peak: Option<f64>,
}

impl ReplayGainTags {
    /// Whether the file carries a gain of its own (the desktop's
    /// `hasReplayGain`): used when Melete has not measured the track.
    pub fn is_present(&self) -> bool {
        self.track_gain_db.is_some_and(f64::is_finite)
            || self.album_gain_db.is_some_and(f64::is_finite)
    }

    /// The tags as a linear factor, or `None` when there is no gain (the
    /// desktop's `calculateReplayGain`).
    ///
    /// Album gain in preference to track gain: an album is what someone sat
    /// down to hear, and track gain flattens the quiet song meant to be quiet.
    /// Peak-limited always, so the peak stays at or under 0 dBFS: the factor
    /// is never more than one over the peak (album peak first, as for the
    /// gain), and with no peak tag never more than one. Unlike Melete's
    /// gain this can turn a track up, as far as its peak allows.
    pub fn linear_gain(&self) -> Option<f64> {
        let finite = |value: Option<f64>| value.filter(|v| v.is_finite());
        let gain = finite(self.album_gain_db).or(finite(self.track_gain_db))?;
        let peak = finite(self.album_peak)
            .or(finite(self.track_peak))
            .filter(|&peak| peak > 0.0)
            .unwrap_or(1.0);
        Some(10f64.powf(gain / 20.0).min(1.0 / peak))
    }
}

/// The factor a track plays at under Level volume.
///
/// Off: one, whatever the file or Melete says. On: Melete's
/// measured gain in dB when it has measured the track (from [`gain_db`],
/// only ever a cut; `Some(0.0)` for a measured track it leaves alone), else
/// the file's own ReplayGain, else one. Melete wins because it measured
/// every track the same way; tags are whatever each tagger wrote. `tags` is
/// `None` when the file's tags were not read.
pub fn level_gain(enabled: bool, tags: Option<&ReplayGainTags>, measured_db: Option<f64>) -> f32 {
    if !enabled {
        return 1.0;
    }
    match measured_db.filter(|db| db.is_finite()) {
        Some(db) => linear_gain(Some(db)),
        None => tags
            .filter(|tags| tags.is_present())
            .and_then(ReplayGainTags::linear_gain)
            .map_or(1.0, |gain| gain as f32),
    }
}

/// A ReplayGain value as tags write it: `-7.03 dB`, `+2.10 dB`, `0.988`.
pub(crate) fn parse_tag_number(text: &str) -> Option<f64> {
    let text = text.trim();
    let number = text
        .strip_suffix("dB")
        .or_else(|| text.strip_suffix("DB"))
        .or_else(|| text.strip_suffix("db"))
        .unwrap_or(text)
        .trim();
    number.parse::<f64>().ok().filter(|value| value.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_numbers_parse_as_written() {
        assert_eq!(parse_tag_number("-7.03 dB"), Some(-7.03));
        assert_eq!(parse_tag_number("+2.10 dB"), Some(2.1));
        assert_eq!(parse_tag_number(" 0.988000 "), Some(0.988));
        assert_eq!(parse_tag_number("-3dB"), Some(-3.0));
        assert_eq!(parse_tag_number("loud"), None);
        assert_eq!(parse_tag_number("NaN"), None);
    }
}
