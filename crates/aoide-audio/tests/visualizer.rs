//! The visualizer tap, measured on offline renders: band energies for
//! generated tones, the window aligned to the heard frame rather than the
//! rendered one, the waveform, and the fall smoothing.

mod common;

use aoide_audio::{EngineHandle, EntryId, MediaSource, TrackSpec, Visualizer, VisualizerConfig};
use common::*;

fn load(handle: &EngineHandle, path: &std::path::Path) {
    handle
        .load(
            TrackSpec::new(EntryId::fresh(), MediaSource::file(path)),
            0,
            true,
        )
        .unwrap();
}

fn band_of(visualizer: &Visualizer, freq: f32) -> usize {
    visualizer
        .band_edges()
        .windows(2)
        .position(|edge| edge[0] <= freq && freq < edge[1])
        .expect("in a band")
}

fn centre(visualizer: &Visualizer, band: usize) -> f32 {
    let edges = visualizer.band_edges();
    (edges[band] * edges[band + 1]).sqrt()
}

#[test]
fn a_tone_lights_its_own_band_at_its_level() {
    let (_e, handle, _) = inline_engine(2);
    let probe = Visualizer::new(handle.visualizer_tap(), VisualizerConfig::default());
    for band in [6, 12, 18, 24, 29] {
        let freq = centre(&probe, band);
        let dir = Dir::new();
        let path = dir.path("tone.wav");
        write_wav(
            &path,
            RATE,
            2,
            &tone(f64::from(freq), 0.5, RATE, RATE as usize, 2),
        );
        let (mut engine, handle, _) = inline_engine(2);
        let mut visualizer = Visualizer::new(handle.visualizer_tap(), VisualizerConfig::default());
        load(&handle, &path);
        render(&mut engine, RATE as usize / 2, 480);
        let frame = visualizer.read();
        let db = frame.levels_db.to_vec();
        let levels = frame.levels.to_vec();
        let sum = |range: std::ops::RangeInclusive<usize>| {
            10.0 * db[range]
                .iter()
                .map(|d| 10f32.powf(d / 10.0))
                .sum::<f32>()
                .log10()
        };
        let total = sum(0..=db.len() - 1);
        let around = sum(band - 1..=band + 1);
        let edges = probe.band_edges();
        let width_bins = (edges[band + 1] - edges[band]) / (RATE as f32 / 2_048.0);
        let far = db
            .iter()
            .enumerate()
            .filter(|&(b, _)| {
                let c = centre(&probe, b);
                c < freq / 2.0 || c > freq * 2.0
            })
            .map(|(_, d)| *d)
            .fold(f32::NEG_INFINITY, f32::max);
        println!(
            "{freq:>7.1} Hz (band {band}, {width_bins:.1} bins wide): band {:+.2} dB, with its \
             neighbours {around:+.2} dB, all bands {total:+.2} dB, loudest band an octave away \
             {far:+.1} dB; level {:.3}",
            db[band], levels[band]
        );
        // A sine at half scale is −6.02 dBFS. The Hann window spreads it over
        // four bins, so a band narrower than that shares it with its
        // neighbours.
        assert_eq!(band_of(&probe, freq), band);
        let loudest = db.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert_eq!(db[band], loudest, "not the loudest band");
        assert!(
            (around - -6.02).abs() < 0.3,
            "it and its neighbours read {around} dB"
        );
        if width_bins >= 4.0 {
            assert!(
                (db[band] - -6.02).abs() < 0.5,
                "its band reads {} dB",
                db[band]
            );
        }
        assert!((total - -6.02).abs() < 0.2, "the bands sum to {total} dB");
        assert!(far < -35.0, "{far} dB an octave away");
        assert!((levels[band] - (db[band] + 80.0) / 80.0).abs() < 1e-6);
    }
}

#[test]
fn silence_reads_as_the_floor() {
    let (mut engine, handle, _) = inline_engine(2);
    let mut visualizer = Visualizer::new(handle.visualizer_tap(), VisualizerConfig::default());
    render(&mut engine, 4_800, 480);
    let frame = visualizer.read();
    assert!(frame.levels.iter().all(|&l| l == 0.0));
    assert!(frame.levels_db.iter().all(|&d| d == -80.0));
    assert!(frame.waveform.iter().all(|&s| s == 0.0));
}

#[test]
fn the_window_ends_at_the_heard_frame_not_the_rendered_one() {
    // 1 kHz for a second, then 5 kHz.
    let dir = Dir::new();
    let path = dir.path("two.wav");
    let mut samples = tone(1_000.0, 0.5, RATE, RATE as usize, 2);
    samples.extend(tone(5_000.0, 0.5, RATE, RATE as usize, 2));
    write_wav(&path, RATE, 2, &samples);
    let (mut engine, handle, _) = inline_engine(2);
    let mut visualizer = Visualizer::new(handle.visualizer_tap(), VisualizerConfig::default());
    let (low, high) = (band_of(&visualizer, 1_000.0), band_of(&visualizer, 5_000.0));
    engine.set_output_latency_frames(9_600); // 200 ms in the device
    load(&handle, &path);

    // 1.1 s rendered, the 5 kHz under way; 0.9 s heard, still 1 kHz.
    render(&mut engine, 52_800, 480);
    let frame = visualizer.read();
    let (before_low, before_high, at) = (
        frame.levels_db[low],
        frame.levels_db[high],
        frame.heard_frame,
    );
    // 0.2 s later the 5 kHz is what is heard.
    render(&mut engine, 9_600, 480);
    let frame = visualizer.read();
    let (after_low, after_high) = (frame.levels_db[low], frame.levels_db[high]);
    println!(
        "heard frame {at}: 1 kHz {before_low:+.1} dB, 5 kHz {before_high:+.1} dB; \
         at {}: 1 kHz {after_low:+.1} dB, 5 kHz {after_high:+.1} dB",
        frame.heard_frame
    );
    assert_eq!(at, 52_800 - 9_600);
    assert!(before_low > -7.0 && before_high < -60.0);
    assert!(after_high > -7.0 && after_low < -60.0);
}

#[test]
fn the_waveform_is_the_heard_samples() {
    let dir = Dir::new();
    let path = dir.path("ramp.wav");
    let samples = ramp(0, RATE as usize, 1); // mono: both channels the same
    write_wav(&path, RATE, 1, &samples);
    let (mut engine, handle, _) = inline_engine(2);
    let config = VisualizerConfig {
        waveform_frames: 512,
        ..Default::default()
    };
    let mut visualizer = Visualizer::new(handle.visualizer_tap(), config);
    engine.set_output_latency_frames(4_800);
    load(&handle, &path);
    render(&mut engine, 24_000, 480);
    let heard = 24_000 - 4_800;
    let frame = visualizer.read();
    let want = to_f32(&samples[heard - 512..heard]);
    assert_eq!(frame.waveform, &want[..]);
    println!(
        "waveform: {} frames ending at heard frame {}, last {} (ramp frame {})",
        frame.waveform.len(),
        frame.heard_frame,
        frame.waveform[511] * SCALE,
        heard - 1
    );
}

#[test]
fn a_falling_band_eases_down_on_its_time_constant() {
    let dir = Dir::new();
    let path = dir.path("stop.wav");
    let mut samples = tone(1_000.0, 0.5, RATE, RATE as usize / 2, 2);
    samples.extend(std::iter::repeat_n(0, RATE as usize * 2));
    write_wav(&path, RATE, 2, &samples);
    let (mut engine, handle, _) = inline_engine(2);
    let config = VisualizerConfig::default();
    let mut visualizer = Visualizer::new(handle.visualizer_tap(), config);
    let band = band_of(&visualizer, 1_000.0);
    load(&handle, &path);
    render(&mut engine, RATE as usize / 2, 480);
    let lit = visualizer.read().levels[band];
    // Silence for one time constant (and the window is all silence by then).
    let tau = (config.fall_seconds * RATE as f32) as usize;
    render(&mut engine, tau, 480);
    let frame = visualizer.read();
    let fallen = frame.levels[band];
    let want = lit * (-1f32).exp();
    println!(
        "fall: {lit:.4} lit, {fallen:.4} one time constant ({} ms) into silence, want {want:.4}; \
         raw {:+.1} dB",
        config.fall_seconds * 1000.0,
        frame.levels_db[band]
    );
    assert!(
        frame.levels_db[band] == -80.0,
        "the window still holds the tone"
    );
    assert!((fallen - want).abs() < 1e-4);
}
