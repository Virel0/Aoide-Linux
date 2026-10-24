//! FLAC files symphonia 0.6.1 refuses on its own: a fixed block size whose
//! STREAMINFO gives the short last block as the minimum, as flacenc writes
//! them. The decoder tolerates them (decode.rs, `tolerate_flac_block_sizes`).

mod common;

use aoide_audio::decode::Decoder;
use aoide_audio::{EntryId, MediaSource, TrackSpec};
use common::*;

use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// STREAMINFO's minimum and maximum block sizes.
fn block_sizes(bytes: &[u8]) -> (u16, u16) {
    assert_eq!(&bytes[..4], b"fLaC");
    (
        u16::from_be_bytes([bytes[8], bytes[9]]),
        u16::from_be_bytes([bytes[10], bytes[11]]),
    )
}

/// Every frame through the engine's decoder.
fn decode(path: &std::path::Path) -> Vec<f32> {
    let mut decoder = Decoder::open(&MediaSource::file(path)).expect("open");
    let mut samples = Vec::new();
    while let Some(packet) = decoder.next_packet().expect("decode") {
        samples.extend_from_slice(packet.samples);
    }
    samples
}

/// symphonia without the workaround: the frames it reads, or its error.
fn symphonia_alone(bytes: Vec<u8>) -> Result<u64, String> {
    let mss = MediaSourceStream::new(Box::new(std::io::Cursor::new(bytes)), Default::default());
    let mut format = symphonia::default::get_probe()
        .probe(
            Hint::new().with_extension("flac"),
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| e.to_string())?;
    let track = format.default_track(TrackType::Audio).expect("a track").id;
    let mut frames = 0;
    loop {
        match format.next_packet() {
            Ok(Some(packet)) if packet.track_id == track => frames += packet.dur.get(),
            Ok(Some(_)) => {}
            Ok(None) => return Ok(frames),
            Err(error) => return Err(error.to_string()),
        }
    }
}

#[test]
fn flacenc_gives_the_short_last_block_as_the_minimum_and_symphonia_refuses_it() {
    let frames = 48_000 + 123;
    let bytes = flac_bytes(RATE, 2, &ramp(0, frames, 2));
    assert_eq!(block_sizes(&bytes), ((frames % 4096) as u16, 4096));
    let refused = symphonia_alone(bytes);
    println!("symphonia 0.6.1 alone: {refused:?}");
    // When this starts passing, symphonia has fixed it and the workaround can
    // go.
    assert!(refused.is_err(), "symphonia read it: {refused:?}");
}

#[test]
fn the_decoder_plays_a_flacenc_file_whole() {
    let dir = Dir::new();
    let path = dir.path("t.flac");
    let frames = 48_000 + 123;
    let samples = ramp(0, frames, 2);
    write_flac(&path, RATE, 2, &samples);
    assert_eq!(decode(&path), to_f32(&samples));
}

#[test]
fn a_last_block_under_sixteen_frames_plays_too() {
    // STREAMINFO says 7, under the 16 symphonia accepts as a minimum.
    let dir = Dir::new();
    let path = dir.path("t.flac");
    let frames = 3 * 4096 + 7;
    let samples = ramp(0, frames, 2);
    let bytes = flac_bytes(RATE, 2, &samples);
    assert_eq!(block_sizes(&bytes), (7, 4096));
    assert!(symphonia_alone(bytes.clone()).is_err());
    std::fs::write(&path, bytes).unwrap();
    assert_eq!(decode(&path), to_f32(&samples));
}

#[test]
fn a_file_of_whole_blocks_is_left_as_it_is() {
    let dir = Dir::new();
    let path = dir.path("t.flac");
    let samples = ramp(0, 4 * 4096, 2);
    let bytes = flac_bytes(RATE, 2, &samples);
    assert_eq!(block_sizes(&bytes), (4096, 4096));
    assert_eq!(symphonia_alone(bytes.clone()), Ok(4 * 4096));
    std::fs::write(&path, bytes).unwrap();
    assert_eq!(decode(&path), to_f32(&samples));
}

#[test]
fn a_flacenc_file_seeks_to_the_exact_frame() {
    let dir = Dir::new();
    let path = dir.path("t.flac");
    let frames = 48_000 + 123;
    let samples = ramp(0, frames, 2);
    write_flac(&path, RATE, 2, &samples);

    let (mut engine, handle, _events) = inline_engine(2);
    handle
        .load(
            TrackSpec::new(EntryId::fresh(), MediaSource::file(&path)),
            700,
            true,
        )
        .unwrap();
    let start = 700 * 48;
    let out = render(&mut engine, frames - start + 1_000, 512);
    let want = to_f32(&samples[start * 2..]);
    assert_eq!(&out[..want.len()], &want[..]);
    assert!(out[want.len()..].iter().all(|&s| s == 0.0));
}
