//! Port of `TasteRankingParityTests.swift` ("Taste ranking parity"): the sixteen
//! candidates the phone and the desktop score, with the same sixteen scores and the same
//! one order. The table is copied verbatim; a change here is a change on every client.

use std::collections::HashMap;

use crate::{FinishCounts, TasteCandidate, TasteProfile, TasteRanking};

struct ParityCase {
    why: &'static str,
    id: &'static str,
    artist: &'static str,
    genres: &'static [&'static str],
    finish: Option<FinishCounts>,
    score: f64,
}

const fn counts(starts: i64, completed: i64) -> Option<FinishCounts> {
    Some(FinishCounts { starts, completed })
}

const TABLE: [ParityCase; 16] = [
    ParityCase {
        why: "a favourite genre is most of what there is to earn",
        id: "metal-stranger",
        artist: "Nobody",
        genres: &["Metal"],
        finish: None,
        score: 0.45,
    },
    ParityCase {
        why: "an unfamiliar genre earns nothing, and is not held against a track",
        id: "ambient-stranger",
        artist: "Nobody",
        genres: &["Ambient"],
        finish: None,
        score: 0.0,
    },
    ParityCase {
        why: "a genre matches however it was capitalised",
        id: "shouted-genre",
        artist: "Nobody",
        genres: &["METAL"],
        finish: None,
        score: 0.45,
    },
    ParityCase {
        why: "so does an artist",
        id: "shouted-artist",
        artist: "SABATON",
        genres: &[],
        finish: None,
        score: 0.3,
    },
    ParityCase {
        why: "four genres are not four times as relevant as the right one",
        id: "four-times-electronic",
        artist: "Nobody",
        genres: &["Electronic", "Electronic", "Electronic", "Electronic"],
        finish: None,
        score: 0.225,
    },
    ParityCase {
        why: "the best match counts wherever in the tags it sits",
        id: "mixed-tags",
        artist: "Nobody",
        genres: &["Ambient", "Metal", "Electronic"],
        finish: None,
        score: 0.45,
    },
    ParityCase {
        why: "the favourite artist, the favourite genre and always finished is the top of the scale",
        id: "favourite-everything",
        artist: "Sabaton",
        genres: &["Metal"],
        finish: counts(10, 10),
        score: 1.05,
    },
    ParityCase {
        why: "a track you never finish sinks, but its genre still counts for it",
        id: "always-skipped",
        artist: "Nobody",
        genres: &["Metal"],
        finish: counts(10, 0),
        score: 0.15,
    },
    ParityCase {
        why: "one skip out of one says nothing, and scores as no history at all",
        id: "barely-judged",
        artist: "Nobody",
        genres: &["Metal"],
        finish: counts(1, 0),
        score: 0.45,
    },
    ParityCase {
        why: "two finishes out of two is still under the sample floor",
        id: "two-of-two",
        artist: "Nobody",
        genres: &[],
        finish: counts(2, 2),
        score: 0.0,
    },
    ParityCase {
        why: "three is the first sample either app will speak from",
        id: "three-of-three",
        artist: "Nobody",
        genres: &[],
        finish: counts(3, 3),
        score: 0.3,
    },
    ParityCase {
        why: "and it counts against a track exactly as far as it counts for one",
        id: "none-of-three",
        artist: "Nobody",
        genres: &[],
        finish: counts(3, 0),
        score: -0.3,
    },
    ParityCase {
        why: "two thirds finished is a third of the way up the band",
        id: "two-of-three",
        artist: "Nobody",
        genres: &[],
        finish: counts(3, 2),
        score: 0.1,
    },
    ParityCase {
        why: "a second-favourite genre and artist together still lose to a favourite genre alone",
        id: "second-favourite",
        artist: "Perturbator",
        genres: &["Electronic"],
        finish: None,
        score: 0.345,
    },
    ParityCase {
        why: "the best song there is, heard an hour ago, falls below the worst that has not been",
        id: "heard-an-hour-ago",
        artist: "Sabaton",
        genres: &["Metal"],
        finish: counts(10, 10),
        score: -0.35,
    },
    ParityCase {
        why: "nothing familiar and nothing measured is the middle of the scale",
        id: "nothing-known",
        artist: "Nobody",
        genres: &[],
        finish: None,
        score: 0.0,
    },
];

/// Four score exactly 0.45 and three exactly 0, so this pins the id tie-break too.
const ORDER: [&str; 16] = [
    "favourite-everything",
    "barely-judged",
    "metal-stranger",
    "mixed-tags",
    "shouted-genre",
    "second-favourite",
    "shouted-artist",
    "three-of-three",
    "four-times-electronic",
    "always-skipped",
    "two-of-three",
    "ambient-stranger",
    "nothing-known",
    "two-of-two",
    "none-of-three",
    "heard-an-hour-ago",
];

fn profile() -> TasteProfile {
    TasteProfile {
        genres: HashMap::from([("electronic".into(), 0.5), ("metal".into(), 1.0)]),
        artists: HashMap::from([("perturbator".into(), 0.4), ("sabaton".into(), 1.0)]),
        recent: ["heard-an-hour-ago".to_owned()].into(),
        recent_artists: Default::default(),
    }
}

fn candidates() -> Vec<TasteCandidate> {
    TABLE
        .iter()
        .map(|row| TasteCandidate::new(row.id, row.artist, row.genres))
        .collect()
}

fn finish() -> HashMap<String, FinishCounts> {
    TABLE
        .iter()
        .filter_map(|row| row.finish.map(|f| (row.id.to_owned(), f)))
        .collect()
}

/// Six places, the way the TypeScript copy rounds: schoolbook, away from zero.
fn rounded(score: f64) -> f64 {
    (score * 1_000_000.0).round() / 1_000_000.0
}

fn ranked(limit: usize) -> Vec<String> {
    TasteRanking::rank(&candidates(), &profile(), &finish(), &HashMap::new(), limit)
        .into_iter()
        .map(|c| c.id)
        .collect()
}

#[test]
fn scores_the_desktops_table_identically() {
    let profile = profile();
    for row in &TABLE {
        let candidate = TasteCandidate::new(row.id, row.artist, row.genres);
        assert_eq!(
            rounded(TasteRanking::score(&candidate, &profile, row.finish, None)),
            row.score,
            "{}",
            row.why
        );
    }
}

#[test]
fn puts_the_whole_table_in_one_order_ties_broken_on_id() {
    assert_eq!(ranked(TABLE.len()), ORDER);
}

#[test]
fn takes_the_best_five_when_five_are_asked_for() {
    assert_eq!(ranked(5), ORDER[..5]);
}

#[test]
fn asking_for_none_gets_none() {
    assert!(ranked(0).is_empty());
}

#[test]
fn answers_every_case_in_the_table() {
    assert_eq!(TABLE.len(), 16);
    assert_eq!(ORDER.len(), TABLE.len());
}
