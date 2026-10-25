//! The compressor, measured on offline renders: its static curve, its attack
//! and release time constants, and bit-transparency when off.

mod common;

use aoide_audio::compressor::built_in_presets;
use aoide_audio::{CompressorPreset, CompressorSettings, EntryId, MediaSource, TrackSpec};
use common::*;

/// A steady (DC) sample at `db` dBFS.
fn dc_value(db: f64) -> i32 {
    (10f64.powf(db / 20.0) * 32_767.0).round() as i32
}

/// The level a DC sample decodes to, in dBFS.
fn dc_level(value: i32) -> f64 {
    20.0 * (f64::from(value) / f64::from(SCALE)).log10()
}

/// Steady steps of DC, `frames` each, on both channels.
fn steps(levels: &[i32], frames: usize) -> Vec<i32> {
    levels
        .iter()
        .flat_map(|&value| std::iter::repeat_n(value, frames * 2))
        .collect()
}

fn compressed(samples: &[i32], settings: Option<&CompressorSettings>) -> Vec<f32> {
    let dir = Dir::new();
    let path = dir.path("dc.wav");
    write_wav(&path, RATE, 2, samples);
    let (mut engine, handle, _) = inline_engine(2);
    handle.set_compressor(settings).unwrap();
    handle
        .load(
            TrackSpec::new(EntryId::fresh(), MediaSource::file(&path)),
            0,
            true,
        )
        .unwrap();
    render(&mut engine, samples.len() / 2, 480)
}

/// The independent statement of the curve: a straight line of slope 1 below
/// the knee, of slope 1/ratio above it, joined by the quadratic knee.
fn designed_output_db(input: f64, threshold: f64, ratio: f64, knee: f64, makeup: f64) -> f64 {
    let over = input - threshold;
    let out = if over <= -knee / 2.0 {
        input
    } else if over >= knee / 2.0 {
        threshold + over / ratio
    } else {
        input + (1.0 / ratio - 1.0) * (over + knee / 2.0).powi(2) / (2.0 * knee)
    };
    out + makeup
}

#[test]
fn the_static_curve_measures_as_designed() {
    let settings = CompressorSettings {
        threshold_db: -24.0,
        ratio: 4.0,
        attack_ms: 1.0,
        release_ms: 20.0,
        makeup_db: 3.0,
        knee_db: 8.0,
    };
    // −60 to 0 dBFS in 3 dB steps, the knee in 1 dB steps.
    let mut levels: Vec<f64> = (0..=20).map(|k| -60.0 + 3.0 * f64::from(k)).collect();
    levels.extend((-28..=-20).map(f64::from));
    levels.sort_by(f64::total_cmp);
    levels.dedup();
    let values: Vec<i32> = levels.iter().map(|&db| dc_value(db)).collect();
    let step = RATE as usize / 4;
    let out = compressed(&steps(&values, step), Some(&settings));

    let mut worst: f64 = 0.0;
    for (index, &value) in values.iter().enumerate() {
        let input = dc_level(value);
        // The step's last frame: settled (12 release time constants in).
        let got = 20.0 * f64::from(out[((index + 1) * step - 1) * 2]).log10();
        let want = designed_output_db(input, -24.0, 4.0, 8.0, 3.0);
        assert!(
            (settings.output_db(input) - want).abs() < 1e-9,
            "the settings' own curve disagrees at {input:.2} dB"
        );
        println!("in {input:+7.2} dBFS: designed {want:+7.3}, measured {got:+7.3}");
        worst = worst.max((got - want).abs());
        assert!(
            (got - want).abs() < 0.05,
            "at {input:.2} dBFS: designed {want:+.3}, measured {got:+.3}"
        );
    }
    println!("static curve: worst difference {worst:.4} dB");
}

/// Frames from `from` until the reduction (in dB, from the output of a DC
/// input) first passes `fraction` of the way from `start_db` to `end_db`.
fn time_to(out: &[f32], input: f32, from: usize, start_db: f64, end_db: f64, fraction: f64) -> f64 {
    let goal = start_db + (end_db - start_db) * fraction;
    let rising = end_db > start_db;
    let frames = (from..out.len() / 2)
        .position(|frame| {
            let reduction = -20.0 * f64::from(out[frame * 2] / input).log10();
            if rising {
                reduction >= goal
            } else {
                reduction <= goal
            }
        })
        .expect("never got there");
    frames as f64
}

#[test]
fn attack_and_release_have_their_time_constants() {
    let settings = CompressorSettings {
        threshold_db: -30.0,
        ratio: 4.0,
        attack_ms: 10.0,
        release_ms: 100.0,
        makeup_db: 0.0,
        knee_db: 0.0,
    };
    let (quiet, loud) = (dc_value(-40.0), dc_value(-10.0));
    let step = RATE as usize / 2;
    let mut samples = steps(&[quiet], step);
    samples.extend(steps(&[loud], step));
    samples.extend(steps(&[quiet], 3 * step));
    let out = compressed(&samples, Some(&settings));

    let full = settings.reduction_db(dc_level(loud));
    println!("a step from -40 to -10 dBFS: {full:.3} dB of reduction when settled");
    let (quiet_in, loud_in) = (quiet as f32 / SCALE, loud as f32 / SCALE);
    assert!(
        out[..step * 2].iter().all(|&s| s == quiet_in),
        "below the threshold, the samples changed"
    );
    let rate = f64::from(RATE);
    let attack = time_to(&out, loud_in, step, 0.0, full, 1.0 - (-1f64).exp());
    let attack_ms = attack * 1000.0 / rate;
    // The reduction held at the end of the loud step, and released after.
    let held = -20.0 * f64::from(out[(2 * step - 1) * 2] / loud_in).log10();
    let release = time_to(&out, quiet_in, 2 * step, held, 0.0, 1.0 - (-1f64).exp());
    let release_ms = release * 1000.0 / rate;
    println!(
        "attack: 63.2 % of the reduction after {attack_ms:.3} ms (asked 10 ms); \
         release: 63.2 % let go after {release_ms:.3} ms (asked 100 ms); held {held:.3} dB"
    );
    // Within a frame of the time constant, plus the frame the step lands on.
    assert!((attack_ms - 10.0).abs() <= 2.0 * 1000.0 / rate);
    assert!((release_ms - 100.0).abs() <= 2.0 * 1000.0 / rate);
    assert!((held - full).abs() < 0.01);
    // And the tail: back to bit-exact once it has let go (to within
    // 0.001 dB, where the release stops: ln(15 / 0.001) = 9.6 time
    // constants, under a second).
    let last = out[(5 * step - 1) * 2];
    assert_eq!(last, quiet_in, "still reducing at the end");
}

#[test]
fn off_is_bit_transparent_and_turning_it_off_lets_go() {
    let dir = Dir::new();
    let path = dir.path("ramp.wav");
    let frames = 2 * RATE as usize;
    let samples = ramp(0, frames, 2);
    write_wav(&path, RATE, 2, &samples);
    let want = to_f32(&samples);

    for (name, settings) in [
        ("never set", None),
        ("off", Some(None)),
        (
            "on, nothing over the threshold, no makeup",
            Some(Some(CompressorSettings {
                threshold_db: 0.0,
                knee_db: 0.0,
                makeup_db: 0.0,
                ..Default::default()
            })),
        ),
    ] {
        let (mut engine, handle, _) = inline_engine(2);
        if let Some(settings) = settings {
            handle.set_compressor(settings.as_ref()).unwrap();
        }
        handle
            .load(
                TrackSpec::new(EntryId::fresh(), MediaSource::file(&path)),
                0,
                true,
            )
            .unwrap();
        let out = render(&mut engine, frames, 480);
        assert_eq!(out, want, "{name} changed the samples");
        println!("{name}: bit-identical");
    }

    // Heavy, then off: exact again once the reduction has released.
    let heavy = built_in_presets()
        .into_iter()
        .find(|p| p.name == "Heavy")
        .unwrap()
        .settings;
    let (mut engine, handle, _) = inline_engine(2);
    handle.set_compressor(Some(&heavy)).unwrap();
    handle
        .load(
            TrackSpec::new(EntryId::fresh(), MediaSource::file(&path)),
            0,
            true,
        )
        .unwrap();
    let on = render(&mut engine, RATE as usize / 2, 480);
    assert_ne!(on[..], want[..on.len()], "Heavy did nothing");
    handle.set_compressor(None).unwrap();
    let off = render(&mut engine, frames - RATE as usize / 2, 480);
    let at = on.len();
    let exact_from = (0..off.len() / 2)
        .rev()
        .take_while(|&f| off[f * 2..f * 2 + 2] == want[at + f * 2..at + f * 2 + 2])
        .last()
        .expect("never exact again");
    println!(
        "off after Heavy: bit-identical {:.1} ms later (release {} ms)",
        exact_from as f64 * 1000.0 / f64::from(RATE),
        heavy.release_ms
    );
    assert!(exact_from < RATE as usize);
}

#[test]
fn makeup_changes_glide() {
    let dir = Dir::new();
    let path = dir.path("tone.wav");
    write_wav(&path, RATE, 2, &tone(100.0, 0.1, RATE, RATE as usize, 2));
    let quiet = CompressorSettings {
        makeup_db: 0.0,
        ..Default::default()
    };
    let loud = CompressorSettings {
        makeup_db: 18.0,
        ..Default::default()
    };
    let (mut engine, handle, _) = inline_engine(2);
    handle.set_compressor(Some(&quiet)).unwrap();
    handle
        .load(
            TrackSpec::new(EntryId::fresh(), MediaSource::file(&path)),
            0,
            true,
        )
        .unwrap();
    render(&mut engine, RATE as usize / 4, 480);
    handle.set_compressor(Some(&loud)).unwrap();
    let out = render(&mut engine, RATE as usize / 2, 480);
    let channel: Vec<f32> = out.iter().step_by(2).copied().collect();
    let step = |s: &[f32]| {
        s.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max)
    };
    let during = step(&channel[..RATE as usize / 8]);
    let after = step(&channel[RATE as usize / 4..]);
    println!("makeup 0 -> 18 dB: biggest step while gliding {during:.5}, after {after:.5}");
    assert!(during <= after * 1.05);
}

#[test]
fn presets_are_there_and_round_trip() {
    let presets = built_in_presets();
    for preset in &presets {
        assert_eq!(preset.settings.validated().unwrap(), preset.settings);
        println!("{}: {:?}", preset.name, preset.settings);
    }
    assert!(presets.len() >= 3);
    let json = serde_json::to_string(&presets).unwrap();
    assert_eq!(
        serde_json::from_str::<Vec<CompressorPreset>>(&json).unwrap(),
        presets
    );
}
