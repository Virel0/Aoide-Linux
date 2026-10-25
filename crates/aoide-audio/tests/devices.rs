//! Choosing an output device and switching while playing, on stand-in
//! devices behind the `OpenStream` seam: the test plays each stream's
//! callback itself.

mod common;

use std::any::Any;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aoide_audio::{
    DecodeMode, DeviceChoice, DeviceFormat, EngineEvent, EngineHandle, EntryId, Error, MediaSource,
    MixTransition, OpenStream, Output, OutputDevice, Render, StreamReport, TrackSpec,
};
use common::*;

const A_FRAMES: usize = 2 * RATE as usize;
const B_FRAMES: usize = RATE as usize;
const LATENCY: u64 = 4_800;
const DEFAULT: &str = "Built-in";

#[derive(Default)]
struct Devices {
    /// Present devices by name, with their own formats.
    present: BTreeMap<String, DeviceFormat>,
    /// Which device each open went to, and at what format.
    opened: Vec<(String, DeviceFormat)>,
    stream: Option<(String, Render, StreamReport)>,
    closed: usize,
}

#[derive(Clone)]
struct StandIn(Arc<Mutex<Devices>>);

struct Stream(Arc<Mutex<Devices>>);

impl Drop for Stream {
    fn drop(&mut self) {
        let mut devices = self.0.lock().unwrap();
        devices.stream = None;
        devices.closed += 1;
    }
}

fn choice(name: &str) -> DeviceChoice {
    DeviceChoice::Device {
        id: format!("test:{name}"),
        name: name.into(),
    }
}

fn format(sample_rate: u32) -> DeviceFormat {
    DeviceFormat {
        sample_rate,
        channels: 2,
    }
}

impl StandIn {
    fn new(devices: &[(&str, u32)]) -> Self {
        let mut all = Devices::default();
        all.present.insert(DEFAULT.into(), format(RATE));
        for &(name, rate) in devices {
            all.present.insert(name.into(), format(rate));
        }
        Self(Arc::new(Mutex::new(all)))
    }

    fn name(device: &DeviceChoice) -> String {
        match device {
            DeviceChoice::FollowDefault => DEFAULT.into(),
            DeviceChoice::Device { name, .. } => name.clone(),
        }
    }

    fn unplug(&self, name: &str) {
        self.0.lock().unwrap().present.remove(name);
    }

    fn opened(&self) -> Vec<(String, DeviceFormat)> {
        self.0.lock().unwrap().opened.clone()
    }

    fn playing_device(&self) -> String {
        self.0
            .lock()
            .unwrap()
            .stream
            .as_ref()
            .expect("a stream")
            .0
            .clone()
    }

    /// Plays `frames` frames through the open stream's callback.
    fn play(&self, frames: usize, latency: u64) -> Vec<f32> {
        let mut devices = self.0.lock().unwrap();
        let (_, render, _) = devices.stream.as_mut().expect("an open stream");
        let mut out = vec![0.0; frames * 2];
        for chunk in out.chunks_mut(480 * 2) {
            render.render(chunk, latency);
        }
        out
    }

    fn report(&self) -> StreamReport {
        self.0
            .lock()
            .unwrap()
            .stream
            .as_ref()
            .expect("a stream")
            .2
            .clone()
    }

    fn wait_for_open(&self, n: usize) {
        let started = Instant::now();
        while self.0.lock().unwrap().opened.len() < n {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "no open after 5 s"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl OpenStream for StandIn {
    fn default_format(&mut self) -> aoide_audio::Result<DeviceFormat> {
        self.device_format(&DeviceChoice::FollowDefault)
    }

    fn open(
        &mut self,
        format: DeviceFormat,
        render: Render,
        report: StreamReport,
    ) -> aoide_audio::Result<Box<dyn Any>> {
        self.open_device(&DeviceChoice::FollowDefault, format, render, report)
    }

    fn device_format(&mut self, device: &DeviceChoice) -> aoide_audio::Result<DeviceFormat> {
        let name = Self::name(device);
        self.0
            .lock()
            .unwrap()
            .present
            .get(&name)
            .copied()
            .ok_or_else(|| Error::Output(format!("{name} is not there")))
    }

    fn open_device(
        &mut self,
        device: &DeviceChoice,
        format: DeviceFormat,
        render: Render,
        report: StreamReport,
    ) -> aoide_audio::Result<Box<dyn Any>> {
        let name = Self::name(device);
        let mut devices = self.0.lock().unwrap();
        if !devices.present.contains_key(&name) {
            return Err(Error::Output(format!("{name} is not there")));
        }
        devices.opened.push((name.clone(), format));
        devices.stream = Some((name, render, report));
        Ok(Box::new(Stream(Arc::clone(&self.0))))
    }
}

/// A playing A, with B chained gaplessly after it: one unbroken ramp.
fn two_tracks(handle: &EngineHandle, dir: &Dir) -> (EntryId, EntryId) {
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

fn first_difference(got: &[f32], want: &[f32]) -> Option<usize> {
    got.iter()
        .zip(want)
        .position(|(got, want)| got != want)
        .map(|sample| sample / 2)
}

#[test]
fn starting_on_a_chosen_device_makes_the_engine_for_it() {
    let devices = StandIn::new(&[("USB DAC", 44_100)]);
    let (output, _handle, _events) =
        Output::start_with(devices.clone(), DecodeMode::Inline, choice("USB DAC")).unwrap();
    assert_eq!(output.config().sample_rate, 44_100);
    assert_eq!(devices.opened(), vec![("USB DAC".into(), format(44_100))]);
    assert_eq!(output.playing_on(), choice("USB DAC"));
    assert_eq!(output.device(), &choice("USB DAC"));
}

#[test]
fn a_chosen_device_missing_at_the_start_gives_way_to_the_default() {
    let devices = StandIn::new(&[]);
    let (output, _handle, events) =
        Output::start_with(devices.clone(), DecodeMode::Inline, choice("USB DAC")).unwrap();
    assert_eq!(devices.opened(), vec![(DEFAULT.into(), format(RATE))]);
    assert_eq!(output.playing_on(), DeviceChoice::FollowDefault);
    assert_eq!(output.device(), &choice("USB DAC"), "the choice is kept");
    let seen = drain(&events);
    println!("{seen:?}");
    assert_eq!(
        seen,
        vec![EngineEvent::DeviceError {
            message: "audio output: USB DAC is not there; playing on the default device".into()
        }]
    );
}

#[test]
fn switching_devices_while_playing_carries_on_from_what_was_heard() {
    let devices = StandIn::new(&[("USB DAC", 96_000)]);
    let (mut output, handle, events) = Output::start(devices.clone(), DecodeMode::Inline).unwrap();
    let dir = Dir::new();
    let (id_a, id_b) = two_tracks(&handle, &dir);

    // 1 s rendered on the built-in output, its last 100 ms still in the
    // device: 900 ms heard.
    let played = 48_000;
    assert_eq!(devices.play(played, LATENCY), to_f32(&ramp(0, played, 2)));
    assert_eq!(handle.position_ms(), 900);

    output.set_device(choice("USB DAC")).unwrap();
    devices.wait_for_open(2);
    assert_eq!(devices.playing_device(), "USB DAC");
    assert_eq!(
        devices.0.lock().unwrap().closed,
        1,
        "the old stream stopped"
    );
    // At the engine's rate, not the DAC's own.
    assert_eq!(devices.opened()[1], ("USB DAC".into(), format(RATE)));
    assert_eq!(output.playing_on(), choice("USB DAC"));

    // On the DAC, from the heard frame on, through the join to B.
    let heard = played - LATENCY as usize;
    let rest = A_FRAMES + B_FRAMES - heard;
    let after = devices.play(rest, LATENCY);
    assert_eq!(
        first_difference(&after, &to_f32(&ramp(heard as u64, rest, 2))),
        None,
        "the new device does not carry on from the heard frame"
    );
    println!(
        "switched at {} ms heard; the DAC's first frame is frame {heard} of the ramp",
        heard * 1000 / RATE as usize
    );
    let seen = drain(&events);
    println!("{seen:?}");
    assert_eq!(
        seen,
        vec![
            EngineEvent::TrackStarted { id: id_a },
            // A opened again at 900 ms, as a seek does.
            EngineEvent::TrackStarted { id: id_a },
            EngineEvent::TrackEnded { id: id_a },
            EngineEvent::TrackStarted { id: id_b },
        ],
        "a switch that works is not an error"
    );

    // And back to following the default.
    output.set_device(DeviceChoice::FollowDefault).unwrap();
    devices.wait_for_open(3);
    assert_eq!(devices.playing_device(), DEFAULT);
    assert_eq!(output.playing_on(), DeviceChoice::FollowDefault);
}

#[test]
fn a_chosen_device_that_goes_away_gives_way_to_the_default() {
    let devices = StandIn::new(&[("USB DAC", RATE)]);
    let (output, handle, events) =
        Output::start_with(devices.clone(), DecodeMode::Inline, choice("USB DAC")).unwrap();
    let dir = Dir::new();
    let (id_a, _) = two_tracks(&handle, &dir);
    let played = 48_000;
    devices.play(played, LATENCY);

    devices.unplug("USB DAC");
    devices.report().failed("device unplugged".into());
    devices.wait_for_open(2);
    assert_eq!(devices.playing_device(), DEFAULT);
    assert_eq!(output.playing_on(), DeviceChoice::FollowDefault);
    assert_eq!(output.device(), &choice("USB DAC"));

    let heard = played - LATENCY as usize;
    let after = devices.play(4_800, 0);
    assert_eq!(
        first_difference(&after, &to_f32(&ramp(heard as u64, 4_800, 2))),
        None
    );
    let seen = drain(&events);
    println!("{seen:?}");
    assert_eq!(
        seen,
        vec![
            EngineEvent::TrackStarted { id: id_a },
            EngineEvent::DeviceError {
                message: "device unplugged".into()
            },
            EngineEvent::DeviceError {
                message: "audio output: USB DAC is not there; playing on the default device".into()
            },
            EngineEvent::DeviceRestored,
            EngineEvent::TrackStarted { id: id_a },
        ]
    );
}

#[test]
fn the_choice_is_plain_data() {
    let listed = OutputDevice {
        id: "alsa:front:CARD=DAC,DEV=0".into(),
        name: "USB DAC".into(),
        is_default: false,
    };
    for choice in [DeviceChoice::FollowDefault, DeviceChoice::from(&listed)] {
        let json = serde_json::to_string(&choice).unwrap();
        println!("{json}");
        assert_eq!(serde_json::from_str::<DeviceChoice>(&json).unwrap(), choice);
    }
    assert_eq!(DeviceChoice::default(), DeviceChoice::FollowDefault);
}
