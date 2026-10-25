//! The player: queue, planner and transport rules on a rendered engine.

mod common;

use std::sync::mpsc::Receiver;

use aoide_audio::{
    Engine, EngineEvent, EntryId, MediaSource, MixTrack, MixTransition, Player, PlayerSettings,
    PlayerTrack, SoundBounds, Transport,
};
use common::*;

struct Rig {
    engine: Engine,
    events: Receiver<EngineEvent>,
    player: Player,
    _dir: Dir,
}

impl Rig {
    /// Three one-second ramp files that continue each other; `album` gives
    /// them one album and consecutive numbers.
    fn new(album: bool) -> (Rig, Vec<PlayerTrack>) {
        let dir = Dir::new();
        let frames = RATE as usize;
        let mut tracks = Vec::new();
        for n in 0..3 {
            let path = dir.path(&format!("{n}.wav"));
            write_wav(&path, RATE, 2, &ramp((n * frames) as u64, frames, 2));
            tracks.push(PlayerTrack {
                key: format!("track-{n}"),
                source: MediaSource::file(path),
                mix: MixTrack {
                    album: if album {
                        "Record".into()
                    } else {
                        format!("Single {n}")
                    },
                    track_number: Some(n as i64 + 1),
                    duration_seconds: Some(1.0),
                    ..MixTrack::default()
                },
                gain_db: None,
            });
        }
        let (engine, handle, events) = inline_engine(2);
        (
            Rig {
                engine,
                events,
                player: Player::new(handle),
                _dir: dir,
            },
            tracks,
        )
    }

    /// Renders, forwarding every engine event to the player as an app would.
    fn run(&mut self, frames: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames * 2);
        for _ in 0..frames.div_ceil(480) {
            out.extend(render(&mut self.engine, 480, 480));
            for event in self.events.try_iter().collect::<Vec<_>>() {
                self.player.handle(&event).unwrap();
            }
        }
        out.truncate(frames * 2);
        out
    }

    fn current(&self) -> usize {
        let id = self.player.queue().current().unwrap().id;
        self.player
            .queue()
            .entries()
            .iter()
            .position(|e| e.id == id)
            .unwrap()
    }
}

#[test]
fn a_queue_plays_through_gaplessly_and_follows_the_boundaries() {
    let (mut rig, tracks) = Rig::new(true);
    rig.player.play_tracks(tracks, 0, false).unwrap();
    assert_eq!(
        rig.player.planned().map(|(_, t)| t),
        Some(MixTransition::Gapless),
        "an album run"
    );
    let out = rig.run(3 * RATE as usize + 960);
    let want = to_f32(&ramp(0, 3 * RATE as usize, 2));
    assert_eq!(
        &out[..want.len()],
        &want[..],
        "three files, one unbroken ramp"
    );
    assert_eq!(rig.current(), 2, "the queue followed both boundaries");
    assert_eq!(rig.engine.transport(), Transport::Stopped);
}

#[test]
fn crossfade_on_blends_between_albums_and_album_lock_keeps_runs_gapless() {
    let (mut rig, tracks) = Rig::new(false);
    // One-second tracks allow a quarter second of overlap: below the floor.
    rig.player
        .set_settings(PlayerSettings {
            crossfade: true,
            album_lock: true,
        })
        .unwrap();
    rig.player.play_tracks(tracks.clone(), 0, false).unwrap();
    assert_eq!(
        rig.player.planned().map(|(_, t)| t),
        Some(MixTransition::Cut)
    );

    let long: Vec<PlayerTrack> = tracks
        .into_iter()
        .map(|mut t| {
            t.mix.duration_seconds = Some(240.0);
            t
        })
        .collect();
    rig.player.play_tracks(long.clone(), 0, false).unwrap();
    assert_eq!(
        rig.player.planned().map(|(_, t)| t),
        Some(MixTransition::Blend { seconds: 4.0 })
    );

    let album: Vec<PlayerTrack> = long
        .into_iter()
        .map(|mut t| {
            t.mix.album = "Record".into();
            t
        })
        .collect();
    rig.player.play_tracks(album, 0, false).unwrap();
    assert_eq!(
        rig.player.planned().map(|(_, t)| t),
        Some(MixTransition::Gapless)
    );
}

#[test]
fn previous_restarts_after_three_seconds_and_goes_back_before() {
    let dir = Dir::new();
    let mut tracks = Vec::new();
    for n in 0..2 {
        let path = dir.path(&format!("{n}.wav"));
        write_wav(&path, RATE, 2, &ramp(0, 5 * RATE as usize, 2));
        tracks.push(PlayerTrack {
            key: format!("track-{n}"),
            source: MediaSource::file(path),
            mix: MixTrack::default(),
            gain_db: None,
        });
    }
    let (engine, handle, events) = inline_engine(2);
    let mut rig = Rig {
        engine,
        events,
        player: Player::new(handle),
        _dir: dir,
    };
    rig.player.play_tracks(tracks, 1, false).unwrap();

    rig.run(4 * RATE as usize);
    assert!(rig.player.position_ms() > 3_000);
    rig.player.previous_track().unwrap();
    rig.run(480);
    assert_eq!(rig.current(), 1, "restarted, not moved");
    assert!(
        rig.player.position_ms() <= 10,
        "at {} ms",
        rig.player.position_ms()
    );

    rig.run(RATE as usize);
    rig.player.previous_track().unwrap();
    rig.run(480);
    assert_eq!(rig.current(), 0, "within three seconds: the previous track");
}

#[test]
fn next_moves_on_and_a_seek_keeps_the_next_track_chained() {
    let (mut rig, tracks) = Rig::new(true);
    rig.player.play_tracks(tracks, 0, false).unwrap();
    rig.run(4_800);
    rig.player.next_track().unwrap();
    rig.run(480);
    assert_eq!(rig.current(), 1);

    rig.player.seek(900).unwrap();
    // Faded in for 10 ms, then the ramp on from 0.9 s into track 2, straight
    // into track 3.
    let out = rig.run(RATE as usize / 10 + RATE as usize);
    let from = RATE as usize + 900 * 48;
    let want = to_f32(&ramp(from as u64, RATE as usize / 10 + RATE as usize, 2));
    assert_eq!(&out[960..], &want[960..out.len()]);
    assert_eq!(rig.current(), 2);
}

#[test]
fn a_track_that_fails_is_skipped() {
    let (mut rig, mut tracks) = Rig::new(false);
    tracks[0].source = MediaSource::file("/nonexistent/aoide-test.flac");
    rig.player.play_tracks(tracks, 0, false).unwrap();
    rig.run(4_800);
    assert_eq!(rig.current(), 1, "moved past the broken track");
    assert_eq!(rig.engine.transport(), Transport::Playing);
}

#[test]
fn keys_follow_their_tracks_through_every_edit() {
    let (mut rig, tracks) = Rig::new(true);
    let by_key = |key: &str| tracks.iter().find(|t| t.key == key).unwrap().source.clone();
    rig.player.play_tracks(tracks.clone(), 0, false).unwrap();
    let mut extra = tracks[2].clone();
    extra.key = "extra".into();
    rig.player
        .edit_queue(|q| {
            q.play_next(vec![extra]);
            q.set_shuffled(true);
            q.move_entries(&[3], 1);
        })
        .unwrap();
    rig.run(RATE as usize + 960);

    let keys: Vec<String> = rig
        .player
        .queue()
        .entries()
        .iter()
        .map(|e| e.track.key.clone())
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(sorted, ["extra", "track-0", "track-1", "track-2"]);
    for entry in rig.player.queue().entries() {
        if entry.track.key != "extra" {
            assert_eq!(
                format!("{:?}", entry.track.source),
                format!("{:?}", by_key(&entry.track.key))
            );
        }
        assert_eq!(
            rig.player.entry_by_key(&entry.track.key).map(|e| e.id),
            Some(entry.id)
        );
    }
    // What the engine reports as playing is found by its key too.
    let playing = rig.player.engine().current().unwrap();
    let key = &rig.player.queue().current().unwrap().track.key;
    assert_eq!(rig.player.entry_by_key(key).unwrap().id, playing);
    assert!(rig.player.entry_by_key("nothing").is_none());
}

#[test]
fn a_queue_loads_held_at_a_position_and_plays_on_from_there() {
    let (mut rig, tracks) = Rig::new(true);
    rig.player.load_tracks(tracks, 1, false, 500).unwrap();
    let held = rig.run(4_800);
    assert_eq!(rig.engine.transport(), Transport::Paused);
    assert!(held.iter().all(|s| *s == 0.0), "nothing sounds while held");
    assert_eq!(rig.current(), 1);
    assert_eq!(
        rig.player.planned().map(|(_, t)| t),
        Some(MixTransition::Gapless),
        "what follows is planned"
    );

    rig.player.play().unwrap();
    // Faded in as a resume is (150 ms), then the ramp from half a second
    // into track 2.
    let out = rig.run(RATE as usize / 5);
    let from = RATE as u64 + 500 * 48;
    let want = to_f32(&ramp(from, RATE as usize / 5, 2));
    let fade = 150 * 48 * 2;
    assert_eq!(&out[fade..], &want[fade..out.len()]);
    assert_eq!(rig.engine.transport(), Transport::Playing);
    assert!(
        (690..=710).contains(&rig.player.position_ms()),
        "at {} ms",
        rig.player.position_ms()
    );
}

#[test]
fn measurements_that_arrive_late_reach_the_next_track() {
    let (mut rig, tracks) = Rig::new(true);
    rig.player.play_tracks(tracks, 0, false).unwrap();
    rig.run(4_800);
    let entries: Vec<EntryId> = rig.player.queue().entries().iter().map(|e| e.id).collect();

    // The second track turns out to have half a second of silence at the
    // top and to need turning down.
    let bounds = Some(SoundBounds::new(500, 1_000));
    assert!(
        rig.player
            .set_measurements(entries[1], bounds, Some(-6.0))
            .unwrap()
    );
    let second = &rig.player.queue().entries()[1].track;
    assert_eq!(second.mix.sound, bounds);
    assert_eq!(second.gain_db, Some(-6.0));
    assert_eq!(rig.player.planned().map(|(id, _)| id), Some(entries[1]));

    // Through to the join: the second track is heard from 0.5 s, quieter.
    let rest = RATE as usize - 4_800;
    let out = rig.run(rest + RATE as usize / 4);
    let first = to_f32(&ramp(4_800, rest, 2));
    assert_eq!(
        &out[..first.len()],
        &first[..],
        "the current track is untouched"
    );
    let gain = 10f32.powf(-6.0 / 20.0);
    let joined = &out[first.len()..first.len() + 200];
    let want = to_f32(&ramp(RATE as u64 + 500 * 48, 100, 2));
    for (heard, plain) in joined.iter().zip(&want) {
        assert!(
            (heard - plain * gain).abs() < 1e-4,
            "{heard} vs {}",
            plain * gain
        );
    }
    assert_eq!(rig.current(), 1);

    assert!(
        !rig.player
            .set_measurements(EntryId(u64::MAX), None, None)
            .unwrap()
    );
}
