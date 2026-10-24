//! Port of `TasteProfileTests.swift` ("Taste ranking" and "Taste profile from history").

use std::collections::HashMap;

use super::store;
use crate::{
    FinishCounts, Millis, PlaySource, Record, Store, TasteCandidate, TasteProfile, TasteRanking,
    TrackRef,
};

fn profile() -> TasteProfile {
    TasteProfile {
        genres: HashMap::from([("metal".into(), 1.0), ("electronic".into(), 0.5)]),
        artists: HashMap::from([("sabaton".into(), 1.0), ("perturbator".into(), 0.4)]),
        recent: ["just-heard".to_owned()].into(),
        recent_artists: Default::default(),
    }
}

fn candidate(id: &str, artist: &str, genres: &[&str]) -> TasteCandidate {
    TasteCandidate::new(id, artist, genres)
}

fn score(candidate: &TasteCandidate, profile: &TasteProfile, finish: Option<FinishCounts>) -> f64 {
    TasteRanking::score(candidate, profile, finish, None)
}

fn rank(
    candidates: &[TasteCandidate],
    profile: &TasteProfile,
    compatibility: &HashMap<String, f64>,
    limit: usize,
) -> Vec<String> {
    TasteRanking::rank(candidates, profile, &HashMap::new(), compatibility, limit)
        .into_iter()
        .map(|c| c.id)
        .collect()
}

#[test]
fn a_favourite_genre_beats_an_unfamiliar_one() {
    let p = profile();
    assert!(
        score(&candidate("a", "Nobody", &["Metal"]), &p, None)
            > score(&candidate("b", "Nobody", &["Ambient"]), &p, None)
    );
}

#[test]
fn genres_are_matched_however_they_are_capitalised() {
    let p = profile();
    assert!(score(&candidate("a", "Nobody", &["METAL"]), &p, None) > 0.0);
    assert!(score(&candidate("a", "SABATON", &[]), &p, None) > 0.0);
}

#[test]
fn six_genres_are_not_six_times_as_relevant_as_the_right_one() {
    let p = profile();
    let one = candidate("a", "Nobody", &["Metal"]);
    let many = candidate(
        "b",
        "Nobody",
        &["Electronic", "Electronic", "Electronic", "Electronic"],
    );
    assert!(score(&one, &p, None) > score(&many, &p, None));
}

#[test]
fn a_familiar_artist_counts_but_for_less_than_the_genre() {
    let p = profile();
    let right_genre = candidate("a", "Nobody", &["Metal"]);
    let right_artist = candidate("b", "Sabaton", &["Ambient"]);
    assert!(score(&right_genre, &p, None) > score(&right_artist, &p, None));
    assert!(score(&right_artist, &p, None) > 0.0);
}

#[test]
fn a_track_you_always_skip_sinks_below_one_you_finish() {
    let p = profile();
    let song = candidate("a", "Nobody", &["Metal"]);
    let finished = score(&song, &p, Some(FinishCounts::new(10, 10)));
    let neutral = score(&song, &p, None);
    let skipped = score(&song, &p, Some(FinishCounts::new(10, 0)));
    assert!(finished > neutral);
    assert!(neutral > skipped);
}

#[test]
fn too_little_history_to_judge_is_not_held_against_a_track() {
    let p = profile();
    let song = candidate("a", "Nobody", &["Metal"]);
    assert_eq!(
        score(&song, &p, Some(FinishCounts::new(1, 0))),
        score(&song, &p, None)
    );
}

#[test]
fn something_just_heard_does_not_come_round_again_however_well_it_fits() {
    let p = profile();
    let perfect = candidate("just-heard", "Sabaton", &["Metal"]);
    let worst = candidate("new", "Nobody", &["Ambient"]);
    let perfect_score = score(&perfect, &p, Some(FinishCounts::new(10, 10)));
    let worst_score = score(&worst, &p, Some(FinishCounts::new(10, 0)));
    assert!(worst_score > perfect_score);
    assert_eq!(rank(&[perfect, worst], &p, &HashMap::new(), 1), ["new"]);
}

#[test]
fn a_record_that_can_follow_what_is_playing_outranks_one_that_cannot() {
    let p = profile();
    let a = candidate("a", "Nobody", &["Metal"]);
    let b = candidate("b", "Nobody", &["Metal"]);
    let compat = HashMap::from([("b".to_owned(), 0.9), ("a".to_owned(), 0.1)]);
    assert_eq!(rank(&[a.clone(), b], &p, &compat, 2), ["b", "a"]);

    let loved = candidate("loved", "Sabaton", &["Metal"]);
    let follows = candidate("follows", "Nobody", &["Metal"]);
    let compat = HashMap::from([("follows".to_owned(), 1.0), ("loved".to_owned(), 0.2)]);
    assert_eq!(
        rank(&[loved, follows], &p, &compat, 2),
        ["follows", "loved"]
    );

    let alone = score(&a, &p, None);
    assert_eq!(TasteRanking::score(&a, &p, None, None), alone);
}

#[test]
fn the_same_artist_twice_in_a_row_is_not_a_set() {
    let lately = TasteProfile {
        genres: HashMap::from([("metal".into(), 1.0)]),
        artists: HashMap::from([("sabaton".into(), 1.0)]),
        recent: Default::default(),
        recent_artists: ["sabaton".to_owned()].into(),
    };
    let again = candidate("again", "Sabaton", &["Metal"]);
    let someone_else = candidate("else", "Nobody", &["Metal"]);
    assert_eq!(
        rank(&[again, someone_else], &lately, &HashMap::new(), 2),
        ["else", "again"]
    );
}

#[test]
fn the_same_history_twice_gives_the_same_order() {
    let p = profile();
    let candidates = [
        candidate("c", "Nobody", &[]),
        candidate("a", "Nobody", &[]),
        candidate("b", "Nobody", &[]),
    ];
    let once = rank(&candidates, &p, &HashMap::new(), 3);
    let mut reversed = candidates.to_vec();
    reversed.reverse();
    let again = rank(&reversed, &p, &HashMap::new(), 3);
    assert_eq!(once, ["a", "b", "c"]);
    assert_eq!(once, again);
}

#[test]
fn asking_for_none_gets_none() {
    assert!(
        rank(
            &[candidate("a", "Nobody", &[])],
            &profile(),
            &HashMap::new(),
            0
        )
        .is_empty()
    );
}

// MARK: - Taste profile from history

fn play(store: &Store, id: &str, artist: &str, genres: &[&str], completed: bool, at: Millis) {
    let track = TrackRef::new(id, id, id, artist, "Album", at).with_genres(genres);
    store.write_local(|tx| track.save(tx)).unwrap();
    let event = store
        .begin_play(id, id, PlaySource::Album, None, at)
        .unwrap();
    store
        .finish_play(
            &event,
            if completed { 200_000 } else { 3_000 },
            Some(200_000),
            at + 1,
            None,
        )
        .unwrap();
}

fn read_profile(store: &Store) -> TasteProfile {
    store.taste_profile(0, 200, 40).unwrap()
}

#[test]
fn favourites_come_out_of_what_was_finished() {
    let store = store();
    for i in 0..6 {
        play(
            &store,
            &format!("m{i}"),
            "Sabaton",
            &["Metal", "Power Metal"],
            true,
            1_000 + i,
        );
    }
    play(&store, "e1", "Perturbator", &["Electronic"], true, 2_000);
    let profile = read_profile(&store);
    assert_eq!(profile.genres.get("metal"), Some(&1.0));
    assert!(
        profile
            .genres
            .get("electronic")
            .is_some_and(|w| *w < 1.0 && *w > 0.0)
    );
    assert_eq!(profile.artists.get("sabaton"), Some(&1.0));
}

#[test]
fn a_song_skipped_every_time_is_not_a_favourite() {
    let store = store();
    for i in 0..6 {
        play(
            &store,
            &format!("s{i}"),
            "Skipped",
            &["Skipped Genre"],
            false,
            1_000 + i,
        );
    }
    play(&store, "k1", "Kept", &["Kept Genre"], true, 2_000);
    let profile = read_profile(&store);
    assert_eq!(profile.genres.get("skipped genre"), None);
    assert_eq!(profile.genres.get("kept genre"), Some(&1.0));
}

#[test]
fn whatever_was_played_lately_counts_as_heard_skipped_or_not() {
    let store = store();
    play(&store, "skipped", "A", &["G"], false, 1_000);
    let profile = read_profile(&store);
    assert!(profile.recent.contains("skipped"));
    assert!(profile.recent_artists.contains("a"));
}

#[test]
fn nothing_listened_to_yet_is_a_profile_that_says_nothing() {
    let profile = read_profile(&store());
    assert!(profile.is_empty());
    assert!(profile.recent.is_empty());
}
