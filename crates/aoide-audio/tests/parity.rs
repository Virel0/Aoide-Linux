//! The tables the iOS and desktop apps answer, answered here with the same
//! numbers: `MixTransitionParityTests.swift` / `mix-transition-parity.test.ts`
//! (31 pairs) and `LoudnessNormalisationTests` / `loudness-parity.test.ts`
//! (9 inputs), copied row for row. Then the unit tests around them.

use aoide_audio::loudness::{REFERENCE_LUFS, gain_db, linear_gain};
use aoide_audio::mix::{
    MATCHED_BLEND_SECONDS, MixTrack, MixTransition, SoundBounds, UNMATCHED_BLEND_SECONDS, curve,
    is_album_run, plan_transition, tempi_agree, tempo_holds,
};

#[derive(Default)]
struct T {
    album: Option<&'static str>,
    number: Option<i64>,
    seconds: Option<Option<f64>>,
    bpm: Option<f64>,
    stability: Option<f64>,
    sound: Option<SoundBounds>,
}

/// The tables' `track(...)`: album "Album", 240 s, nothing measured.
fn track(t: T) -> MixTrack {
    MixTrack {
        album: t.album.unwrap_or("Album").to_owned(),
        track_number: t.number,
        duration_seconds: t.seconds.unwrap_or(Some(240.0)),
        sound: t.sound,
        bpm: t.bpm,
        bpm_stability: t.stability,
    }
}

struct Row {
    why: &'static str,
    outgoing: MixTrack,
    incoming: MixTrack,
    automix: bool,
    album_lock: bool,
    kind: &'static str,
    overlap: f64,
}

fn row(
    why: &'static str,
    outgoing: T,
    incoming: T,
    automix: bool,
    album_lock: bool,
    kind: &'static str,
    overlap: f64,
) -> Row {
    Row {
        why,
        outgoing: track(outgoing),
        incoming: track(incoming),
        automix,
        album_lock,
        kind,
        overlap,
    }
}

fn other(t: T) -> T {
    T {
        album: Some("Other"),
        ..t
    }
}

fn dark(number: i64, bpm: Option<f64>) -> T {
    T {
        album: Some("Dark Side"),
        number: Some(number),
        bpm,
        ..T::default()
    }
}

fn bpm(bpm: f64) -> T {
    T {
        bpm: Some(bpm),
        ..T::default()
    }
}

fn mix_table() -> Vec<Row> {
    vec![
        row(
            "the mixer off leaves the handover alone",
            bpm(128.0),
            other(bpm(128.0)),
            false,
            true,
            "cut",
            0.0,
        ),
        row(
            "an album run is gapless with the mixer off too",
            dark(4, None),
            dark(5, None),
            false,
            true,
            "gapless",
            0.0,
        ),
        row(
            "album lock is the mixer’s off-switch, never a second mixer",
            dark(4, None),
            dark(5, None),
            false,
            false,
            "cut",
            0.0,
        ),
        row(
            "a record that segues is never handed to the mixer",
            dark(4, Some(90.0)),
            dark(5, Some(140.0)),
            true,
            true,
            "gapless",
            0.0,
        ),
        row(
            "album lock off lets the mixer work across a record",
            dark(4, Some(128.0)),
            dark(5, Some(128.0)),
            true,
            false,
            "blend",
            8.0,
        ),
        row(
            "agreeing tempi earn the long blend",
            bpm(124.0),
            other(bpm(128.0)),
            true,
            true,
            "blend",
            8.0,
        ),
        row(
            "half time is one groove counted twice",
            bpm(85.0),
            other(bpm(170.0)),
            true,
            true,
            "blend",
            8.0,
        ),
        row(
            "two octaves apart is still one groove",
            bpm(128.0),
            other(bpm(32.0)),
            true,
            true,
            "blend",
            8.0,
        ),
        row(
            "genuinely different speeds get the short blend",
            bpm(100.0),
            other(bpm(128.0)),
            true,
            true,
            "blend",
            4.0,
        ),
        row(
            "six per cent apart is inside the tolerance",
            bpm(120.0),
            other(bpm(127.2)),
            true,
            true,
            "blend",
            8.0,
        ),
        row(
            "just past six per cent is outside it",
            bpm(120.0),
            other(bpm(128.0)),
            true,
            true,
            "blend",
            4.0,
        ),
        row(
            "nothing measured is not an agreement",
            T::default(),
            other(T::default()),
            true,
            true,
            "blend",
            4.0,
        ),
        row(
            "one measured tempo is not an agreement either",
            bpm(128.0),
            other(T::default()),
            true,
            true,
            "blend",
            4.0,
        ),
        row(
            "a zero tempo is not a measurement",
            bpm(0.0),
            other(bpm(128.0)),
            true,
            true,
            "blend",
            4.0,
        ),
        row(
            "out of order on one record is not a run",
            dark(9, Some(128.0)),
            dark(1, Some(128.0)),
            true,
            true,
            "blend",
            8.0,
        ),
        row(
            "the next number on another record is not a run",
            dark(9, Some(128.0)),
            T {
                album: Some("Wish You Were Here"),
                number: Some(10),
                bpm: Some(128.0),
                ..T::default()
            },
            true,
            true,
            "blend",
            8.0,
        ),
        row(
            "two untitled albums are not one record",
            T {
                album: Some(""),
                number: Some(4),
                bpm: Some(128.0),
                ..T::default()
            },
            T {
                album: Some(""),
                number: Some(5),
                bpm: Some(128.0),
                ..T::default()
            },
            true,
            true,
            "blend",
            8.0,
        ),
        row(
            "one album without track numbers is not a run",
            T {
                album: Some("Dark Side"),
                bpm: Some(128.0),
                ..T::default()
            },
            T {
                album: Some("Dark Side"),
                bpm: Some(128.0),
                ..T::default()
            },
            true,
            true,
            "blend",
            8.0,
        ),
        row(
            "a short track keeps three quarters of itself",
            bpm(128.0),
            other(T {
                seconds: Some(Some(20.0)),
                ..bpm(128.0)
            }),
            true,
            true,
            "blend",
            5.0,
        ),
        row(
            "exactly a quarter of the shorter track is allowed",
            bpm(128.0),
            other(T {
                seconds: Some(Some(32.0)),
                ..bpm(128.0)
            }),
            true,
            true,
            "blend",
            8.0,
        ),
        row(
            "a short blend under the share is not lengthened to it",
            bpm(100.0),
            other(T {
                seconds: Some(Some(20.0)),
                ..bpm(128.0)
            }),
            true,
            true,
            "blend",
            4.0,
        ),
        row(
            "two seconds of overlap is still a transition",
            bpm(128.0),
            other(T {
                seconds: Some(Some(8.0)),
                ..bpm(128.0)
            }),
            true,
            true,
            "blend",
            2.0,
        ),
        row(
            "a hair under two seconds is not",
            bpm(128.0),
            other(T {
                seconds: Some(Some(7.9)),
                ..bpm(128.0)
            }),
            true,
            true,
            "cut",
            0.0,
        ),
        row(
            "a sting is too short to fade at all",
            bpm(128.0),
            other(T {
                seconds: Some(Some(6.0)),
                ..bpm(128.0)
            }),
            true,
            true,
            "cut",
            0.0,
        ),
        row(
            "an unknown length ahead is a cut, not a guess",
            bpm(128.0),
            other(T {
                seconds: Some(None),
                ..bpm(128.0)
            }),
            true,
            true,
            "cut",
            0.0,
        ),
        row(
            "an unknown length behind is a cut too",
            T {
                seconds: Some(None),
                ..bpm(128.0)
            },
            other(bpm(128.0)),
            true,
            true,
            "cut",
            0.0,
        ),
        row(
            "measured sound, not file length, is what there is to fade",
            bpm(128.0),
            other(T {
                seconds: Some(Some(120.0)),
                sound: Some(SoundBounds::new(100_000, 110_000)),
                ..bpm(128.0)
            }),
            true,
            true,
            "blend",
            2.5,
        ),
        row(
            "a track whose tempo does not hold is nothing to match to",
            T {
                stability: Some(0.2),
                ..bpm(128.0)
            },
            other(bpm(128.0)),
            true,
            true,
            "blend",
            4.0,
        ),
        row(
            "and neither is one coming up",
            bpm(128.0),
            other(T {
                stability: Some(0.4),
                ..bpm(128.0)
            }),
            true,
            true,
            "blend",
            4.0,
        ),
        row(
            "half the windows agreeing is steady enough",
            T {
                stability: Some(0.5),
                ..bpm(128.0)
            },
            other(T {
                stability: Some(0.5),
                ..bpm(128.0)
            }),
            true,
            true,
            "blend",
            8.0,
        ),
        row(
            "an unmeasured stability is not held against a track",
            bpm(128.0),
            other(T {
                stability: Some(0.9),
                ..bpm(128.0)
            }),
            true,
            true,
            "blend",
            8.0,
        ),
    ]
}

#[test]
fn plans_the_shared_table_identically() {
    let table = mix_table();
    assert_eq!(
        table.len(),
        31,
        "the table has thirty-one rows on every side"
    );
    let mut wrong = Vec::new();
    for row in &table {
        let plan = plan_transition(&row.outgoing, &row.incoming, row.automix, row.album_lock);
        if plan.kind() != row.kind || (plan.overlap_seconds() - row.overlap).abs() >= 1e-9 {
            wrong.push(format!(
                "{}: {:?}, want {} {}",
                row.why, plan, row.kind, row.overlap
            ));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn answers_the_shared_loudness_table_identically() {
    let table: [(f64, Option<f64>, Option<f64>); 9] = [
        (-9.0, None, Some(-9.0)),
        (-14.0, None, Some(-4.0)),
        (-24.0, None, None),
        (-18.0, None, None),
        (-20.0, Some(1.5), None),
        (-9.0, Some(-0.3), Some(-9.0)),
        (-6.0, Some(2.0), Some(-12.0)),
        (-30.0, Some(0.5), None),
        (-8.333, None, Some(-9.67)),
    ];
    for (lufs, peak, expected) in table {
        // Exact, as the desktop's `toBe` is.
        assert_eq!(
            gain_db(Some(lufs), peak),
            expected,
            "{lufs} LUFS, peak {peak:?}"
        );
    }
}

// The iOS and desktop unit tests around the tables.

fn plain(album: &str, number: Option<i64>, seconds: f64, bpm: Option<f64>) -> MixTrack {
    MixTrack {
        album: album.into(),
        track_number: number,
        duration_seconds: Some(seconds),
        bpm,
        ..MixTrack::default()
    }
}

#[test]
fn half_time_and_octaves_agree_and_different_speeds_do_not() {
    assert!(tempi_agree(Some(85.0), Some(170.0)));
    assert!(tempi_agree(Some(170.0), Some(86.0)));
    assert!(tempi_agree(Some(128.0), Some(32.0)));
    assert!(!tempi_agree(Some(100.0), Some(128.0)));
}

#[test]
fn a_tempo_that_is_missing_or_not_a_number_is_no_agreement() {
    assert!(!tempi_agree(None, Some(128.0)));
    assert!(!tempi_agree(Some(128.0), None));
    assert!(!tempi_agree(Some(0.0), Some(128.0)));
    assert!(!tempi_agree(Some(f64::NAN), Some(128.0)));
    assert!(!tempi_agree(Some(f64::INFINITY), Some(128.0)));
    assert!(!tempi_agree(Some(-128.0), Some(-128.0)));
}

#[test]
fn stability_missing_holds_and_nan_does_not() {
    assert!(tempo_holds(None));
    assert!(!tempo_holds(Some(f64::NAN)));
    assert!(tempo_holds(Some(0.5)));
    assert!(!tempo_holds(Some(0.49)));
}

#[test]
fn steady_tracks_keep_the_long_blend_and_unsteady_ones_lose_it() {
    let steady = MixTrack {
        bpm_stability: Some(1.0),
        ..plain("Album", None, 240.0, Some(128.0))
    };
    let ramping = MixTrack {
        bpm_stability: Some(0.1),
        ..plain("Other", None, 240.0, Some(128.0))
    };
    assert_eq!(
        plan_transition(&steady, &ramping, true, true),
        MixTransition::Blend {
            seconds: UNMATCHED_BLEND_SECONDS
        }
    );
    assert_eq!(
        plan_transition(&ramping, &steady, true, true),
        MixTransition::Blend {
            seconds: UNMATCHED_BLEND_SECONDS
        }
    );
    let one = MixTrack {
        bpm_stability: Some(0.9),
        ..plain("Album", None, 240.0, Some(128.0))
    };
    let two = MixTrack {
        bpm_stability: Some(0.62),
        ..plain("Other", None, 240.0, Some(126.0))
    };
    assert_eq!(
        plan_transition(&one, &two, true, true),
        MixTransition::Blend {
            seconds: MATCHED_BLEND_SECONDS
        }
    );
}

#[test]
fn a_length_that_is_not_a_number_is_a_cut() {
    let base = plain("Album", None, 240.0, None);
    let infinite = MixTrack {
        duration_seconds: Some(f64::INFINITY),
        ..plain("Other", None, 0.0, None)
    };
    let nan = MixTrack {
        duration_seconds: Some(f64::NAN),
        ..plain("Album", None, 0.0, None)
    };
    assert_eq!(
        plan_transition(&base, &infinite, true, true),
        MixTransition::Cut
    );
    assert_eq!(
        plan_transition(&nan, &plain("Other", None, 240.0, None), true, true),
        MixTransition::Cut
    );
}

#[test]
fn bounds_that_end_before_they_start_leave_nothing_to_fade() {
    let backwards = MixTrack {
        sound: Some(SoundBounds::new(40_000, 10_000)),
        ..plain("Other", None, 240.0, None)
    };
    assert_eq!(
        plan_transition(&plain("Album", None, 240.0, None), &backwards, true, true),
        MixTransition::Cut
    );
}

#[test]
fn an_album_run_needs_the_same_named_album_and_the_next_number() {
    let four = plain("Dark Side", Some(4), 240.0, None);
    assert!(is_album_run(
        &four,
        &plain("Dark Side", Some(5), 240.0, None)
    ));
    assert!(!is_album_run(
        &four,
        &plain("Dark Side", Some(6), 240.0, None)
    ));
    assert!(!is_album_run(
        &four,
        &plain("Dark Side", Some(4), 240.0, None)
    ));
}

#[test]
fn the_curve_starts_and_ends_where_a_fade_should_and_holds_its_power() {
    assert!((curve::outgoing(0.0) - 1.0).abs() < 1e-9);
    assert!(curve::outgoing(1.0).abs() < 1e-9);
    assert!(curve::incoming(0.0).abs() < 1e-9);
    assert!((curve::incoming(1.0) - 1.0).abs() < 1e-9);
    for step in 0..=20 {
        let p = f64::from(step) / 20.0;
        let (out, into) = (curve::outgoing(p), curve::incoming(p));
        assert!((out * out + into * into - 1.0).abs() < 1e-9);
    }
    assert!((curve::outgoing(0.5) - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-9);
    assert_eq!(curve::outgoing(-1.0), 1.0);
    assert_eq!(curve::incoming(2.0), 1.0);
    assert_eq!(curve::incoming(f64::NAN), 0.0);
}

#[test]
fn loudness_edges_match_the_desktop() {
    assert_eq!(REFERENCE_LUFS, -18.0);
    assert_eq!(gain_db(Some(-9.7), None), Some(-8.3));
    assert_eq!(gain_db(Some(-6.0), None), Some(-12.0));
    assert_eq!(gain_db(Some(-23.0), None), None);
    assert_eq!(gain_db(Some(-30.4), None), None);
    assert_eq!(gain_db(Some(-17.99), None), Some(-0.01));
    assert_eq!(gain_db(Some(-18.01), None), None);
    assert_eq!(gain_db(Some(-17.998), None), None);
    assert_eq!(gain_db(Some(-17.994), None), Some(-0.01));
    assert_eq!(gain_db(None, None), None);
    assert_eq!(gain_db(Some(f64::NAN), None), None);
    assert_eq!(gain_db(Some(f64::INFINITY), None), None);
    assert_eq!(gain_db(Some(-9.7), Some(-0.3)), Some(-8.3));
    assert_eq!(gain_db(Some(-17.5), Some(0.0)), Some(-0.5));
    assert_eq!(gain_db(Some(-17.9), Some(1.5)), Some(-1.5));
    assert_eq!(gain_db(Some(-24.0), Some(0.3)), None);
    assert_eq!(gain_db(Some(-17.9), Some(0.4)), Some(-0.4));
    assert_eq!(gain_db(Some(-9.7), Some(f64::NAN)), Some(-8.3));
}

#[test]
fn the_linear_factor_matches_the_decibels_and_never_exceeds_one() {
    let factor = linear_gain(gain_db(Some(-12.0), None));
    assert!((f64::from(factor) - 10f64.powf(-6.0 / 20.0)).abs() < 1e-6);
    assert_eq!(linear_gain(None), 1.0);
    assert_eq!(linear_gain(Some(6.0)), 1.0, "never louder");
}
