//! ReplayGain from file tags, and Level volume over the tags and the
//! sidecar's measurement, measured on offline renders of tagged fixtures
//! (tests/fixtures/README.md says how they were made).

mod common;

use std::path::PathBuf;
use std::sync::mpsc::Receiver;

use aoide_audio::decode::Decoder;
use aoide_audio::loudness::{ReplayGainTags, level_gain};
use aoide_audio::{
    Engine, EngineEvent, EntryId, MediaSource, MixTrack, Player, PlayerTrack, TrackSpec,
};
use common::*;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn tags(name: &str) -> ReplayGainTags {
    Decoder::open(&MediaSource::file(fixture(name)))
        .expect("open")
        .replay_gain()
}

#[test]
fn tags_are_read_from_vorbis_comments_id3_and_mp4() {
    let album = ReplayGainTags {
        track_gain_db: Some(-4.0),
        track_peak: Some(0.5),
        album_gain_db: Some(-7.0),
        album_peak: Some(0.6),
    };
    assert_eq!(tags("rg_album.flac"), album, "FLAC (Vorbis comments)");
    assert_eq!(tags("rg_album.mp3"), album, "MP3 (ID3v2.4 TXXX)");
    assert_eq!(
        tags("rg_album.m4a"),
        ReplayGainTags {
            track_peak: None,
            ..album
        },
        "M4A (iTunes ---- atoms)"
    );
    assert_eq!(
        tags("rg_track.ogg"),
        ReplayGainTags {
            track_gain_db: Some(-3.0),
            ..Default::default()
        },
        "Ogg Vorbis"
    );
    assert_eq!(
        tags("rg_boost.flac"),
        ReplayGainTags {
            track_gain_db: Some(9.0),
            track_peak: Some(0.5),
            ..Default::default()
        }
    );
    for untagged in ["alac_a.m4a", "mp3_a.mp3", "vorbis_a.ogg"] {
        assert!(!tags(untagged).is_present(), "{untagged} has no tags");
    }
}

#[test]
fn album_gain_first_and_the_peak_never_past_full_scale() {
    let both = ReplayGainTags {
        track_gain_db: Some(-4.0),
        track_peak: Some(0.5),
        album_gain_db: Some(-7.0),
        album_peak: Some(0.6),
    };
    assert!((db(both.linear_gain().unwrap()) - -7.0).abs() < 1e-9);
    let track_only = ReplayGainTags {
        album_gain_db: None,
        album_peak: None,
        ..both
    };
    assert!((db(track_only.linear_gain().unwrap()) - -4.0).abs() < 1e-9);
    // A boost goes as far as the peak allows, and no further.
    let boost = ReplayGainTags {
        track_gain_db: Some(9.0),
        track_peak: Some(0.5),
        ..Default::default()
    };
    assert!((boost.linear_gain().unwrap() - 2.0).abs() < 1e-12);
    // No peak tag: never above unity.
    let unknown_peak = ReplayGainTags {
        track_gain_db: Some(3.0),
        ..Default::default()
    };
    assert_eq!(unknown_peak.linear_gain(), Some(1.0));
    assert_eq!(ReplayGainTags::default().linear_gain(), None);
}

#[test]
fn level_volume_is_one_switch_and_the_sidecar_wins_over_tags() {
    let tagged = ReplayGainTags {
        track_gain_db: Some(-4.0),
        ..Default::default()
    };
    let untagged = ReplayGainTags::default();
    let measured = Some(-6.0);
    let in_db = |factor: f32| db(f64::from(factor));
    // Off: nothing, whatever the file or the sidecar says.
    assert_eq!(level_gain(false, Some(&tagged), measured), 1.0);
    assert_eq!(level_gain(false, Some(&untagged), measured), 1.0);
    // On: the sidecar's measurement wins over the file's tags...
    assert!((in_db(level_gain(true, Some(&tagged), measured)) - -6.0).abs() < 1e-5);
    assert!((in_db(level_gain(true, Some(&untagged), measured)) - -6.0).abs() < 1e-5);
    assert!((in_db(level_gain(true, None, measured)) - -6.0).abs() < 1e-5);
    // ...including its "leave it alone" for a track it measured as quiet.
    assert_eq!(level_gain(true, Some(&tagged), Some(0.0)), 1.0);
    // Unmeasured: the tags, else nothing.
    assert!((in_db(level_gain(true, Some(&tagged), None)) - -4.0).abs() < 1e-5);
    assert_eq!(level_gain(true, Some(&untagged), None), 1.0);
}

/// The level of a fixture rendered with its tags applied against without,
/// in dB, and the rendered peak with them.
fn rendered(name: &str, sidecar_db: Option<f64>) -> (f64, f32) {
    let play = |replay_gain: bool, gain_db: Option<f64>| {
        let (mut engine, handle, _) = inline_engine(2);
        let spec = TrackSpec::new(EntryId::fresh(), MediaSource::file(fixture(name)))
            .with_gain_db(gain_db)
            .with_replay_gain(replay_gain);
        handle.load(spec, 0, true).unwrap();
        render(&mut engine, RATE as usize / 2, 480)
    };
    let plain = play(false, None);
    let levelled = play(true, sidecar_db);
    let peak = levelled.iter().fold(0.0f32, |p, s| p.max(s.abs()));
    (db(rms(&levelled, 2) / rms(&plain, 2)), peak)
}

#[test]
fn the_render_applies_the_tags() {
    for (name, want) in [
        ("rg_album.flac", -7.0),
        ("rg_album.mp3", -7.0),
        ("rg_album.m4a", -7.0),
        ("rg_track.ogg", -3.0),
    ] {
        let (got, _) = rendered(name, None);
        println!("{name}: tags say {want:+.2} dB, rendered {got:+.4} dB");
        assert!((got - want).abs() < 0.01, "{name}: {got:+.4} dB");
    }
    // +9 dB asked, the peak tag (0.5, the tone's own) allows +6.02.
    let (got, peak) = rendered("rg_boost.flac", None);
    println!("rg_boost.flac: +9 dB asked, rendered {got:+.4} dB, peak {peak} (1.0 is 0 dBFS)");
    assert!((got - 20.0 * 2f64.log10()).abs() < 0.01);
    assert!(peak <= 1.0, "the peak went past full scale: {peak}");
    assert!(peak > 0.999, "the boost stopped short of the peak: {peak}");
}

#[test]
fn the_sidecar_wins_over_a_tagged_file() {
    // The album fixture (tags say −7 dB) with a sidecar gain of −6 dB:
    // −6, neither −7 nor both (−13).
    let (got, _) = rendered("rg_album.flac", Some(-6.0));
    println!("tagged, with a sidecar gain of -6 dB: {got:+.4} dB");
    assert!((got - -6.0).abs() < 0.01);
}

// Through the player

struct Rig {
    engine: Engine,
    events: Receiver<EngineEvent>,
    player: Player,
}

impl Rig {
    fn new() -> Self {
        let (engine, handle, events) = inline_engine(2);
        Self {
            engine,
            events,
            player: Player::new(handle),
        }
    }

    fn run(&mut self, frames: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames * 2);
        for _ in 0..frames.div_ceil(480) {
            out.extend(render(&mut self.engine, 480, 480));
            for event in self.events.try_iter().collect::<Vec<_>>() {
                self.player.handle(&event).unwrap();
            }
        }
        out
    }
}

fn track(path: PathBuf, gain_db: Option<f64>) -> PlayerTrack {
    PlayerTrack {
        key: "t".into(),
        source: MediaSource::file(path),
        mix: MixTrack::default(),
        gain_db,
    }
}

#[test]
fn the_players_switch_governs_both_sources() {
    let dir = Dir::new();
    let untagged = dir.path("untagged.wav");
    write_wav(
        &untagged,
        RATE,
        2,
        &tone(1_000.0, 0.5, RATE, RATE as usize / 2, 2),
    );
    let tagged = fixture("rg_album.flac");
    let level = |on: Option<bool>, path: &PathBuf, gain_db: Option<f64>| {
        let mut rig = Rig::new();
        if let Some(on) = on {
            rig.player.set_level_volume(on).unwrap();
        }
        rig.player
            .play_tracks(vec![track(path.clone(), gain_db)], 0, false)
            .unwrap();
        rms(&rig.run(RATE as usize / 4), 2)
    };
    let unity_tagged = level(Some(false), &tagged, None);
    let unity_untagged = level(Some(false), &untagged, None);

    let cases = [
        // On: the sidecar wins over the file's −7 dB tags.
        ("on, tagged", Some(true), &tagged, unity_tagged, -6.0),
        ("on, untagged", Some(true), &untagged, unity_untagged, -6.0),
        ("off, tagged", Some(false), &tagged, unity_tagged, 0.0),
        ("off, untagged", Some(false), &untagged, unity_untagged, 0.0),
        // Before the app says: the sidecar's gain as given, tags ignored.
        ("unset, tagged", None, &tagged, unity_tagged, -6.0),
    ];
    for (name, on, path, unity, want) in cases {
        let got = db(level(on, path, Some(-6.0)) / unity);
        println!("{name}, sidecar -6 dB: {got:+.4} dB (want {want:+.2})");
        assert!((got - want).abs() < 0.01, "{name}: {got:+.4} dB");
    }
}
