//! The sound settings past Round 1, kept in the config file: the equaliser
//! and the compressor (with the listener's own equaliser presets), and the
//! output device.
//!
//! Plain data and pure functions; what applies them is the equaliser screen
//! (`screens::equalizer`) and `playback::sound`.

use aoide_audio::compressor::{self, CompressorPreset, CompressorSettings};
use aoide_audio::eq::{self, EqBand, EqCurve, EqPreset};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SoundSettings {
    pub equalizer: EqualizerSettings,
    pub compressor: CompressorChoice,
    /// The output device: the system's default unless one was chosen.
    pub device: aoide_audio::DeviceChoice,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EqualizerSettings {
    /// Off by default: the sound as it is.
    pub enabled: bool,
    /// The preset the curve is, or `None` once it has been changed by hand.
    pub preset: Option<String>,
    pub curve: EqCurve,
    /// The listener's own presets, in the order saved.
    pub user_presets: Vec<EqPreset>,
}

impl Default for EqualizerSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            preset: Some("Flat".to_owned()),
            curve: EqCurve::flat(),
            user_presets: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CompressorChoice {
    pub enabled: bool,
    /// The built-in preset the settings are, or `None` when changed by hand.
    pub preset: Option<String>,
    pub settings: CompressorSettings,
}

impl Default for CompressorChoice {
    fn default() -> Self {
        Self {
            enabled: false,
            preset: Some("Moderate".to_owned()),
            settings: compressor::built_in_presets()
                .into_iter()
                .find(|p| p.name == "Moderate")
                .map(|p| p.settings)
                .unwrap_or_default(),
        }
    }
}

/// One line of the output picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceOption {
    pub label: String,
    pub choice: aoide_audio::DeviceChoice,
}

impl std::fmt::Display for DeviceOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// The output picker's lines: the system default (naming the device it is
/// now, when the list says), then every listed device, then the chosen one
/// if it isn't connected, so the picker still shows what was chosen.
pub fn device_options(
    listed: &[aoide_audio::OutputDevice],
    chosen: &aoide_audio::DeviceChoice,
) -> Vec<DeviceOption> {
    use aoide_audio::DeviceChoice;
    let default = listed.iter().find(|d| d.is_default);
    let mut options = vec![DeviceOption {
        label: match default {
            Some(device) => format!("System Default ({})", device.name),
            None => "System Default".to_owned(),
        },
        choice: DeviceChoice::FollowDefault,
    }];
    options.extend(listed.iter().map(|device| DeviceOption {
        label: device.name.clone(),
        choice: DeviceChoice::from(device),
    }));
    if let DeviceChoice::Device { name, .. } = chosen
        && !listed.iter().any(|d| same_device(chosen, d))
    {
        options.push(DeviceOption {
            label: format!("{name} (not connected)"),
            choice: chosen.clone(),
        });
    }
    options
}

/// Whether `choice` names `device`: by id, or by name when the id has
/// changed (as `aoide_audio::DeviceChoice` finds it).
pub fn same_device(choice: &aoide_audio::DeviceChoice, device: &aoide_audio::OutputDevice) -> bool {
    match choice {
        aoide_audio::DeviceChoice::FollowDefault => false,
        aoide_audio::DeviceChoice::Device { id, name } => *id == device.id || *name == device.name,
    }
}

/// The picker's line for what is chosen.
pub fn chosen_option(
    options: &[DeviceOption],
    chosen: &aoide_audio::DeviceChoice,
) -> Option<DeviceOption> {
    options
        .iter()
        .find(|o| o.choice == *chosen)
        .or_else(|| {
            let aoide_audio::DeviceChoice::Device { id, name } = chosen else {
                return None;
            };
            options.iter().find(|o| match &o.choice {
                aoide_audio::DeviceChoice::Device { id: i, name: n } => i == id || n == name,
                aoide_audio::DeviceChoice::FollowDefault => false,
            })
        })
        .cloned()
}

/// What the picker shows when the curve is no preset.
pub const CUSTOM: &str = "Custom";

impl SoundSettings {
    /// Every value held to its range, the way a hand-edited config file is
    /// read. A curve the engine would refuse (too many bands, a number that
    /// isn't one) is flattened rather than kept.
    pub fn sanitised(mut self) -> Self {
        let curve = &mut self.equalizer.curve;
        let finite = curve.preamp_db.is_finite()
            && curve
                .bands
                .iter()
                .all(|b| b.freq_hz.is_finite() && b.gain_db.is_finite() && b.q.is_finite());
        if !finite || curve.bands.len() > eq::MAX_BANDS {
            *curve = EqCurve::flat();
            self.equalizer.preset = None;
        }
        self.equalizer
            .user_presets
            .retain(|p| !p.name.trim().is_empty() && p.curve.bands.len() <= eq::MAX_BANDS);
        match self.compressor.settings.validated() {
            Ok(settings) => self.compressor.settings = settings,
            Err(_) => self.compressor = CompressorChoice::default(),
        }
        self
    }
}

impl EqualizerSettings {
    /// What the engine gets: the curve while on, nothing while off.
    pub fn active(&self) -> Option<&EqCurve> {
        self.enabled.then_some(&self.curve)
    }

    /// Every preset by name, the built-in ones first.
    pub fn preset_names(&self) -> Vec<String> {
        eq::built_in_presets()
            .into_iter()
            .map(|p| p.name)
            .chain(self.user_presets.iter().map(|p| p.name.clone()))
            .collect()
    }

    /// The picker's label for the curve as it is.
    pub fn label(&self) -> String {
        self.preset.clone().unwrap_or_else(|| CUSTOM.to_owned())
    }

    /// Takes a preset's curve. Whether the name was found.
    pub fn choose(&mut self, name: &str) -> bool {
        let found = eq::built_in_presets()
            .into_iter()
            .chain(self.user_presets.iter().cloned())
            .find(|p| p.name == name);
        match found {
            Some(preset) => {
                self.curve = editable(&preset.curve);
                self.preset = Some(preset.name);
                true
            }
            None => false,
        }
    }

    /// Whether `name` is one of the listener's own presets (which can be
    /// deleted), not a built-in one.
    pub fn is_user_preset(&self, name: &str) -> bool {
        self.user_presets.iter().any(|p| p.name == name)
    }

    /// The preamp, moved by hand.
    pub fn set_preamp(&mut self, db: f32) {
        self.curve = editable(&self.curve);
        self.curve.preamp_db = db.clamp(-eq::GAIN_LIMIT_DB, eq::GAIN_LIMIT_DB);
        self.preset = None;
    }

    /// One band's gain, moved by hand.
    pub fn set_band(&mut self, index: usize, db: f32) {
        self.curve = editable(&self.curve);
        if let Some(band) = self.curve.bands.get_mut(index) {
            band.gain_db = db.clamp(-eq::GAIN_LIMIT_DB, eq::GAIN_LIMIT_DB);
            self.preset = None;
        }
    }

    /// Saves the curve as the listener's preset `name`, replacing one of
    /// theirs with that name. A built-in name is refused, so the built-in
    /// presets always mean the same.
    pub fn save_preset(&mut self, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Give the preset a name.".to_owned());
        }
        if name.eq_ignore_ascii_case(CUSTOM)
            || eq::built_in_presets()
                .iter()
                .any(|p| p.name.eq_ignore_ascii_case(name))
        {
            return Err(format!(
                "\u{201c}{name}\u{201d} is a built-in preset's name."
            ));
        }
        let preset = EqPreset {
            name: name.to_owned(),
            curve: self.curve.clone(),
        };
        match self.user_presets.iter_mut().find(|p| p.name == name) {
            Some(existing) => *existing = preset,
            None => self.user_presets.push(preset),
        }
        self.preset = Some(name.to_owned());
        Ok(())
    }

    /// Deletes one of the listener's presets; the curve stays as it is.
    pub fn delete_preset(&mut self, name: &str) {
        self.user_presets.retain(|p| p.name != name);
        if self.preset.as_deref() == Some(name) {
            self.preset = None;
        }
    }
}

/// A curve with a slider for every band: a flat curve with no bands gets the
/// ten bands the built-in presets use, at 0 dB.
pub fn editable(curve: &EqCurve) -> EqCurve {
    if !curve.bands.is_empty() {
        return curve.clone();
    }
    EqCurve {
        preamp_db: curve.preamp_db,
        bands: eq::PRESET_FREQUENCIES_HZ
            .iter()
            .map(|&freq| EqBand::peaking(freq, 0.0, eq::PRESET_Q))
            .collect(),
    }
}

/// The curve's response at `count` points from 20 Hz to 20 kHz (or just
/// under half the rate), log-spaced, as `(frequency, dB)`: what the screen
/// draws.
pub fn curve_points(curve: &EqCurve, sample_rate: u32, count: usize) -> Vec<(f64, f64)> {
    let low = f64::from(eq::MIN_FREQ_HZ);
    let high = f64::from(eq::MAX_FREQ_HZ).min(f64::from(sample_rate) * 0.45);
    let count = count.max(2);
    (0..count)
        .map(|i| {
            let freq = low * (high / low).powf(i as f64 / (count - 1) as f64);
            (freq, curve.response_db(freq, sample_rate))
        })
        .collect()
}

/// `1 kHz`, `125 Hz`: a band's label.
pub fn frequency_label(hz: f32) -> String {
    if hz >= 1_000.0 {
        let k = hz / 1_000.0;
        if (k - k.round()).abs() < 0.05 {
            format!("{}k", k.round())
        } else {
            format!("{k:.1}k")
        }
    } else {
        format!("{}", hz.round())
    }
}

/// `+3.5 dB`, `0 dB`, `−2 dB`.
pub fn gain_label(db: f32) -> String {
    let rounded = (db * 2.0).round() / 2.0;
    if rounded == 0.0 {
        "0 dB".to_owned()
    } else if rounded > 0.0 {
        format!("+{rounded} dB")
    } else {
        format!("\u{2212}{} dB", -rounded)
    }
}

impl CompressorChoice {
    /// What the engine gets.
    pub fn active(&self) -> Option<&CompressorSettings> {
        self.enabled.then_some(&self.settings)
    }

    pub fn label(&self) -> String {
        self.preset.clone().unwrap_or_else(|| CUSTOM.to_owned())
    }

    pub fn preset_names() -> Vec<String> {
        compressor::built_in_presets()
            .into_iter()
            .map(|p| p.name)
            .collect()
    }

    pub fn choose(&mut self, name: &str) -> bool {
        let found: Option<CompressorPreset> = compressor::built_in_presets()
            .into_iter()
            .find(|p| p.name == name);
        match found {
            Some(preset) => {
                self.settings = preset.settings;
                self.preset = Some(preset.name);
                true
            }
            None => false,
        }
    }

    /// One control, moved by hand, held to its range.
    pub fn set(&mut self, field: CompressorField, value: f32) {
        let (low, high) = field.range();
        let value = value.clamp(low, high);
        let s = &mut self.settings;
        match field {
            CompressorField::Threshold => s.threshold_db = value,
            CompressorField::Ratio => s.ratio = value,
            CompressorField::Attack => s.attack_ms = value,
            CompressorField::Release => s.release_ms = value,
            CompressorField::Makeup => s.makeup_db = value,
            CompressorField::Knee => s.knee_db = value,
        }
        self.preset = None;
    }
}

/// The compressor's controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressorField {
    Threshold,
    Ratio,
    Attack,
    Release,
    Makeup,
    Knee,
}

impl CompressorField {
    pub const ALL: [CompressorField; 6] = [
        CompressorField::Threshold,
        CompressorField::Ratio,
        CompressorField::Attack,
        CompressorField::Release,
        CompressorField::Makeup,
        CompressorField::Knee,
    ];

    pub fn title(self) -> &'static str {
        match self {
            CompressorField::Threshold => "Threshold",
            CompressorField::Ratio => "Ratio",
            CompressorField::Attack => "Attack",
            CompressorField::Release => "Release",
            CompressorField::Makeup => "Makeup Gain",
            CompressorField::Knee => "Knee",
        }
    }

    pub fn range(self) -> (f32, f32) {
        match self {
            CompressorField::Threshold => compressor::THRESHOLD_RANGE_DB,
            CompressorField::Ratio => compressor::RATIO_RANGE,
            CompressorField::Attack => compressor::ATTACK_RANGE_MS,
            CompressorField::Release => compressor::RELEASE_RANGE_MS,
            CompressorField::Makeup => compressor::MAKEUP_RANGE_DB,
            CompressorField::Knee => compressor::KNEE_RANGE_DB,
        }
    }

    pub fn step(self) -> f32 {
        match self {
            CompressorField::Ratio => 0.5,
            CompressorField::Attack => 0.5,
            CompressorField::Release => 10.0,
            _ => 0.5,
        }
    }

    pub fn value(self, s: &CompressorSettings) -> f32 {
        match self {
            CompressorField::Threshold => s.threshold_db,
            CompressorField::Ratio => s.ratio,
            CompressorField::Attack => s.attack_ms,
            CompressorField::Release => s.release_ms,
            CompressorField::Makeup => s.makeup_db,
            CompressorField::Knee => s.knee_db,
        }
    }

    pub fn label(self, value: f32) -> String {
        match self {
            CompressorField::Threshold => format!("{value:.1} dBFS").replace('-', "\u{2212}"),
            CompressorField::Ratio => format!("{value:.1} : 1"),
            CompressorField::Attack => format!("{value:.1} ms"),
            CompressorField::Release => format!("{value:.0} ms"),
            CompressorField::Makeup | CompressorField::Knee => format!("{value:.1} dB"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_are_off_until_turned_on() {
        let sound = SoundSettings::default();
        assert!(sound.equalizer.active().is_none());
        assert!(sound.compressor.active().is_none());
        assert_eq!(sound.equalizer.label(), "Flat");
        assert_eq!(sound.compressor.label(), "Moderate");
    }

    #[test]
    fn a_flat_curve_gets_ten_bands_to_move() {
        let curve = editable(&EqCurve::flat());
        assert_eq!(curve.bands.len(), 10);
        assert!(curve.is_flat());
        let freqs: Vec<f32> = curve.bands.iter().map(|b| b.freq_hz).collect();
        assert_eq!(freqs, eq::PRESET_FREQUENCIES_HZ);
    }

    #[test]
    fn choosing_a_preset_takes_its_curve_and_moving_a_band_makes_it_custom() {
        let mut settings = EqualizerSettings::default();
        assert!(settings.choose("Rock"));
        let rock = eq::built_in_presets()
            .into_iter()
            .find(|p| p.name == "Rock")
            .unwrap();
        assert_eq!(settings.curve, rock.curve);
        assert_eq!(settings.label(), "Rock");
        settings.set_band(0, 5.0);
        assert_eq!(settings.label(), CUSTOM);
        assert_eq!(settings.curve.bands[0].gain_db, 5.0);
        settings.set_band(3, 40.0);
        assert_eq!(settings.curve.bands[3].gain_db, eq::GAIN_LIMIT_DB);
        assert!(!settings.choose("No Such Preset"));
    }

    #[test]
    fn saving_a_preset_keeps_it_by_name_and_refuses_a_built_in_name() {
        let mut settings = EqualizerSettings::default();
        settings.set_band(2, 4.0);
        settings.set_preamp(-4.0);
        assert_eq!(settings.label(), CUSTOM);
        assert!(settings.save_preset("  ").is_err());
        assert!(settings.save_preset("rock").is_err());
        assert!(settings.save_preset("Custom").is_err());
        settings.save_preset(" Desk Speakers ").unwrap();
        assert_eq!(settings.label(), "Desk Speakers");
        assert_eq!(settings.user_presets.len(), 1);
        assert_eq!(settings.user_presets[0].curve.preamp_db, -4.0);
        assert!(settings.is_user_preset("Desk Speakers"));
        assert!(
            settings
                .preset_names()
                .ends_with(&["Desk Speakers".to_owned()])
        );

        // The same name again replaces it.
        settings.set_band(2, 6.0);
        settings.save_preset("Desk Speakers").unwrap();
        assert_eq!(settings.user_presets.len(), 1);
        assert_eq!(settings.user_presets[0].curve.bands[2].gain_db, 6.0);

        settings.choose("Flat");
        assert!(settings.choose("Desk Speakers"));
        assert_eq!(settings.curve.bands[2].gain_db, 6.0);

        settings.delete_preset("Desk Speakers");
        assert!(settings.user_presets.is_empty());
        assert_eq!(settings.label(), CUSTOM);
        assert_eq!(settings.curve.bands[2].gain_db, 6.0);
    }

    #[test]
    fn the_drawn_curve_is_the_engines_response() {
        let mut settings = EqualizerSettings::default();
        settings.choose("Bass Boost");
        let points = curve_points(&settings.curve, 48_000, 64);
        assert_eq!(points.len(), 64);
        assert!((points[0].0 - 20.0).abs() < 1e-9);
        assert!((points[63].0 - 20_000.0).abs() < 1e-6);
        for (freq, db) in &points {
            assert_eq!(*db, settings.curve.response_db(*freq, 48_000));
        }
        // Bass up, treble back to the preamp.
        assert!(points[5].1 > points[60].1);
        // At 32 kHz the top stops short of Nyquist.
        let low_rate = curve_points(&settings.curve, 32_000, 8);
        assert!(low_rate.last().unwrap().0 <= 14_400.0 + 1e-6);
    }

    #[test]
    fn the_compressor_moves_by_hand_within_its_ranges() {
        let mut choice = CompressorChoice::default();
        assert!(choice.choose("Late Night"));
        assert_eq!(choice.settings.threshold_db, -32.0);
        choice.set(CompressorField::Ratio, 50.0);
        assert_eq!(choice.settings.ratio, 20.0);
        assert_eq!(choice.label(), CUSTOM);
        for field in CompressorField::ALL {
            let (low, high) = field.range();
            choice.set(field, low - 1.0);
            assert_eq!(field.value(&choice.settings), low);
            choice.set(field, high + 1.0);
            assert_eq!(field.value(&choice.settings), high);
        }
        assert!(choice.settings.validated().is_ok());
    }

    #[test]
    fn a_broken_config_is_read_back_sane() {
        let mut sound = SoundSettings::default();
        sound.equalizer.curve.preamp_db = f32::NAN;
        sound.equalizer.preset = Some("Rock".into());
        sound.compressor.settings.ratio = f32::INFINITY;
        sound.equalizer.user_presets.push(EqPreset {
            name: " ".into(),
            curve: EqCurve::flat(),
        });
        let sound = sound.sanitised();
        assert_eq!(sound.equalizer.curve, EqCurve::flat());
        assert_eq!(sound.equalizer.preset, None);
        assert!(sound.equalizer.user_presets.is_empty());
        assert_eq!(sound.compressor, CompressorChoice::default());

        let mut sound = SoundSettings::default();
        sound.compressor.settings.threshold_db = -500.0;
        assert_eq!(sound.sanitised().compressor.settings.threshold_db, -60.0);
    }

    fn device(id: &str, name: &str, is_default: bool) -> aoide_audio::OutputDevice {
        aoide_audio::OutputDevice {
            id: id.into(),
            name: name.into(),
            is_default,
        }
    }

    #[test]
    fn the_output_picker_lists_the_default_then_every_device() {
        use aoide_audio::DeviceChoice;
        let listed = [
            device("alsa:pipewire", "PipeWire", true),
            device("alsa:hw:CARD=USB", "USB DAC", false),
        ];
        let options = device_options(&listed, &DeviceChoice::FollowDefault);
        let labels: Vec<&str> = options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(labels, ["System Default (PipeWire)", "PipeWire", "USB DAC"]);
        assert_eq!(
            chosen_option(&options, &DeviceChoice::FollowDefault)
                .unwrap()
                .label,
            "System Default (PipeWire)"
        );
        let usb = DeviceChoice::from(&listed[1]);
        assert_eq!(chosen_option(&options, &usb).unwrap().label, "USB DAC");
    }

    #[test]
    fn a_renumbered_card_is_found_by_name_and_a_missing_one_still_shown() {
        use aoide_audio::DeviceChoice;
        let listed = [device("alsa:hw:CARD=2", "USB DAC", false)];
        let renumbered = DeviceChoice::Device {
            id: "alsa:hw:CARD=1".into(),
            name: "USB DAC".into(),
        };
        let options = device_options(&listed, &renumbered);
        assert_eq!(options.len(), 2);
        assert_eq!(
            chosen_option(&options, &renumbered).unwrap().label,
            "USB DAC"
        );

        let gone = DeviceChoice::Device {
            id: "alsa:hw:CARD=9".into(),
            name: "Headphones".into(),
        };
        let options = device_options(&listed, &gone);
        assert_eq!(options.last().unwrap().label, "Headphones (not connected)");
        assert_eq!(
            chosen_option(&options, &gone).unwrap().label,
            "Headphones (not connected)"
        );
        // Nothing listed at all: the default without a name.
        assert_eq!(
            device_options(&[], &DeviceChoice::FollowDefault)[0].label,
            "System Default"
        );
    }

    #[test]
    fn labels_read_plainly() {
        assert_eq!(frequency_label(32.0), "32");
        assert_eq!(frequency_label(1_000.0), "1k");
        assert_eq!(frequency_label(16_000.0), "16k");
        assert_eq!(frequency_label(2_500.0), "2.5k");
        assert_eq!(gain_label(0.1), "0 dB");
        assert_eq!(gain_label(3.4), "+3.5 dB");
        assert_eq!(gain_label(-2.0), "\u{2212}2 dB");
        assert_eq!(CompressorField::Threshold.label(-20.0), "\u{2212}20.0 dBFS");
        assert_eq!(CompressorField::Ratio.label(3.0), "3.0 : 1");
    }

    #[test]
    fn the_settings_round_trip_through_json() {
        let mut sound = SoundSettings::default();
        sound.equalizer.enabled = true;
        sound.equalizer.choose("Jazz");
        sound.equalizer.save_preset("Mine").unwrap();
        sound.compressor.enabled = true;
        sound.device = aoide_audio::DeviceChoice::Device {
            id: "alsa:hw:CARD=USB".into(),
            name: "USB DAC".into(),
        };
        let json = serde_json::to_string(&sound).unwrap();
        let back: SoundSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, sound);
        let empty: SoundSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, SoundSettings::default());
    }
}
