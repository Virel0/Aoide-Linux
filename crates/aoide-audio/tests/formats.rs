//! The other formats the decoder is built with, measured on small fixtures
//! (tests/fixtures; its README says how each was made).
//!
//! ALAC is lossless, so it is held to the ramp: every frame exact, joins and
//! seeks included. The lossy files carry clicks at known frames instead, and
//! each measurement says where those clicks land: the length the decoder
//! gives, how far the audio is shifted from the source (encoder delay left
//! in), and the gap a gapless join leaves between the two tracks' music.
//!
//! What symphonia 0.6.1 gives, as measured here:
//!
//! | Format | Length | Shift | Join gap |
//! |---|---|---|---|
//! | ALAC in M4A | exact | 0 | 0, sample-exact |
//! | MP3 (LAME tag) | exact | 0 | 0 |
//! | Ogg Vorbis | exact | 0 | 0 |
//! | AAC in M4A (Apple encoder) | priming + padding | 2112 | 3077 frames (64 ms) |
//! | AAC in ADTS (ffmpeg, as Jellyfin transcodes) | priming + padding | 1024 | 1029 frames (21 ms) |
//!
//! Shift and gap are in frames at 48 kHz; the gap is A's padding plus B's
//! priming, so it changes with A's length.
//!
//! AAC is not gapless: symphonia's MP4 reader ignores the priming and padding
//! the file declares (it does not read edit lists or iTunSMPB) and its AAC
//! decoder trims nothing, so every AAC track starts with the encoder's
//! priming frames and ends with its padding. ADTS has nowhere to declare
//! them at all.

mod common;

use std::path::PathBuf;

use aoide_audio::decode::Decoder;
use aoide_audio::{EngineEvent, EntryId, Error, MediaSource, MixTransition, TrackSpec};
use common::*;

/// The fixtures' lengths: the same as the generated files in gapless.rs.
const A_FRAMES: usize = 48_000 + 123;
const B_FRAMES: usize = 30_000 + 7;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Where the fixtures' clicks are: every 4001 frames from frame 1000, and one
/// 500 frames before the end. Left channel positive, right negative.
fn clicks(frames: usize) -> Vec<usize> {
    let mut at: Vec<usize> = (1_000..frames - 600).step_by(4_001).collect();
    at.push(frames - 500);
    at
}

/// The shift, in frames, that best lines `clicks` (offset by `base`) up with
/// the left channel of `samples`: the lag where the clicks' summed level is
/// highest. Lossy coding smears a click over a few frames; summing over
/// every click finds its centre.
fn shift(samples: &[f32], channels: usize, clicks: &[usize], base: usize) -> i64 {
    let frames = samples.len() / channels;
    (-8i64..=4_000)
        .map(|lag| {
            let level: f32 = clicks
                .iter()
                .filter_map(|&click| {
                    let at = (base + click) as i64 + lag;
                    (0..frames as i64)
                        .contains(&at)
                        .then(|| samples[at as usize * channels])
                })
                .sum();
            (lag, level)
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(lag, _)| lag)
        .expect("a range of lags")
}

struct Decoded {
    samples: Vec<f32>,
    channels: usize,
    rate: u32,
    /// What the container says.
    total_frames: Option<u64>,
}

impl Decoded {
    fn frames(&self) -> usize {
        self.samples.len() / self.channels
    }
}

fn decode(name: &str) -> Decoded {
    let mut decoder = Decoder::open(&MediaSource::file(fixture(name))).expect("open");
    let (rate, total_frames) = (decoder.sample_rate(), decoder.total_frames());
    let mut samples = Vec::new();
    let mut channels = 0;
    let mut next = 0;
    while let Some(packet) = decoder.next_packet().expect("decode") {
        assert_eq!(packet.first_frame, next, "{name}: packets follow on");
        next += packet.frames() as u64;
        channels = packet.channels;
        samples.extend_from_slice(packet.samples);
    }
    Decoded {
        samples,
        channels,
        rate,
        total_frames,
    }
}

/// Renders `name` on the engine from `start_ms`, for `frames` frames.
fn render_from(name: &str, start_ms: u64, frames: usize) -> Vec<f32> {
    let (mut engine, handle, _events) = inline_engine(2);
    handle
        .load(
            TrackSpec::new(EntryId::fresh(), MediaSource::file(fixture(name))),
            start_ms,
            true,
        )
        .unwrap();
    render(&mut engine, frames, 512)
}

/// Renders `a` then `b`, joined gaplessly, and checks the events.
fn render_join(a: &str, b: &str, frames: usize) -> Vec<f32> {
    let (mut engine, handle, events) = inline_engine(2);
    let (id_a, id_b) = (EntryId::fresh(), EntryId::fresh());
    handle
        .load(TrackSpec::new(id_a, MediaSource::file(fixture(a))), 0, true)
        .unwrap();
    handle
        .set_next(Some((
            TrackSpec::new(id_b, MediaSource::file(fixture(b))),
            MixTransition::Gapless,
        )))
        .unwrap();
    let out = render(&mut engine, frames, 333);
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
    out
}

/// What a lossy format measures, and what it is held to.
struct Lossy {
    a: &'static str,
    b: &'static str,
    /// Frames the decoder gives beyond the source's own.
    extra_a: usize,
    extra_b: usize,
    /// How far behind the source the audio comes out: encoder delay left in.
    shift: i64,
    /// Silence a gapless join leaves between A's last frame of music and B's
    /// first.
    join_gap: i64,
}

fn measure(format: &Lossy) {
    // Decode, and the length.
    let a = decode(format.a);
    let b = decode(format.b);
    assert_eq!((a.rate, a.channels), (RATE, 2), "{}", format.a);
    println!(
        "{}: {} frames decoded for {A_FRAMES} (container says {:?}); {}: {} for {B_FRAMES}",
        format.a,
        a.frames(),
        a.total_frames,
        format.b,
        b.frames()
    );
    assert_eq!(a.frames(), A_FRAMES + format.extra_a, "{} length", format.a);
    assert_eq!(b.frames(), B_FRAMES + format.extra_b, "{} length", format.b);
    assert_eq!(a.total_frames, Some(a.frames() as u64), "{}", format.a);

    // Where the music sits in what is decoded.
    let shift_a = shift(&a.samples, 2, &clicks(A_FRAMES), 0);
    let shift_b = shift(&b.samples, 2, &clicks(B_FRAMES), 0);
    println!(
        "{}: shifted {shift_a} frames; {}: {shift_b}",
        format.a, format.b
    );
    assert_eq!((shift_a, shift_b), (format.shift, format.shift));

    // A seek half a second in: the clicks after it, where they should be.
    let start = RATE as usize / 2;
    let out = render_from(format.a, 500, A_FRAMES - start + 4_000);
    let after: Vec<usize> = clicks(A_FRAMES)
        .into_iter()
        .filter(|&click| click >= start + 100)
        .map(|click| click - start)
        .collect();
    let seek_shift = shift(&out, 2, &after, 0);
    println!("{}: after a seek to 500 ms, shifted {seek_shift}", format.a);
    assert_eq!(seek_shift, format.shift, "the seek moves the music");

    // The gapless join, on the engine.
    let out = render_join(format.a, format.b, a.frames() + b.frames() + 2_000);
    // B starts where A's decode ends, so B's clicks are looked for from there.
    let shift_in_a = shift(&out, 2, &clicks(A_FRAMES), 0);
    let shift_in_b = shift(&out, 2, &clicks(B_FRAMES), a.frames());
    let gap = (a.frames() as i64 + shift_in_b) - (shift_in_a + A_FRAMES as i64);
    println!(
        "{} then {}: B's music starts {gap} frames after A's ends ({:.1} ms)",
        format.a,
        format.b,
        gap as f64 * 1000.0 / f64::from(RATE)
    );
    assert_eq!(gap, format.join_gap, "the join's gap");
    let heard = out
        .iter()
        .step_by(2)
        .rposition(|&s| s != 0.0)
        .map_or(0, |i| i + 1);
    assert!(
        heard <= a.frames() + b.frames(),
        "the join plays {heard} frames, more than the two decodes"
    );
}

#[test]
fn mp3_with_a_lame_tag_is_gapless() {
    // ffmpeg's libmp3lame writes the LAME tag (encoder delay and padding),
    // and symphonia's MP3 reader and decoder trim both.
    measure(&Lossy {
        a: "mp3_a.mp3",
        b: "mp3_b.mp3",
        extra_a: 0,
        extra_b: 0,
        shift: 0,
        join_gap: 0,
    });
}

#[test]
fn ogg_vorbis_is_gapless() {
    // The last page's granule position gives the length; the first packet's
    // preroll is dropped by the decoder.
    measure(&Lossy {
        a: "vorbis_a.ogg",
        b: "vorbis_b.ogg",
        extra_a: 0,
        extra_b: 0,
        shift: 0,
        join_gap: 0,
    });
}

#[test]
fn aac_in_m4a_keeps_its_priming_and_padding() {
    // Apple's encoder: 2112 frames of priming, then padding to a whole
    // 1024-frame packet (965 frames for A, 649 for B). The file says so in
    // iTunSMPB; symphonia 0.6.1 reads neither that nor an edit list, so the
    // join has A's padding and B's priming between the two: 3077 frames,
    // 64 ms.
    measure(&Lossy {
        a: "aac_a.m4a",
        b: "aac_b.m4a",
        extra_a: 2_112 + 965,
        extra_b: 2_112 + 649,
        shift: 2_112,
        join_gap: 965 + 2_112,
    });
}

#[test]
fn aac_in_adts_keeps_its_priming_and_padding() {
    // ffmpeg's native AAC encoder, which Jellyfin transcodes with: 1024
    // frames of priming, padding to a whole packet (5 frames for A, 713 for
    // B), and ADTS has no field to say so.
    measure(&Lossy {
        a: "adts_a.aac",
        b: "adts_b.aac",
        extra_a: 1_024 + 5,
        extra_b: 1_024 + 713,
        shift: 1_024,
        join_gap: 5 + 1_024,
    });
}

// ALAC: lossless, so every frame is checked.

fn first_difference(got: &[f32], want: &[f32], channels: usize) -> Option<usize> {
    got.chunks_exact(channels)
        .zip(want.chunks_exact(channels))
        .position(|(g, w)| g != w)
}

#[test]
fn alac_decodes_every_frame_exactly() {
    let a = decode("alac_a.m4a");
    assert_eq!((a.rate, a.channels), (RATE, 2));
    assert_eq!(a.frames(), A_FRAMES);
    assert_eq!(a.total_frames, Some(A_FRAMES as u64));
    assert_eq!(
        first_difference(&a.samples, &to_f32(&ramp(0, A_FRAMES, 2)), 2),
        None
    );
}

#[test]
fn alac_seeks_to_the_exact_frame() {
    // 700 ms is frame 33 600, inside a 4096-frame packet: the packet before
    // it is decoded and the frames up to it dropped.
    let start = 700 * 48;
    let out = render_from("alac_a.m4a", 700, A_FRAMES - start + 1_000);
    let want = to_f32(&ramp(start as u64, A_FRAMES - start, 2));
    assert_eq!(first_difference(&out[..want.len()], &want, 2), None);
    assert!(out[want.len()..].iter().all(|&s| s == 0.0));
}

#[test]
fn alac_to_alac_is_sample_exact() {
    let total = A_FRAMES + B_FRAMES;
    let out = render_join("alac_a.m4a", "alac_b.m4a", total + 1_000);
    let mut want = to_f32(&ramp(0, A_FRAMES, 2));
    want.extend(to_f32(&ramp(A_FRAMES as u64, B_FRAMES, 2)));
    assert_eq!(
        first_difference(&out[..total * 2], &want, 2),
        None,
        "the join is not sample-exact"
    );
    assert!(out[total * 2..].iter().all(|&s| s == 0.0));
}

// Opus: symphonia has no decoder for it.

#[test]
fn opus_fails_with_a_typed_error() {
    let error = Decoder::open(&MediaSource::file(fixture("opus.opus")))
        .err()
        .expect("Opus does not decode");
    assert_eq!(error, Error::Unsupported("Opus".into()));
    assert_eq!(error.to_string(), "unsupported format: Opus");
}

#[test]
fn an_opus_track_reports_the_error_and_the_queue_moves_on() {
    let (mut engine, handle, events) = inline_engine(2);
    let (opus, next) = (EntryId::fresh(), EntryId::fresh());
    handle
        .load(
            TrackSpec::new(opus, MediaSource::file(fixture("opus.opus"))),
            0,
            true,
        )
        .unwrap();
    render(&mut engine, 4_096, 512);
    assert_eq!(
        drain(&events),
        vec![
            EngineEvent::Error {
                id: opus,
                message: "unsupported format: Opus".into(),
            },
            EngineEvent::Finished,
        ]
    );
    // The player moves on from Finished; the engine itself is fine to load.
    handle
        .load(
            TrackSpec::new(next, MediaSource::file(fixture("mp3_b.mp3"))),
            0,
            true,
        )
        .unwrap();
    render(&mut engine, 4_096, 512);
    assert_eq!(drain(&events), vec![EngineEvent::TrackStarted { id: next }]);
}
