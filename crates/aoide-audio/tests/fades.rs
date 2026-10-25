//! The pause and resume fades, measured on offline renders: their exact
//! equal-power curve, where they start and end to the frame, that a resume
//! carries on from the first frame not heard, and no click.

mod common;

use std::f64::consts::FRAC_PI_2;

use aoide_audio::engine::PAUSE_FADE_SECONDS;
use aoide_audio::{EntryId, MediaSource, TrackSpec, Transport};
use common::*;

const FADE: usize = 7_200; // 150 ms at 48 kHz

fn level(at: usize) -> f64 {
    (at as f64 / FADE as f64 * FRAC_PI_2).sin()
}

fn playing(samples: &[i32]) -> (aoide_audio::Engine, aoide_audio::EngineHandle, Dir) {
    let dir = Dir::new();
    let path = dir.path("p.wav");
    write_wav(&path, RATE, 2, samples);
    let (engine, handle, _) = inline_engine(2);
    handle
        .load(
            TrackSpec::new(EntryId::fresh(), MediaSource::file(&path)),
            0,
            true,
        )
        .unwrap();
    (engine, handle, dir)
}

#[test]
fn a_pause_fades_out_on_the_equal_power_curve_and_stops_on_its_frame() {
    assert_eq!(
        (PAUSE_FADE_SECONDS * f64::from(RATE)).round() as usize,
        FADE
    );
    let value = 16_384; // DC at half scale: the output is the gain itself.
    let (mut engine, handle, _dir) = playing(&vec![value; 2 * 2 * RATE as usize]);
    let dc = value as f32 / SCALE;
    render(&mut engine, 24_000, 480);
    handle.pause().unwrap();
    // Not a multiple of the fade, so it ends mid-block.
    let out = render(&mut engine, FADE + 4_800, 512);
    let mut worst: f64 = 0.0;
    for k in 0..FADE {
        let want = level(FADE - k);
        let got = f64::from(out[k * 2] / dc);
        worst = worst.max((got - want).abs());
    }
    let sounding = out.iter().step_by(2).take_while(|&&s| s != 0.0).count();
    println!(
        "pause: {sounding} frames of fade ({:.3} ms), worst difference from sin(p·π/2) {worst:.2e}, \
         first {:.6}, last {:.6}",
        sounding as f64 * 1000.0 / f64::from(RATE),
        out[0] / dc,
        out[(FADE - 1) * 2] / dc
    );
    assert!(worst < 1e-6);
    assert_eq!(sounding, FADE, "the fade is not 150 ms to the frame");
    assert!(
        out[FADE * 2..].iter().all(|&s| s == 0.0),
        "sound after the fade"
    );
    assert_eq!(engine.transport(), Transport::Paused);
}

#[test]
fn a_resume_fades_in_and_carries_on_from_the_first_frame_not_heard() {
    let samples = ramp(0, 3 * RATE as usize, 2);
    let (mut engine, handle, _dir) = playing(&samples);
    let want = to_f32(&samples);
    let paused_at_frame = 24_000;
    render(&mut engine, paused_at_frame, 480);
    handle.pause().unwrap();
    let fade_out = render(&mut engine, FADE + 2_000, 480);
    assert_eq!(handle.position_ms(), 650, "held where the fade ended");
    render(&mut engine, RATE as usize, 480); // a second of pause
    assert_eq!(handle.position_ms(), 650, "moved while paused");
    handle.play().unwrap();
    let fade_in = render(&mut engine, FADE + 4_800, 480);

    // Divided by its gain, every faded frame is the ramp: nothing skipped,
    // nothing repeated, across the pause.
    let mut worst: f32 = 0.0;
    for k in 0..FADE {
        for c in 0..2 {
            let out = fade_out[k * 2 + c];
            let source = want[(paused_at_frame + k) * 2 + c];
            worst = worst.max((out - source * level(FADE - k) as f32).abs());
            let out = fade_in[k * 2 + c];
            let source = want[(paused_at_frame + FADE + k) * 2 + c];
            worst = worst.max((out - source * level(k + 1) as f32).abs());
        }
    }
    println!("pause and resume over a ramp: worst sample off the faded ramp {worst:.2e}");
    assert!(worst < 1e-6);
    // Full from the fade's last frame on, bit for bit.
    let resumed_from = paused_at_frame + FADE;
    assert_eq!(
        &fade_in[(FADE - 1) * 2..],
        &want[(resumed_from + FADE - 1) * 2..(resumed_from + FADE + 4_800) * 2]
    );
    assert_eq!(handle.position_ms(), 650 + 250);
}

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
fn pausing_and_resuming_do_not_click_even_halfway() {
    let (mut engine, handle, _dir) = playing(&tone(1_000.0, 0.8, RATE, 3 * RATE as usize, 2));
    let steady = biggest_step(&render(&mut engine, 24_000, 480));
    handle.pause().unwrap();
    let out = render(&mut engine, 3_000, 480);
    // Changed its mind 60 ms into the fade: back up from where it got to.
    handle.play().unwrap();
    let back = render(&mut engine, FADE, 480);
    handle.pause().unwrap();
    let down = render(&mut engine, FADE + 480, 480);
    let mut all = out.clone();
    all.extend(&back);
    all.extend(&down);
    let worst = biggest_step(&all);
    let joins = [
        (out[out.len() - 2] - back[0]).abs(),
        (back[back.len() - 2] - down[0]).abs(),
    ];
    println!(
        "steady step {steady:.5}; worst step through a pause, a resume halfway and a pause \
         {worst:.5}; at the reversals {:.5}, {:.5}",
        joins[0], joins[1]
    );
    assert!(
        worst <= steady * 1.001,
        "a step of {worst} against {steady}"
    );
    assert!(down[FADE * 2..].iter().all(|&s| s == 0.0));
}
