//! Measured joins between two generated files.

mod common;

use std::time::Duration;

use aoide_audio::{
    DecodeMode, Engine, EngineConfig, EngineEvent, EntryId, MediaSource, MixTransition, TrackSpec,
};
use common::*;

const A_FRAMES: usize = 48_000 + 123;
const B_FRAMES: usize = 30_000 + 7;

/// Index of the first frame where the render differs from what was expected.
fn first_difference(got: &[f32], want: &[f32], channels: usize) -> Option<usize> {
    got.chunks_exact(channels)
        .zip(want.chunks_exact(channels))
        .position(|(g, w)| g != w)
}

fn join(write: fn(&std::path::Path, u32, usize, &[i32]), extension: &str, block: usize) {
    let dir = Dir::new();
    let (a, b) = (
        dir.path(&format!("a.{extension}")),
        dir.path(&format!("b.{extension}")),
    );
    // B carries on the ramp exactly where A stops.
    let a_samples = ramp(0, A_FRAMES, 2);
    let b_samples = ramp(A_FRAMES as u64, B_FRAMES, 2);
    write(&a, RATE, 2, &a_samples);
    write(&b, RATE, 2, &b_samples);

    let (mut engine, handle, events) = inline_engine(2);
    let (id_a, id_b) = (EntryId::fresh(), EntryId::fresh());
    handle
        .load(TrackSpec::new(id_a, MediaSource::file(&a)), 0, true)
        .unwrap();
    handle
        .set_next(Some((
            TrackSpec::new(id_b, MediaSource::file(&b)),
            MixTransition::Gapless,
        )))
        .unwrap();

    let total = A_FRAMES + B_FRAMES;
    let out = render(&mut engine, total + 1_000, block);

    let mut want = to_f32(&a_samples);
    want.extend(to_f32(&b_samples));
    assert_eq!(
        first_difference(&out[..total * 2], &want, 2),
        None,
        "the join is not sample-exact"
    );
    // And nothing after the last frame.
    assert!(out[total * 2..].iter().all(|&s| s == 0.0));

    // Every step between neighbouring frames is the ramp's own step: no gap,
    // no repeat, at the join or anywhere else.
    let left: Vec<f32> = out[..total * 2].iter().step_by(2).copied().collect();
    for (k, pair) in left.windows(2).enumerate() {
        let step = ((pair[1] - pair[0]) * SCALE).round() as i32;
        assert!(step == 1 || step == -19_999, "frame {k}: step {step}");
    }

    assert_eq!(
        drain(&events),
        vec![
            EngineEvent::TrackStarted { id: id_a },
            EngineEvent::TrackEnded { id: id_a },
            EngineEvent::TrackStarted { id: id_b },
            EngineEvent::TrackEnded { id: id_b },
            EngineEvent::Finished,
        ]
    );
}

#[test]
fn wav_to_wav_is_sample_exact() {
    // A block size that puts the join in the middle of a block.
    join(write_wav, "wav", 333);
}

#[test]
fn flac_to_flac_is_sample_exact() {
    join(write_flac, "flac", 512);
}

#[test]
fn a_cut_joins_the_same_way() {
    // Cut and gapless differ in the planner, not in the join: neither puts
    // anything between the two tracks.
    let dir = Dir::new();
    let (a, b) = (dir.path("a.flac"), dir.path("b.wav"));
    write_flac(&a, RATE, 2, &ramp(0, A_FRAMES, 2));
    write_wav(&b, RATE, 2, &ramp(A_FRAMES as u64, B_FRAMES, 2));
    let (mut engine, handle, _events) = inline_engine(2);
    handle
        .load(
            TrackSpec::new(EntryId::fresh(), MediaSource::file(&a)),
            0,
            true,
        )
        .unwrap();
    handle
        .set_next(Some((
            TrackSpec::new(EntryId::fresh(), MediaSource::file(&b)),
            MixTransition::Cut,
        )))
        .unwrap();
    let out = render(&mut engine, A_FRAMES + B_FRAMES, 1024);
    let mut want = to_f32(&ramp(0, A_FRAMES, 2));
    want.extend(to_f32(&ramp(A_FRAMES as u64, B_FRAMES, 2)));
    assert_eq!(first_difference(&out, &want, 2), None);
}

#[test]
fn the_threaded_path_is_sample_exact_too() {
    let dir = Dir::new();
    let (a, b) = (dir.path("a.flac"), dir.path("b.flac"));
    let a_samples = ramp(0, A_FRAMES, 2);
    let b_samples = ramp(A_FRAMES as u64, B_FRAMES, 2);
    write_flac(&a, RATE, 2, &a_samples);
    write_flac(&b, RATE, 2, &b_samples);

    let (mut engine, handle, _events) = Engine::new(EngineConfig {
        sample_rate: RATE,
        channels: 2,
        decode: DecodeMode::Threaded,
    });
    handle
        .load(
            TrackSpec::new(EntryId::fresh(), MediaSource::file(&a)),
            0,
            true,
        )
        .unwrap();
    handle
        .set_next(Some((
            TrackSpec::new(EntryId::fresh(), MediaSource::file(&b)),
            MixTransition::Gapless,
        )))
        .unwrap();
    // Both files fit in the decode-ahead ring; give the decoder time to fill it
    // so the render does not catch up with it.
    engine.render(&mut []);
    std::thread::sleep(Duration::from_millis(400));

    let total = A_FRAMES + B_FRAMES;
    let out = render(&mut engine, total, 480);
    let mut want = to_f32(&a_samples);
    want.extend(to_f32(&b_samples));
    assert_eq!(first_difference(&out, &want, 2), None);
}

/// A join through a rate conversion: two 44.1 kHz halves of one tone played
/// on a 48 kHz engine, against the same tone as a single file. One resampler
/// runs across the join, so the two renders match.
#[test]
fn a_resampled_join_matches_one_continuous_file() {
    let dir = Dir::new();
    let rate = 44_100;
    let (a_frames, b_frames) = (44_100 + 17, 22_050 + 3);
    let whole = tone(997.0, 0.5, rate, a_frames + b_frames, 2);
    let (a, b, one) = (dir.path("a.wav"), dir.path("b.wav"), dir.path("one.wav"));
    write_wav(&a, rate, 2, &whole[..a_frames * 2]);
    write_wav(&b, rate, 2, &whole[a_frames * 2..]);
    write_wav(&one, rate, 2, &whole);

    let expected_frames = ((a_frames + b_frames) as f64 * 48_000.0 / 44_100.0).round() as usize;

    let (mut joined, handle, events) = inline_engine(2);
    let id_b = EntryId::fresh();
    handle
        .load(
            TrackSpec::new(EntryId::fresh(), MediaSource::file(&a)),
            0,
            true,
        )
        .unwrap();
    handle
        .set_next(Some((
            TrackSpec::new(id_b, MediaSource::file(&b)),
            MixTransition::Gapless,
        )))
        .unwrap();
    let out_joined = render(&mut joined, expected_frames + 2_000, 512);

    let (mut single, handle, _) = inline_engine(2);
    handle
        .load(
            TrackSpec::new(EntryId::fresh(), MediaSource::file(&one)),
            0,
            true,
        )
        .unwrap();
    let out_single = render(&mut single, expected_frames + 2_000, 512);

    let length = |out: &[f32]| {
        out.iter()
            .step_by(2)
            .rposition(|&s| s != 0.0)
            .map_or(0, |i| i + 1)
    };
    let (joined_len, single_len) = (length(&out_joined), length(&out_single));
    assert!(
        joined_len.abs_diff(expected_frames) <= 1,
        "joined: {joined_len} frames, expected {expected_frames}"
    );
    assert_eq!(joined_len, single_len);

    let worst = out_joined
        .iter()
        .zip(&out_single)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max);
    println!(
        "resampled join: {joined_len} frames (expected {expected_frames}), worst difference from one file {worst:e}"
    );
    assert!(
        worst < 1e-6,
        "the joined render strays {worst} from the continuous one"
    );

    let started: Vec<_> = drain(&events)
        .into_iter()
        .filter(|e| matches!(e, EngineEvent::TrackStarted { .. }))
        .collect();
    assert_eq!(started.len(), 2);
}

/// The heard track switches on the exact output frame that A's last input
/// frame maps to through the 44.1 to 48 kHz conversion.
#[test]
fn a_resampled_join_switches_tracks_on_its_frame() {
    let dir = Dir::new();
    let rate = 44_100;
    let (a_frames, b_frames) = (44_100 + 17, 4_410);
    let whole = tone(997.0, 0.5, rate, a_frames + b_frames, 2);
    let (a, b) = (dir.path("a.wav"), dir.path("b.wav"));
    write_wav(&a, rate, 2, &whole[..a_frames * 2]);
    write_wav(&b, rate, 2, &whole[a_frames * 2..]);

    let (mut engine, handle, _) = inline_engine(2);
    let (id_a, id_b) = (EntryId::fresh(), EntryId::fresh());
    handle
        .load(TrackSpec::new(id_a, MediaSource::file(&a)), 0, true)
        .unwrap();
    handle
        .set_next(Some((
            TrackSpec::new(id_b, MediaSource::file(&b)),
            MixTransition::Gapless,
        )))
        .unwrap();

    let boundary = (a_frames as f64 * 48_000.0 / 44_100.0).round() as usize;
    render(&mut engine, boundary, 500);
    assert_eq!(
        handle.current(),
        Some(id_a),
        "still A after {boundary} frames"
    );
    render(&mut engine, 1, 1);
    assert_eq!(handle.current(), Some(id_b), "B from frame {boundary}");
    assert_eq!(handle.position_ms(), 0);
}
