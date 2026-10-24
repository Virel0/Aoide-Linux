//! Offline renders, measured: trimming, crossfade level, seeking, loudness
//! gain, volume and pause.

mod common;

use aoide_audio::loudness::gain_db;
use aoide_audio::{
    EngineEvent, EntryId, MediaSource, MixTransition, SoundBounds, TrackSpec, Transport,
};
use common::*;

fn first_difference(got: &[f32], want: &[f32], channels: usize) -> Option<usize> {
    got.chunks_exact(channels)
        .zip(want.chunks_exact(channels))
        .position(|(g, w)| g != w)
}

// Trimming

fn trims(write: fn(&std::path::Path, u32, usize, &[i32]), extension: &str) {
    let dir = Dir::new();
    let path = dir.path(&format!("t.{extension}"));
    let frames = 2 * RATE as usize;
    let samples = ramp(0, frames, 2);
    write(&path, RATE, 2, &samples);

    let bounds = SoundBounds::new(250, 1_500);
    let (start, end) = (250 * 48, 1_500 * 48); // frames at 48 kHz
    let (mut engine, handle, events) = inline_engine(2);
    let id = EntryId::fresh();
    handle
        .load(
            TrackSpec::new(id, MediaSource::file(&path)).with_bounds(Some(bounds)),
            0,
            true,
        )
        .unwrap();

    let out = render(&mut engine, frames, 480);
    let want = to_f32(&samples[start * 2..end * 2]);
    assert_eq!(
        first_difference(&out[..want.len()], &want, 2),
        None,
        "the trimmed span is not the file's frames {start}..{end}"
    );
    assert!(
        out[want.len()..].iter().all(|&s| s == 0.0),
        "something played past the end bound"
    );
    println!(
        "{extension}: first frame {} (want {}), last frame {} (want {})",
        out[0] * SCALE,
        want[0] * SCALE,
        out[want.len() - 2] * SCALE,
        want[want.len() - 2] * SCALE
    );
    assert_eq!(
        drain(&events),
        vec![
            EngineEvent::TrackStarted { id },
            EngineEvent::TrackEnded { id },
            EngineEvent::Finished,
        ]
    );
}

#[test]
fn trimming_starts_and_ends_on_the_right_sample_wav() {
    trims(write_wav, "wav");
}

#[test]
fn trimming_starts_and_ends_on_the_right_sample_flac() {
    trims(write_flac, "flac");
}

#[test]
fn trimmed_tracks_join_gaplessly() {
    let dir = Dir::new();
    let (a, b) = (dir.path("a.flac"), dir.path("b.flac"));
    let a_samples = ramp(0, RATE as usize, 2);
    let b_samples = ramp(5_000, RATE as usize, 2);
    write_flac(&a, RATE, 2, &a_samples);
    write_flac(&b, RATE, 2, &b_samples);
    let (mut engine, handle, _) = inline_engine(2);
    // A loses its last 200 ms, B its first 100 ms.
    let a_spec = TrackSpec::new(EntryId::fresh(), MediaSource::file(&a))
        .with_bounds(Some(SoundBounds::new(0, 800)));
    let b_spec = TrackSpec::new(EntryId::fresh(), MediaSource::file(&b))
        .with_bounds(Some(SoundBounds::new(100, 1_000)));
    handle.load(a_spec, 0, true).unwrap();
    handle
        .set_next(Some((b_spec, MixTransition::Gapless)))
        .unwrap();
    let out = render(&mut engine, 2 * RATE as usize, 512);
    let mut want = to_f32(&a_samples[..800 * 48 * 2]);
    want.extend(to_f32(&b_samples[100 * 48 * 2..]));
    assert_eq!(first_difference(&out[..want.len()], &want, 2), None);
    assert!(out[want.len()..].iter().all(|&s| s == 0.0));
}

// Crossfade

#[test]
fn a_crossfade_holds_the_level_of_one_track() {
    let dir = Dir::new();
    let (a, b) = (dir.path("a.flac"), dir.path("b.flac"));
    let seconds = 6;
    let frames = seconds * RATE as usize;
    // Two uncorrelated signals at one level, as two recordings are.
    let a_samples = noise(1, 0.5, frames, 2);
    let b_samples = noise(2, 0.5, frames, 2);
    write_flac(&a, RATE, 2, &a_samples);
    write_flac(&b, RATE, 2, &b_samples);

    let overlap = 2 * RATE as usize;
    let (mut engine, handle, events) = inline_engine(2);
    let (id_a, id_b) = (EntryId::fresh(), EntryId::fresh());
    handle
        .load(TrackSpec::new(id_a, MediaSource::file(&a)), 0, true)
        .unwrap();
    handle
        .set_next(Some((
            TrackSpec::new(id_b, MediaSource::file(&b)),
            MixTransition::Blend { seconds: 2.0 },
        )))
        .unwrap();
    let total = 2 * frames - overlap;
    let out = render(&mut engine, total + RATE as usize, 512);

    let blend_start = frames - overlap;
    // Untouched before the overlap, and B untouched after it.
    assert_eq!(
        first_difference(
            &out[..blend_start * 2],
            &to_f32(&a_samples[..blend_start * 2]),
            2
        ),
        None
    );
    assert_eq!(
        first_difference(
            &out[frames * 2..total * 2],
            &to_f32(&b_samples[overlap * 2..]),
            2
        ),
        None,
        "B after the overlap is not B"
    );
    assert!(
        out[total * 2..].iter().all(|&s| s == 0.0),
        "longer than A + B - overlap"
    );

    // The level through the overlap, in 100 ms windows, against one track.
    let window = RATE as usize / 10;
    let reference = rms(&out[..blend_start * 2], 2);
    let mut worst: f64 = 0.0;
    let mut middle = 0.0;
    for w in 0..overlap / window {
        let at = blend_start + w * window;
        let level = db(rms(&out[at * 2..(at + window) * 2], 2) / reference);
        worst = if level.abs() > worst.abs() {
            level
        } else {
            worst
        };
        if w == overlap / window / 2 {
            middle = level;
        }
    }
    // What a linear fade would have done at the midpoint, for comparison.
    let mid = blend_start + overlap / 2;
    let linear: Vec<f32> = (0..window)
        .flat_map(|k| {
            let (x, y) = (
                a_samples[(mid + k) * 2] as f32 / SCALE,
                b_samples[(mid - blend_start + k) * 2] as f32 / SCALE,
            );
            [0.5 * x + 0.5 * y, 0.0]
        })
        .collect();
    println!(
        "crossfade: worst 100 ms window {worst:+.3} dB, midpoint {middle:+.3} dB (a linear fade: {:+.3} dB)",
        db(rms(&linear, 2) / reference)
    );
    assert!(
        worst.abs() <= 1.5,
        "the level strays {worst:.2} dB through the overlap"
    );

    let events = drain(&events);
    let started_b = events
        .iter()
        .position(|e| *e == EngineEvent::TrackStarted { id: id_b })
        .unwrap();
    let ended_a = events
        .iter()
        .position(|e| *e == EngineEvent::TrackEnded { id: id_a })
        .unwrap();
    assert!(started_b < ended_a, "B starts before A ends: {events:?}");
}

#[test]
fn a_crossfade_never_overlaps_more_than_is_left() {
    // Asked for after the point where the overlap should have begun, the
    // blend shrinks to what is left of A instead of cutting A off mid-fade.
    let dir = Dir::new();
    let (a, b) = (dir.path("a.wav"), dir.path("b.wav"));
    let frames = 3 * RATE as usize;
    write_wav(&a, RATE, 2, &tone(440.0, 0.5, RATE, frames, 2));
    write_wav(&b, RATE, 2, &tone(660.0, 0.5, RATE, frames, 2));
    let (mut engine, handle, _) = inline_engine(2);
    handle
        .load(
            TrackSpec::new(EntryId::fresh(), MediaSource::file(&a)),
            0,
            true,
        )
        .unwrap();
    render(&mut engine, frames - RATE as usize, 512); // one second left
    handle
        .set_next(Some((
            TrackSpec::new(EntryId::fresh(), MediaSource::file(&b)),
            MixTransition::Blend { seconds: 2.0 },
        )))
        .unwrap();
    let out = render(&mut engine, frames, 512);
    // A + B - 1 s of overlap in all.
    let last = out.iter().step_by(2).rposition(|&s| s != 0.0).unwrap();
    let played = frames - RATE as usize + last + 1;
    println!(
        "late blend: {played} frames played, {} expected",
        2 * frames - RATE as usize
    );
    assert!(played.abs_diff(2 * frames - RATE as usize) <= 2);
}

#[test]
fn a_track_chained_during_a_blend_follows_the_incoming_track() {
    // A blends into B; while that is under way the next plan arrives, as the
    // player sends it on hearing B start. Once with a second blend (B into C)
    // and once with a gapless join (B straight into C).
    for second in [
        MixTransition::Blend { seconds: 1.0 },
        MixTransition::Gapless,
    ] {
        let dir = Dir::new();
        let frames = 3 * RATE as usize;
        let paths: Vec<_> = (0..3).map(|n| dir.path(&format!("{n}.wav"))).collect();
        let c_samples = noise(3, 0.5, frames, 2);
        write_wav(&paths[0], RATE, 2, &noise(1, 0.5, frames, 2));
        write_wav(&paths[1], RATE, 2, &noise(2, 0.5, frames, 2));
        write_wav(&paths[2], RATE, 2, &c_samples);
        let ids = [EntryId::fresh(), EntryId::fresh(), EntryId::fresh()];
        let spec = |n: usize| TrackSpec::new(ids[n], MediaSource::file(&paths[n]));

        let (mut engine, handle, events) = inline_engine(2);
        handle.load(spec(0), 0, true).unwrap();
        handle
            .set_next(Some((spec(1), MixTransition::Blend { seconds: 1.0 })))
            .unwrap();
        let mut out = Vec::new();
        let mut sent = false;
        while out.len() < 12 * RATE as usize * 2 {
            out.extend(render(&mut engine, 480, 480));
            if !sent && drain(&events).contains(&EngineEvent::TrackStarted { id: ids[1] }) {
                handle.set_next(Some((spec(2), second))).unwrap();
                sent = true;
            }
        }
        let overlaps = 1 + usize::from(second != MixTransition::Gapless);
        let total = 3 * frames - overlaps * RATE as usize;
        let last = out.iter().step_by(2).rposition(|&s| s != 0.0).unwrap() + 1;
        println!("{second:?} after a blend: {last} frames played, {total} expected");
        assert_eq!(last, total, "C did not follow B");
        let tail = 4_800;
        assert_eq!(
            &out[(total - tail) * 2..total * 2],
            &to_f32(&c_samples[(frames - tail) * 2..])[..]
        );
    }
}

// Seeking

fn seeks(write: fn(&std::path::Path, u32, usize, &[i32]), extension: &str) {
    let dir = Dir::new();
    let path = dir.path(&format!("s.{extension}"));
    let frames = 3 * RATE as usize;
    let samples = ramp(0, frames, 2);
    write(&path, RATE, 2, &samples);

    let (mut engine, handle, _) = inline_engine(2);
    let id = EntryId::fresh();
    handle
        .load(TrackSpec::new(id, MediaSource::file(&path)), 1_234, true)
        .unwrap();
    let block = 512;
    let out = render(&mut engine, block, block);
    let target = 1_234 * 48;
    let landed = ((out[0] * SCALE).round() as i64 + 10_000) as usize; // ramp value back to its frame
    println!(
        "{extension}: seek to frame {target}, landed on {landed}; position {} ms after one block",
        handle.position_ms()
    );
    assert_eq!(
        landed,
        target % 20_000,
        "the seek did not land on its frame"
    );
    assert_eq!(
        first_difference(&out, &to_f32(&samples[target * 2..(target + block) * 2]), 2),
        None
    );
    // Reported within one block of where it is.
    let reported = handle.position_ms() as i64;
    let block_ms = (block as f64 * 1000.0 / f64::from(RATE)) as i64;
    assert!(
        (reported - 1_234).abs() <= block_ms + 1,
        "position {reported} ms"
    );
}

#[test]
fn a_seek_lands_on_its_frame_wav() {
    seeks(write_wav, "wav");
}

#[test]
fn a_seek_lands_on_its_frame_flac() {
    seeks(write_flac, "flac");
}

#[test]
fn a_seek_while_playing_fades_in_and_then_is_exact() {
    let dir = Dir::new();
    let path = dir.path("s.flac");
    let samples = ramp(0, 3 * RATE as usize, 2);
    write_flac(&path, RATE, 2, &samples);
    let (mut engine, handle, _) = inline_engine(2);
    let spec = TrackSpec::new(EntryId::fresh(), MediaSource::file(&path));
    handle.load(spec.clone(), 0, true).unwrap();
    render(&mut engine, 4_800, 480);
    handle.load(spec, 2_000, true).unwrap();
    let out = render(&mut engine, 4_800, 480);
    // Ten milliseconds of fade-in so the jump does not click, then exact.
    let fade = 480 + 1;
    let start = 2_000 * 48;
    assert!(
        out[0].abs() < 1e-3,
        "the first frame after a seek is not faded"
    );
    assert_eq!(
        first_difference(
            &out[fade * 2..],
            &to_f32(&samples[(start + fade) * 2..(start + 4_800) * 2]),
            2
        ),
        None
    );
}

// Loudness and volume

fn level_with(gain: Option<f64>) -> f64 {
    let dir = Dir::new();
    let path = dir.path("l.wav");
    write_wav(&path, RATE, 2, &tone(1_000.0, 0.8, RATE, RATE as usize, 2));
    let (mut engine, handle, _) = inline_engine(2);
    handle
        .load(
            TrackSpec::new(EntryId::fresh(), MediaSource::file(&path)).with_gain_db(gain),
            0,
            true,
        )
        .unwrap();
    let out = render(&mut engine, RATE as usize, 1024);
    rms(&out, 2)
}

#[test]
fn the_loudness_gain_is_applied_in_the_render() {
    let plain = level_with(None);
    let gain = gain_db(Some(-12.0), None); // 6 dB over the reference
    assert_eq!(gain, Some(-6.0));
    let levelled = level_with(gain);
    let measured = db(levelled / plain);
    println!("loudness: asked -6.00 dB, measured {measured:+.4} dB");
    assert!((measured + 6.0).abs() < 0.01);
}

#[test]
fn a_positive_gain_never_amplifies() {
    let plain = level_with(None);
    let boosted = level_with(Some(6.0));
    assert_eq!(plain, boosted, "a +6 dB gain changed the level");
}

#[test]
fn volume_ramps_within_one_block() {
    let dir = Dir::new();
    let path = dir.path("v.wav");
    let samples = tone(1_000.0, 0.8, RATE, RATE as usize, 2);
    write_wav(&path, RATE, 2, &samples);
    let (mut engine, handle, _) = inline_engine(2);
    handle
        .load(
            TrackSpec::new(EntryId::fresh(), MediaSource::file(&path)),
            0,
            true,
        )
        .unwrap();
    render(&mut engine, 4_800, 480);
    handle.set_volume(0.5).unwrap();
    let out = render(&mut engine, 4_800, 480);
    let want = to_f32(&samples[4_800 * 2..9_600 * 2]);
    // After the first block, exactly half.
    for (got, want) in out[480 * 2..].iter().zip(&want[480 * 2..]) {
        assert!((got - want * 0.5).abs() < 1e-6);
    }
    // And no jump inside the ramp block bigger than the signal's own step.
    let biggest_step = out[..480 * 2]
        .iter()
        .step_by(2)
        .copied()
        .collect::<Vec<_>>()
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(
        biggest_step < 0.11,
        "a step of {biggest_step} while ramping"
    );
}

#[test]
fn pause_holds_the_position_and_play_carries_on() {
    let dir = Dir::new();
    let path = dir.path("p.wav");
    write_wav(
        &path,
        RATE,
        2,
        &tone(440.0, 0.5, RATE, 3 * RATE as usize, 2),
    );
    let (mut engine, handle, _) = inline_engine(2);
    handle
        .load(
            TrackSpec::new(EntryId::fresh(), MediaSource::file(&path)),
            0,
            true,
        )
        .unwrap();
    render(&mut engine, RATE as usize / 2, 480);
    handle.pause().unwrap();
    render(&mut engine, 960, 480); // the fade-out runs out
    let paused_at = handle.position_ms();
    let silent = render(&mut engine, RATE as usize, 480);
    assert!(silent.iter().all(|&s| s == 0.0), "sound while paused");
    assert_eq!(
        handle.position_ms(),
        paused_at,
        "the position moved while paused"
    );
    assert_eq!(engine.transport(), Transport::Paused);
    println!("pause: held at {paused_at} ms (paused after 500 ms + a 10 ms fade)");
    assert!((paused_at as i64 - 510).abs() <= 11);

    handle.play().unwrap();
    render(&mut engine, 4_800, 480);
    let resumed = handle.position_ms();
    assert!(
        (resumed as i64 - (paused_at as i64 + 100)).abs() <= 1,
        "carried on to {resumed} ms"
    );
}

#[test]
fn the_heard_position_trails_the_rendered_one_by_the_latency() {
    let dir = Dir::new();
    let (a, b) = (dir.path("a.wav"), dir.path("b.wav"));
    write_wav(&a, RATE, 2, &ramp(0, RATE as usize, 2));
    write_wav(&b, RATE, 2, &ramp(0, RATE as usize, 2));
    let (mut engine, handle, events) = inline_engine(2);
    let latency = 4_800; // 100 ms of device buffer
    engine.set_output_latency_frames(latency);
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

    render(&mut engine, 24_000, 480);
    assert_eq!(handle.position_ms(), 400, "500 ms rendered, 400 ms heard");

    // B is rendered from frame 48 000 but heard 100 ms later.
    render(&mut engine, 24_000 + 2_400, 480);
    assert_eq!(handle.current(), Some(id_a));
    assert!(
        !drain(&events).contains(&EngineEvent::TrackStarted { id: id_b }),
        "announced before it is heard"
    );
    render(&mut engine, 2_400, 480);
    assert_eq!(handle.current(), Some(id_b));
    assert!(drain(&events).contains(&EngineEvent::TrackStarted { id: id_b }));
    assert_eq!(handle.position_ms(), 0);
}
