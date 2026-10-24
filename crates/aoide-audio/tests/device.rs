//! The real output device. Ignored by default: run with `--ignored` on a
//! machine with sound. Silent on purpose (volume zero): it checks that the
//! callback runs, the latency is read and the heard position moves, not
//! what the speaker sounds like.

mod common;

use std::time::{Duration, Instant};

use aoide_audio::{EngineEvent, EntryId, MediaSource, Output, TrackSpec};
use common::*;

#[test]
#[ignore = "needs an audio output device"]
fn the_default_device_plays_and_reports_the_heard_position() {
    let (output, handle, events) = Output::start_default().expect("an output device");
    let config = output.config();
    println!(
        "device: {} Hz, {} channels",
        config.sample_rate, config.channels
    );

    let dir = Dir::new();
    let path = dir.path("tone.wav");
    write_wav(&path, 44_100, 2, &tone(440.0, 0.2, 44_100, 2 * 44_100, 2));
    handle.set_volume(0.0).unwrap();
    let id = EntryId::fresh();
    handle
        .load(TrackSpec::new(id, MediaSource::file(&path)), 0, true)
        .unwrap();

    let started = Instant::now();
    let mut heard_start = None;
    while started.elapsed() < Duration::from_millis(1_500) {
        if let Ok(event) = events.recv_timeout(Duration::from_millis(50))
            && event == (EngineEvent::TrackStarted { id })
        {
            heard_start = Some(started.elapsed());
        }
    }
    let position = handle.position_ms();
    println!("heard the start after {heard_start:?}; position {position} ms after 1.5 s");
    assert!(heard_start.is_some(), "no TrackStarted from the device");
    assert!(
        (900..=1_600).contains(&position),
        "position {position} ms after 1.5 s"
    );
}
