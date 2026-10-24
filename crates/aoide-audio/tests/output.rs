//! The output's recovery when its stream fails, on a stand-in for the device:
//! the test plays the device callback, and fails the stream when it likes.

mod common;

use std::any::Any;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aoide_audio::{
    DecodeMode, DeviceFormat, EngineEvent, EntryId, Error, MediaSource, MixTransition, OpenStream,
    Output, Render, StreamReport, TrackSpec,
};
use common::*;

const A_FRAMES: usize = 2 * RATE as usize;
const B_FRAMES: usize = RATE as usize;
/// What the stand-in device holds before it plays: 100 ms.
const LATENCY: u64 = 4_800;

struct Device {
    /// What the default device would choose.
    default: DeviceFormat,
    /// The format each open asked for.
    opened: Vec<DeviceFormat>,
    /// Opens still to refuse.
    refuse: usize,
    /// The open stream's callback and error report.
    stream: Option<(Render, StreamReport)>,
    closed: usize,
}

#[derive(Clone)]
struct StandIn {
    device: Arc<Mutex<Device>>,
}

struct Stream(Arc<Mutex<Device>>);

impl Drop for Stream {
    fn drop(&mut self) {
        let mut device = self.0.lock().unwrap();
        device.stream = None;
        device.closed += 1;
    }
}

impl OpenStream for StandIn {
    fn default_format(&mut self) -> aoide_audio::Result<DeviceFormat> {
        Ok(self.device.lock().unwrap().default)
    }

    fn open(
        &mut self,
        format: DeviceFormat,
        render: Render,
        report: StreamReport,
    ) -> aoide_audio::Result<Box<dyn Any>> {
        let mut device = self.device.lock().unwrap();
        if device.refuse > 0 {
            device.refuse -= 1;
            return Err(Error::Output("no device".into()));
        }
        device.opened.push(format);
        device.stream = Some((render, report));
        Ok(Box::new(Stream(Arc::clone(&self.device))))
    }
}

impl StandIn {
    fn new() -> Self {
        Self {
            device: Arc::new(Mutex::new(Device {
                default: DeviceFormat {
                    sample_rate: RATE,
                    channels: 2,
                },
                opened: Vec::new(),
                refuse: 0,
                stream: None,
                closed: 0,
            })),
        }
    }

    /// Plays `frames` frames through the open stream's callback.
    fn play(&self, frames: usize, latency: u64) -> Vec<f32> {
        let mut device = self.device.lock().unwrap();
        let (render, _) = device.stream.as_mut().expect("an open stream");
        let mut out = vec![0.0; frames * 2];
        for chunk in out.chunks_mut(480 * 2) {
            render.render(chunk, latency);
        }
        out
    }

    fn report(&self) -> StreamReport {
        let device = self.device.lock().unwrap();
        device.stream.as_ref().expect("an open stream").1.clone()
    }

    /// Waits for the `n`th open.
    fn wait_for_open(&self, n: usize) {
        let started = Instant::now();
        while self.device.lock().unwrap().opened.len() < n {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "no reopen after 5 s"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// A playing A, with B chained gaplessly after it: one unbroken ramp.
fn two_tracks(handle: &aoide_audio::EngineHandle, dir: &Dir) -> (EntryId, EntryId) {
    let (a, b) = (dir.path("a.wav"), dir.path("b.wav"));
    write_wav(&a, RATE, 2, &ramp(0, A_FRAMES, 2));
    write_wav(&b, RATE, 2, &ramp(A_FRAMES as u64, B_FRAMES, 2));
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
    (id_a, id_b)
}

#[test]
fn a_failed_stream_reopens_and_plays_on_from_what_was_heard() {
    let device = StandIn::new();
    let (output, handle, events) = Output::start(device.clone(), DecodeMode::Inline).unwrap();
    let dir = Dir::new();
    let (id_a, id_b) = two_tracks(&handle, &dir);

    // 1 s rendered, the last 100 ms of it still in the device: 900 ms heard.
    let played = 48_000;
    let before = device.play(played, LATENCY);
    assert_eq!(before, to_f32(&ramp(0, played, 2)));
    assert_eq!(handle.position_ms(), 900);

    // The device goes. The new default would choose 44.1 kHz, but the engine
    // is made for 48 kHz and asks for that.
    device.device.lock().unwrap().default.sample_rate = 44_100;
    device.report().failed("device unplugged".into());
    device.wait_for_open(2);
    assert_eq!(device.device.lock().unwrap().closed, 1);

    // The new stream plays on from the heard frame, through the gapless join
    // to B: the queue is the engine's, untouched.
    let heard = played - LATENCY as usize;
    let rest = A_FRAMES + B_FRAMES - heard;
    let after = device.play(rest + 1_000, 0);
    let mut want = to_f32(&ramp(heard as u64, rest, 2));
    want.extend(std::iter::repeat_n(0.0, 2_000));
    let first_wrong = after
        .iter()
        .zip(&want)
        .position(|(got, want)| got != want)
        .map(|sample| sample / 2);
    assert_eq!(first_wrong, None, "the reopened stream does not carry on");

    let seen = drain(&events);
    println!("{seen:?}");
    assert_eq!(
        seen,
        vec![
            EngineEvent::TrackStarted { id: id_a },
            EngineEvent::DeviceError {
                message: "device unplugged".into()
            },
            EngineEvent::DeviceRestored,
            // A opened again at 900 ms, as a seek does.
            EngineEvent::TrackStarted { id: id_a },
            EngineEvent::TrackEnded { id: id_a },
            EngineEvent::TrackStarted { id: id_b },
            EngineEvent::TrackEnded { id: id_b },
            EngineEvent::Finished,
        ]
    );
    assert_eq!(
        device.device.lock().unwrap().opened,
        vec![
            DeviceFormat {
                sample_rate: RATE,
                channels: 2
            };
            2
        ]
    );
    drop(output);
    assert_eq!(device.device.lock().unwrap().closed, 2);
}

#[test]
fn a_device_that_will_not_open_is_tried_again() {
    let device = StandIn::new();
    let (_output, handle, events) = Output::start(device.clone(), DecodeMode::Inline).unwrap();
    let dir = Dir::new();
    two_tracks(&handle, &dir);
    device.play(9_600, LATENCY);

    device.device.lock().unwrap().refuse = 3;
    let failed = Instant::now();
    device.report().failed("gone".into());
    device.wait_for_open(2);
    // Waits of 50, 100 and 250 ms between the four tries.
    let took = failed.elapsed();
    println!("reopened after {took:?}");
    assert!(took >= Duration::from_millis(400), "{took:?}");

    let errors: Vec<_> = drain(&events)
        .into_iter()
        .filter(|e| !matches!(e, EngineEvent::TrackStarted { .. }))
        .collect();
    // The refusals are reported once, not on every try.
    assert_eq!(
        errors,
        vec![
            EngineEvent::DeviceError {
                message: "gone".into()
            },
            EngineEvent::DeviceError {
                message: "audio output: no device".into()
            },
            EngineEvent::DeviceRestored,
        ]
    );
    // Heard 100 ms, so the new stream starts there.
    let after = device.play(480, 0);
    assert_eq!(after, to_f32(&ramp(9_600 - LATENCY, 480, 2)));
}

#[test]
fn a_glitch_is_reported_and_nothing_reopens() {
    let device = StandIn::new();
    let (_output, handle, events) = Output::start(device.clone(), DecodeMode::Inline).unwrap();
    let dir = Dir::new();
    two_tracks(&handle, &dir);
    device.play(4_800, LATENCY);
    device.report().glitch("underrun".into());
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(device.device.lock().unwrap().opened.len(), 1);
    assert!(drain(&events).contains(&EngineEvent::DeviceError {
        message: "underrun".into()
    }));
    // The same stream plays straight on.
    assert_eq!(device.play(480, LATENCY), to_f32(&ramp(4_800, 480, 2)));
}

#[test]
fn a_late_report_from_a_replaced_stream_is_ignored() {
    let device = StandIn::new();
    let (_output, handle, _events) = Output::start(device.clone(), DecodeMode::Inline).unwrap();
    let dir = Dir::new();
    two_tracks(&handle, &dir);
    device.play(4_800, LATENCY);
    let old = device.report();
    old.failed("gone".into());
    device.wait_for_open(2);
    old.failed("gone, again".into());
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(device.device.lock().unwrap().opened.len(), 2);
}

#[test]
fn an_output_that_cannot_start_says_why() {
    let device = StandIn::new();
    device.device.lock().unwrap().refuse = 1;
    let error = Output::start(device, DecodeMode::Inline).err();
    assert_eq!(error, Some(Error::Output("no device".into())));
}

// The engine's side, on its own.

#[test]
fn resuming_from_heard_waits_out_a_pause() {
    // Paused, nothing was lost: what was heard is where the decks stopped.
    let dir = Dir::new();
    let (mut engine, handle, _events) = inline_engine(2);
    two_tracks(&handle, &dir);
    engine.set_output_latency_frames(LATENCY);
    render(&mut engine, 48_000, 480);
    handle.pause().unwrap();
    render(&mut engine, 9_600, 480);
    let paused_at = handle.position_ms();
    engine.resume_from_heard();
    handle.play().unwrap();
    render(&mut engine, 4_800, 480);
    assert!(handle.position_ms() > paused_at);
    assert!(handle.position_ms() < paused_at + 200);
}

#[test]
fn resuming_from_heard_across_an_unheard_join_plays_on() {
    // A's end was rendered but not yet heard: B carries on as rendered, and
    // at most the latency is lost.
    let dir = Dir::new();
    let (mut engine, handle, _events) = inline_engine(2);
    let (_, id_b) = two_tracks(&handle, &dir);
    engine.set_output_latency_frames(LATENCY);
    let rendered = A_FRAMES + 1_000;
    render(&mut engine, rendered, 480);
    engine.resume_from_heard();
    let out = render(&mut engine, 480, 480);
    assert_eq!(out, to_f32(&ramp(rendered as u64, 480, 2)));
    assert_eq!(handle.current(), Some(id_b));
}
