//! The equaliser, measured on offline renders: its magnitude response against
//! the design, bit-transparency when flat or off, and no click when a band
//! moves.

mod common;

use aoide_audio::eq::{GAIN_LIMIT_DB, MAX_BANDS, built_in_presets};
use aoide_audio::{EntryId, EqBand, EqCurve, EqPreset, MediaSource, TrackSpec};
use common::*;

const AMPLITUDE: f64 = 0.25;

/// The level change `curve` makes to a steady tone at `freq`, in dB: a
/// second of tone rendered with and without it, measured after the glide.
fn measured_gain_db(curve: &EqCurve, freq: f64) -> f64 {
    let dir = Dir::new();
    let path = dir.path("tone.wav");
    write_wav(
        &path,
        RATE,
        2,
        &tone(freq, AMPLITUDE, RATE, 2 * RATE as usize, 2),
    );
    let level = |curve: Option<&EqCurve>| {
        let (mut engine, handle, _) = inline_engine(2);
        handle.set_equalizer(curve).unwrap();
        handle
            .load(
                TrackSpec::new(EntryId::fresh(), MediaSource::file(&path)),
                0,
                true,
            )
            .unwrap();
        let out = render(&mut engine, 2 * RATE as usize, 512);
        // Past the glide and the filters' own settling.
        rms(&out[RATE as usize * 2..], 2)
    };
    db(level(Some(curve)) / level(None))
}

const TEST_FREQUENCIES: [f64; 9] = [
    40.0, 100.0, 250.0, 700.0, 1_000.0, 2_500.0, 6_000.0, 12_000.0, 18_000.0,
];

fn matches_its_response(name: &str, curve: &EqCurve) {
    let mut worst: f64 = 0.0;
    for freq in TEST_FREQUENCIES {
        let want = curve.response_db(freq, RATE);
        let got = measured_gain_db(curve, freq);
        println!("{name}: {freq:>6} Hz designed {want:+7.3} dB, measured {got:+7.3} dB");
        worst = worst.max((got - want).abs());
        assert!(
            (got - want).abs() < 0.5,
            "{name} at {freq} Hz: designed {want:+.3} dB, measured {got:+.3} dB"
        );
    }
    println!("{name}: worst difference {worst:.4} dB");
}

#[test]
fn a_peaking_band_measures_as_designed() {
    let curve = EqCurve {
        preamp_db: 0.0,
        bands: vec![EqBand::peaking(1_000.0, 6.0, 1.0)],
    };
    // The design itself: the asked-for gain at the centre, nothing far away.
    assert!((curve.response_db(1_000.0, RATE) - 6.0).abs() < 1e-6);
    assert!(curve.response_db(40.0, RATE).abs() < 0.1);
    assert!(curve.response_db(18_000.0, RATE).abs() < 0.1);
    matches_its_response("peaking 1 kHz +6 dB Q 1", &curve);
}

#[test]
fn shelves_and_a_cut_measure_as_designed() {
    let low = EqCurve {
        preamp_db: 0.0,
        bands: vec![EqBand::low_shelf(200.0, 8.0, 0.707)],
    };
    // A shelf has its whole gain far below its frequency and half at it.
    assert!((low.response_db(20.0, RATE) - 8.0).abs() < 0.1);
    assert!((low.response_db(200.0, RATE) - 4.0).abs() < 1e-6);
    assert!(low.response_db(18_000.0, RATE).abs() < 0.1);
    matches_its_response("low shelf 200 Hz +8 dB", &low);

    let high_and_cut = EqCurve {
        preamp_db: -2.0,
        bands: vec![
            EqBand::high_shelf(5_000.0, -6.0, 0.707),
            EqBand::peaking(250.0, -9.0, 2.0),
        ],
    };
    assert!((high_and_cut.response_db(5_000.0, RATE) - (-2.0 - 3.0)).abs() < 0.1);
    assert!((high_and_cut.response_db(250.0, RATE) - (-2.0 - 9.0)).abs() < 0.1);
    matches_its_response(
        "high shelf 5 kHz -6 dB, 250 Hz -9 dB, preamp -2",
        &high_and_cut,
    );
}

#[test]
fn a_built_in_preset_measures_as_designed() {
    let loudness = built_in_presets()
        .into_iter()
        .find(|preset| preset.name == "Loudness")
        .expect("a Loudness preset");
    matches_its_response("Loudness preset", &loudness.curve);
}

#[test]
fn the_built_in_presets_are_there_and_never_push_past_full_scale() {
    let presets = built_in_presets();
    let names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
    println!("presets: {names:?}");
    for wanted in [
        "Flat",
        "Bass Boost",
        "Treble Boost",
        "Vocal",
        "Loudness",
        "Acoustic",
        "Electronic",
    ] {
        assert!(names.contains(&wanted), "no {wanted} preset");
    }
    for preset in &presets {
        let peak = preset.curve.peak_db(RATE);
        let peak_44 = preset.curve.peak_db(44_100);
        println!(
            "{:>14}: preamp {:+.1} dB, highest point {:+.2} dB",
            preset.name, preset.curve.preamp_db, peak
        );
        assert!(preset.curve.bands.len() <= MAX_BANDS);
        assert!(
            peak <= 0.05 && peak_44 <= 0.05,
            "{} peaks at {peak:+.2} dB",
            preset.name
        );
        assert!(preset.curve.preamp_db.abs() <= GAIN_LIMIT_DB);
    }
    assert!(presets[0].curve.is_flat(), "Flat is not flat");
}

#[test]
fn presets_are_plain_data_that_round_trip() {
    let preset = EqPreset {
        name: "Mine".into(),
        curve: EqCurve {
            preamp_db: -3.5,
            bands: vec![
                EqBand::low_shelf(90.0, 4.0, 0.707),
                EqBand::peaking(3_150.0, -2.5, 1.8),
            ],
        },
    };
    let json = serde_json::to_string(&preset).unwrap();
    println!("{json}");
    assert_eq!(serde_json::from_str::<EqPreset>(&json).unwrap(), preset);
    let presets = built_in_presets();
    let json = serde_json::to_string(&presets).unwrap();
    assert_eq!(
        serde_json::from_str::<Vec<EqPreset>>(&json).unwrap(),
        presets
    );
}

// Transparency

type Setup = Box<dyn Fn(&aoide_audio::EngineHandle)>;

/// The ramp through the engine with `setup` applied after the first
/// `before` frames, and the frames rendered after that.
fn ramp_render(setup: impl Fn(&aoide_audio::EngineHandle), before: usize) -> (Vec<f32>, Vec<f32>) {
    let dir = Dir::new();
    let path = dir.path("ramp.wav");
    let frames = 2 * RATE as usize;
    let samples = ramp(0, frames, 2);
    write_wav(&path, RATE, 2, &samples);
    let (mut engine, handle, _) = inline_engine(2);
    handle
        .load(
            TrackSpec::new(EntryId::fresh(), MediaSource::file(&path)),
            0,
            true,
        )
        .unwrap();
    let mut out = render(&mut engine, before, 480);
    setup(&handle);
    out.extend(render(&mut engine, frames - before, 480));
    (out, to_f32(&samples))
}

#[test]
fn off_and_flat_are_bit_transparent() {
    let flat = built_in_presets().remove(0).curve;
    let zeros = EqCurve {
        preamp_db: 0.0,
        bands: (0..MAX_BANDS)
            .map(|k| EqBand::peaking(100.0 * (k + 1) as f32, 0.0, 1.0))
            .collect(),
    };
    let cases: [(&str, Setup); 4] = [
        ("never set", Box::new(|_| {})),
        ("off", Box::new(|h| h.set_equalizer(None).unwrap())),
        (
            "Flat preset",
            Box::new(move |h| h.set_equalizer(Some(&flat)).unwrap()),
        ),
        (
            "ten bands at 0 dB",
            Box::new(move |h| h.set_equalizer(Some(&zeros)).unwrap()),
        ),
    ];
    for (name, setup) in cases {
        let (out, want) = ramp_render(setup, 0);
        assert_eq!(out, want, "{name} changed the samples");
        println!("{name}: {} frames, bit-identical", out.len() / 2);
    }
}

#[test]
fn turning_it_off_glides_back_to_bit_transparent() {
    let boost = built_in_presets()
        .into_iter()
        .find(|preset| preset.name == "Bass Boost")
        .unwrap()
        .curve;
    let dir = Dir::new();
    let path = dir.path("ramp.wav");
    let frames = 2 * RATE as usize;
    let samples = ramp(0, frames, 2);
    write_wav(&path, RATE, 2, &samples);
    let (mut engine, handle, _) = inline_engine(2);
    handle.set_equalizer(Some(&boost)).unwrap();
    handle
        .load(
            TrackSpec::new(EntryId::fresh(), MediaSource::file(&path)),
            0,
            true,
        )
        .unwrap();
    let on = render(&mut engine, RATE as usize / 2, 480);
    let want = to_f32(&samples);
    assert_ne!(on[..], want[..on.len()], "the boost did nothing");
    handle.set_equalizer(None).unwrap();
    let off = render(&mut engine, frames - RATE as usize / 2, 480);
    let off_from = on.len() / 2;
    // Exact from some frame on; how long the glide took.
    let first_exact = (0..off.len() / 2)
        .rev()
        .take_while(|&f| off[f * 2..f * 2 + 2] == want[(off_from + f) * 2..(off_from + f) * 2 + 2])
        .last()
        .expect("never exact again");
    println!(
        "off: bit-identical again {:.1} ms after the switch",
        first_exact as f64 * 1000.0 / f64::from(RATE)
    );
    assert!(
        first_exact < RATE as usize / 4,
        "still filtering after 250 ms"
    );
}

// Clicks

/// The biggest sample-to-sample step on channel 0.
fn biggest_step(samples: &[f32]) -> f32 {
    samples
        .iter()
        .step_by(2)
        .collect::<Vec<_>>()
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

#[test]
fn moving_a_band_does_not_click() {
    let dir = Dir::new();
    let path = dir.path("low.wav");
    let freq = 100.0;
    write_wav(
        &path,
        RATE,
        2,
        &tone(freq, AMPLITUDE, RATE, 2 * RATE as usize, 2),
    );
    let cut = EqCurve {
        preamp_db: 0.0,
        bands: vec![EqBand::peaking(100.0, -12.0, 1.0)],
    };
    let boost = EqCurve {
        preamp_db: 0.0,
        bands: vec![EqBand::peaking(100.0, 12.0, 1.0)],
    };
    let moved = EqCurve {
        preamp_db: 0.0,
        bands: vec![EqBand::low_shelf(400.0, 12.0, 0.707)],
    };
    let (mut engine, handle, _) = inline_engine(2);
    handle.set_equalizer(Some(&cut)).unwrap();
    handle
        .load(
            TrackSpec::new(EntryId::fresh(), MediaSource::file(&path)),
            0,
            true,
        )
        .unwrap();
    let before = render(&mut engine, RATE as usize / 2, 480);
    // Worst case: −12 to +12 dB in one go, mid-block.
    handle.set_equalizer(Some(&boost)).unwrap();
    let change = render(&mut engine, RATE as usize / 4, 480);
    // A band changing kind and frequency.
    handle.set_equalizer(Some(&moved)).unwrap();
    let kind = render(&mut engine, RATE as usize / 4, 480);
    let after = render(&mut engine, RATE as usize / 4, 480);

    // A sine's own biggest step is 2π f A / rate at its amplitude; the
    // loudest steady state bounds every step a click-free change can make.
    let steady = biggest_step(&before[RATE as usize / 4 * 2..]);
    let loud = biggest_step(&change[RATE as usize / 8 * 2..]);
    let during = biggest_step(&change[..RATE as usize / 8 * 2]);
    let kind_step = biggest_step(&kind);
    let after_step = biggest_step(&after);
    let bound = loud.max(after_step) * 1.05;
    println!(
        "steps: at -12 dB {steady:.5}, at +12 dB {loud:.5}, while gliding {during:.5}, \
         changing kind {kind_step:.5}, after {after_step:.5}; bound {bound:.5}"
    );
    assert!(during <= bound, "a step of {during} while gliding");
    assert!(
        kind_step <= bound,
        "a step of {kind_step} while changing kind"
    );
}
