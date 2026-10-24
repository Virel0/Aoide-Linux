//! Port of `PlayHistoryTests.swift`: "Play classification", "Play history",
//! "Finish rate" and "Activity tags".

use super::{all, pending, store};
use crate::{
    Activity, FinishCounts, FinishRate, Like, Millis, Op, PlayClassifier, PlayEvent, PlaySource,
    PlayWeighting, Record, Store, TrackLink, TrackRef, TrackStats, now_millis,
};

// MARK: - Play classification

fn classify(ms: i64, duration: Option<i64>) -> crate::PlayOutcome {
    PlayClassifier::classify(ms, duration, None)
}

/// The boundary itself, from both sides.
#[test]
fn exactly_half_a_track_is_a_play_a_millisecond_short_is_neither() {
    let exactly = classify(45_000, Some(90_000));
    assert!(exactly.counts_as_play);
    assert!(!exactly.skipped);
    let just_under = classify(44_999, Some(90_000));
    assert!(!just_under.counts_as_play);
    assert!(!just_under.skipped);
}

#[test]
fn the_fixed_and_fractional_halves_each_cover_what_the_other_cannot() {
    let short = classify(30_000, Some(90_000));
    assert!(!short.counts_as_play && !short.skipped);
    let long = classify(300_000, Some(720_000));
    assert!(long.counts_as_play && !long.skipped);
    let bailed = classify(120_000, Some(720_000));
    assert!(!bailed.counts_as_play && bailed.skipped);
}

#[test]
fn a_four_second_skip_is_a_skip() {
    let outcome = classify(4_000, Some(180_000));
    assert!(!outcome.counts_as_play);
    assert!(outcome.skipped);
}

#[test]
fn a_completed_ten_second_track_is_a_play_however_short() {
    let outcome = classify(10_000, Some(10_000));
    assert!(outcome.completed && outcome.counts_as_play && !outcome.skipped);
}

#[test]
fn finishing_a_beat_short_of_the_duration_still_counts_as_finishing() {
    assert!(
        classify(
            200_000 - PlayClassifier::COMPLETION_TOLERANCE_MS,
            Some(200_000)
        )
        .completed
    );
    let outside = classify(
        200_000 - PlayClassifier::COMPLETION_TOLERANCE_MS - 1,
        Some(200_000),
    );
    assert!(!outside.completed);
    assert!(outside.counts_as_play);
}

#[test]
fn the_players_word_on_completion_overrides_the_arithmetic() {
    let outcome = PlayClassifier::classify(1_000, Some(300_000), Some(true));
    assert!(outcome.completed && outcome.counts_as_play && !outcome.skipped);
}

#[test]
fn with_no_duration_only_the_duration_free_half_of_the_rule_applies() {
    assert!(classify(240_000, None).counts_as_play);
    assert!(!classify(239_999, None).counts_as_play);
    assert!(!classify(239_999, None).skipped);
    assert!(classify(47_999, None).skipped);
    assert!(!classify(999_999, None).completed);
}

#[test]
fn a_zero_duration_is_treated_as_no_duration() {
    let outcome = classify(0, Some(0));
    assert!(!outcome.completed && !outcome.counts_as_play && outcome.skipped);
    assert!(classify(240_000, Some(0)).counts_as_play);
}

#[test]
fn no_listen_is_both_a_play_and_a_skip() {
    for duration in [
        None,
        Some(0),
        Some(1),
        Some(10_000),
        Some(90_000),
        Some(240_000),
        Some(720_000),
    ] {
        for ms in [
            0, 1, 4_000, 47_999, 48_000, 120_000, 239_999, 240_000, 720_000,
        ] {
            let outcome = classify(ms, duration);
            assert!(
                !(outcome.counts_as_play && outcome.skipped),
                "{duration:?} {ms}"
            );
        }
    }
}

// MARK: - Play history

fn cache(store: &Store, id: &str, artist: &str, album: &str, duration: Option<i64>) {
    let mut track = TrackRef::new(id, id, id, artist, album, 0);
    track.duration_ms = duration;
    store.write_local(|tx| track.insert(tx)).unwrap();
}

fn cached(store: &Store, id: &str, duration: Option<i64>) {
    cache(store, id, "Artist", "Album", duration);
}

fn play(store: &Store, id: &str, at: Millis, ms: i64, duration: Option<i64>) -> PlayEvent {
    let event = store
        .begin_play(id, id, PlaySource::Album, None, at)
        .unwrap();
    store
        .finish_play(&event, ms, duration, at + ms, None)
        .unwrap()
}

fn ids(list: &[String]) -> Vec<&str> {
    list.iter().map(String::as_str).collect()
}

#[test]
fn finishing_replaces_the_unsynced_begin_op_rather_than_queueing_a_second() {
    let store = store();
    let event = store
        .begin_play("t1", "k1", PlaySource::Unknown, None, 1_000)
        .unwrap();
    assert_eq!(pending(&store).len(), 1);
    store
        .finish_play(&event, 180_000, Some(200_000), 181_000, None)
        .unwrap();

    let ops = pending(&store);
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].entity, "playEvent");
    assert_eq!(ops[0].entity_id, event.id);
    assert!(ops[0].payload.contains("\"msPlayed\":180000"));
    let rows = all::<PlayEvent>(&store);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].ms_played, 180_000);
    assert_eq!(rows[0].ended_at, Some(181_000));
}

/// A synced op cannot be replaced: the correction is a new op that lands after it.
#[test]
fn finishing_after_a_sync_appends_a_correction_under_a_fresh_op_id() {
    let store = store();
    let event = store
        .begin_play("t1", "k1", PlaySource::Unknown, None, 1_000)
        .unwrap();
    let begin = pending(&store).remove(0);
    store
        .mark_synced(std::slice::from_ref(&begin.op_id))
        .unwrap();
    store
        .finish_play(&event, 180_000, Some(200_000), 181_000, None)
        .unwrap();

    let waiting = pending(&store);
    assert_eq!(waiting.len(), 1);
    assert_ne!(waiting[0].op_id, begin.op_id);
    assert!(waiting[0].payload.contains("\"msPlayed\":180000"));
    let every = all::<Op>(&store);
    assert_eq!(every.len(), 2);
    assert!(every.iter().any(|op| op.op_id == begin.op_id && op.synced));
}

/// An app killed mid-play must not invent an outcome.
#[test]
fn a_play_that_never_finished_counts_as_neither_a_play_nor_a_skip() {
    let store = store();
    cached(&store, "t1", Some(200_000));
    store
        .begin_play("t1", "k1", PlaySource::Unknown, None, 1_000)
        .unwrap();
    let stats = store.stats("t1").unwrap();
    assert_eq!(stats.play_count, 0);
    assert_eq!(stats.skip_count, 0);
    assert_eq!(stats.last_played_at, None);
    assert_eq!(stats.last_started_at, Some(1_000));
    assert!(stats.has_history());
    assert_eq!(store.recently_played(50).unwrap(), ["t1"]);
}

#[test]
fn a_one_shot_record_writes_a_classified_event_and_its_op() {
    let store = store();
    cached(&store, "t1", Some(200_000));
    store
        .record_play(
            &TrackLink::new("t1", "k1"),
            1_000,
            5_000,
            Some(200_000),
            PlaySource::Unknown,
            None,
        )
        .unwrap();
    assert_eq!(store.stats("t1").unwrap().skip_count, 1);
    assert_eq!(pending(&store).len(), 1);
}

#[test]
fn play_counts_and_skips_add_up_across_a_mixed_history() {
    let store = store();
    cached(&store, "t1", Some(200_000));
    for index in 0..3 {
        play(&store, "t1", 10_000 + index * 1_000, 200_000, Some(200_000));
    }
    for index in 0..2 {
        play(&store, "t1", 20_000 + index * 1_000, 5_000, Some(200_000));
    }
    play(&store, "t1", 30_000, 70_000, Some(200_000));

    let stats = store.stats("t1").unwrap();
    assert_eq!(stats.play_count, 3);
    assert_eq!(stats.skip_count, 2);
    assert_eq!(stats.total_ms_played, 3 * 200_000 + 2 * 5_000 + 70_000);
    assert_eq!(stats.first_played_at, Some(10_000));
    assert_eq!(stats.last_played_at, Some(12_000));
    assert_eq!(stats.last_started_at, Some(30_000));
    assert_eq!(store.play_count("t1").unwrap(), 3);
}

#[test]
fn the_sql_threshold_agrees_with_the_classifier_to_the_millisecond() {
    let store = store();
    cached(&store, "on", Some(200_000));
    cached(&store, "under", Some(200_000));
    play(&store, "on", 1_000, 100_000, Some(200_000));
    play(&store, "under", 1_000, 99_999, Some(200_000));
    assert_eq!(store.play_count("on").unwrap(), 1);
    assert_eq!(store.play_count("under").unwrap(), 0);
}

/// The stored skip flag wins over a stale cached duration.
#[test]
fn an_event_the_player_called_a_skip_never_also_counts_as_a_play() {
    let store = store();
    cached(&store, "t1", Some(60_000));
    play(&store, "t1", 1_000, 50_000, Some(720_000));
    let stats = store.stats("t1").unwrap();
    assert_eq!(stats.skip_count, 1);
    assert_eq!(stats.play_count, 0);
}

#[test]
fn a_play_of_an_uncached_track_still_counts_using_the_duration_free_rule() {
    let store = store();
    play(&store, "ghost", 1_000, 250_000, None);
    assert_eq!(store.play_count("ghost").unwrap(), 1);
    assert_eq!(store.unattributed_plays(0, Millis::MAX).unwrap(), 1);
    assert!(store.top_artists(0, 10).unwrap().is_empty());
}

#[test]
fn the_batched_stats_agree_with_the_single_track_form() {
    let store = store();
    for id in ["t1", "t2", "t3"] {
        cached(&store, id, Some(200_000));
    }
    play(&store, "t1", 1_000, 200_000, Some(200_000));
    play(&store, "t1", 2_000, 5_000, Some(200_000));
    play(&store, "t2", 3_000, 120_000, Some(200_000));

    let wanted: Vec<String> = ["t1", "t2", "t3"].map(str::to_owned).to_vec();
    let batched = store.stats_for(&wanted).unwrap();
    for id in &wanted {
        assert_eq!(batched[id], store.stats(id).unwrap());
    }
    assert_eq!(batched["t1"].play_count, 1);
    assert_eq!(batched["t1"].skip_count, 1);
    assert_eq!(batched["t2"].play_count, 1);
}

#[test]
fn every_requested_id_comes_back_zeroed_when_it_has_no_history() {
    let store = store();
    let stats = store
        .stats_for(&["a", "b", "c"].map(str::to_owned))
        .unwrap();
    assert_eq!(stats.len(), 3);
    assert_eq!(stats["b"], TrackStats::empty("b"));
    assert!(!stats["b"].has_history());
}

#[test]
fn stats_survive_a_batch_larger_than_sqlites_parameter_limit() {
    let store = store();
    let wanted: Vec<String> = (0..1_100).map(|i| format!("t{i}")).collect();
    cached(&store, "t7", Some(200_000));
    play(&store, "t7", 1_000, 200_000, Some(200_000));
    let stats = store.stats_for(&wanted).unwrap();
    assert_eq!(stats.len(), wanted.len());
    assert_eq!(stats["t7"].play_count, 1);
    assert_eq!(stats["t999"].play_count, 0);
}

#[test]
fn top_tracks_rank_by_qualifying_plays_and_ties_break_deterministically() {
    let store = store();
    for id in ["a", "b", "c"] {
        cached(&store, id, Some(200_000));
    }
    for index in 0..3 {
        play(&store, "c", 1_000 + index, 200_000, Some(200_000));
    }
    play(&store, "a", 5_000, 200_000, Some(200_000));
    play(&store, "b", 6_000, 200_000, Some(200_000));
    for index in 0..9 {
        play(&store, "b", 7_000 + index, 2_000, Some(200_000));
    }
    let top = store.top_tracks(0, 3).unwrap();
    assert_eq!(
        top.iter()
            .map(|t| t.jellyfin_id.as_str())
            .collect::<Vec<_>>(),
        ["c", "a", "b"]
    );
    assert_eq!(
        top.iter().map(|t| t.play_count).collect::<Vec<_>>(),
        [3, 1, 1]
    );
    assert_eq!(store.top_tracks(0, 1).unwrap().len(), 1);
}

#[test]
fn top_artists_and_albums_come_from_the_track_cache() {
    let store = store();
    cache(&store, "t1", "Aphex Twin", "SAW", Some(200_000));
    cache(&store, "t2", "Aphex Twin", "SAW", Some(200_000));
    cache(&store, "t3", "Boards of Canada", "MHTRTC", Some(200_000));
    play(&store, "t1", 1_000, 200_000, Some(200_000));
    play(&store, "t2", 2_000, 200_000, Some(200_000));
    play(&store, "t3", 3_000, 200_000, Some(200_000));

    let artists = store.top_artists(0, 10).unwrap();
    assert_eq!(
        artists
            .iter()
            .map(|a| a.artist.as_str())
            .collect::<Vec<_>>(),
        ["Aphex Twin", "Boards of Canada"]
    );
    assert_eq!(
        artists.iter().map(|a| a.play_count).collect::<Vec<_>>(),
        [2, 1]
    );
    let albums = store.top_albums(0, 10).unwrap();
    assert_eq!(
        albums.iter().map(|a| a.album.as_str()).collect::<Vec<_>>(),
        ["SAW", "MHTRTC"]
    );
    assert_eq!(
        albums.iter().map(|a| a.play_count).collect::<Vec<_>>(),
        [2, 1]
    );
}

/// Two records called "Greatest Hits" are two records.
#[test]
fn albums_with_the_same_title_by_different_artists_stay_apart() {
    let store = store();
    cache(&store, "t1", "One", "Greatest Hits", Some(200_000));
    cache(&store, "t2", "Two", "Greatest Hits", Some(200_000));
    play(&store, "t1", 1_000, 200_000, Some(200_000));
    play(&store, "t2", 2_000, 200_000, Some(200_000));
    let albums = store.top_albums(0, 10).unwrap();
    assert_eq!(albums.len(), 2);
    let mut artists: Vec<&str> = albums.iter().map(|a| a.artist.as_str()).collect();
    artists.sort();
    assert_eq!(artists, ["One", "Two"]);
}

#[test]
fn recently_played_is_distinct_and_most_recent_first() {
    let store = store();
    for id in ["a", "b"] {
        cached(&store, id, Some(200_000));
    }
    play(&store, "a", 1_000, 200_000, Some(200_000));
    play(&store, "b", 2_000, 200_000, Some(200_000));
    play(&store, "a", 3_000, 200_000, Some(200_000));
    assert_eq!(ids(&store.recently_played(50).unwrap()), ["a", "b"]);
    assert_eq!(ids(&store.recently_played(1).unwrap()), ["a"]);
}

/// One skip out of one play is not a 100% skip rate.
#[test]
fn skip_rate_stays_silent_until_there_is_enough_evidence() {
    let store = store();
    cached(&store, "t1", Some(200_000));
    for index in 0..3 {
        play(&store, "t1", 1_000 + index, 200_000, Some(200_000));
    }
    play(&store, "t1", 4_000, 5_000, Some(200_000));
    assert_eq!(store.skip_rate("t1").unwrap(), None);
    play(&store, "t1", 5_000, 5_000, Some(200_000));
    assert_eq!(store.skip_rate("t1").unwrap(), Some(0.4));
}

#[test]
fn a_track_with_no_history_has_no_skip_rate_rather_than_a_rate_of_zero() {
    assert_eq!(store().skip_rate("nothing").unwrap(), None);
}

// MARK: - Recap

#[test]
fn a_recap_totals_the_window_and_names_the_busiest_day() {
    let store = store();
    // Midday UTC on two consecutive days stays two local days in any time zone.
    let day_one: Millis = 1_699_963_200_000;
    let day_two = day_one + 86_400_000;
    cache(&store, "t1", "Aphex Twin", "SAW", Some(200_000));
    cache(&store, "t2", "Boards of Canada", "MHTRTC", Some(200_000));
    for index in 0..3 {
        play(
            &store,
            "t1",
            day_one + index * 3_600_000,
            200_000,
            Some(200_000),
        );
    }
    play(&store, "t2", day_one + 4 * 3_600_000, 5_000, Some(200_000));
    play(&store, "t2", day_two, 200_000, Some(200_000));
    play(&store, "t9", day_two + 3_600_000, 250_000, None);

    let recap = store.recap(day_one, day_one + 3 * 86_400_000).unwrap();
    assert_eq!(recap.total_plays, 5);
    assert_eq!(
        recap.total_ms_played,
        3 * 200_000 + 5_000 + 200_000 + 250_000
    );
    assert_eq!(recap.distinct_tracks, 3);
    assert_eq!(recap.distinct_artists, 2);
    assert_eq!(recap.distinct_albums, 2);
    assert_eq!(recap.unattributed_plays, 1);
    assert_eq!(
        recap
            .top_tracks
            .iter()
            .map(|t| t.jellyfin_id.as_str())
            .collect::<Vec<_>>(),
        ["t1", "t2", "t9"]
    );
    assert_eq!(recap.top_artists[0].artist, "Aphex Twin");
    assert_eq!(recap.top_albums[0].album, "SAW");
    assert_eq!(recap.busiest_day.unwrap().play_count, 3);
}

#[test]
fn the_recap_window_includes_its_start_and_excludes_its_end() {
    let store = store();
    cached(&store, "t1", Some(200_000));
    play(&store, "t1", 1_000, 200_000, Some(200_000));
    play(&store, "t1", 2_000, 200_000, Some(200_000));
    assert_eq!(store.recap(1_000, 2_000).unwrap().total_plays, 1);
    assert_eq!(store.recap(2_000, 3_000).unwrap().total_plays, 1);
    assert_eq!(store.recap(0, 1_000).unwrap().total_plays, 0);
}

#[test]
fn an_empty_recap_is_zeros_rather_than_a_failure() {
    let recap = store().recap(0, 1_000).unwrap();
    assert_eq!(recap.total_plays, 0);
    assert_eq!(recap.total_ms_played, 0);
    assert_eq!(recap.busiest_day, None);
    assert!(recap.top_tracks.is_empty());
}

// MARK: - Weighted shuffle

#[test]
fn no_track_is_ever_weighted_to_zero_however_badly_it_scores() {
    assert_eq!(
        PlayWeighting::weight(0, 1_000, Some(false)),
        PlayWeighting::FLOOR
    );
    assert!(PlayWeighting::weight(0, 0, Some(false)) > 0.0);
    assert!(PlayWeighting::weight(0, 0, None) > 0.0);
}

#[test]
fn one_skip_out_of_one_listen_only_nudges_the_weight() {
    let neutral = PlayWeighting::weight(0, 0, None);
    let once_skipped = PlayWeighting::weight(0, 1, None);
    assert!(once_skipped < neutral);
    assert!(once_skipped > neutral * 0.6);
}

#[test]
fn plays_raise_the_weight_with_diminishing_returns() {
    let none = PlayWeighting::weight(0, 0, None);
    let some = PlayWeighting::weight(10, 0, None);
    let many = PlayWeighting::weight(100, 0, None);
    assert!(some > none);
    assert!(many > some);
    assert!(many < none * 10.0);
}

#[test]
fn weights_read_likes_plays_and_skips_out_of_the_store() {
    let store = store();
    for id in ["loved", "neutral", "skipped", "disliked"] {
        cached(&store, id, Some(200_000));
    }
    for index in 0..5 {
        play(&store, "loved", 1_000 + index, 200_000, Some(200_000));
        play(&store, "neutral", 2_000 + index, 200_000, Some(200_000));
        play(&store, "skipped", 3_000 + index, 2_000, Some(200_000));
    }
    store
        .record(
            Like::new("loved", "loved", true, 0, ""),
            false,
            now_millis(),
        )
        .unwrap();
    store
        .record(
            Like::new("disliked", "disliked", false, 0, ""),
            false,
            now_millis(),
        )
        .unwrap();

    let wanted = ["loved", "neutral", "skipped", "disliked", "unheard"].map(str::to_owned);
    let weights = store.play_weights(&wanted).unwrap();
    assert_eq!(weights.len(), 5);
    assert!(weights["unheard"] > 0.0);
    assert!(weights["loved"] > weights["neutral"]);
    assert!(weights["skipped"] < weights["neutral"]);
    assert!(weights["disliked"] < weights["unheard"]);
    assert!(weights["skipped"] > 0.0);
    assert!(weights["disliked"] > 0.0);
}

#[test]
fn an_unliked_row_that_was_undone_stops_counting_against_a_track() {
    let store = store();
    let like = store
        .record(Like::new("t1", "t1", false, 0, ""), false, now_millis())
        .unwrap();
    let wanted = ["t1".to_owned()];
    let penalised = store.play_weights(&wanted).unwrap()["t1"];
    store.record(like, true, now_millis()).unwrap();
    let restored = store.play_weights(&wanted).unwrap()["t1"];
    assert!(restored > penalised);
}

// MARK: - Finish rate

#[test]
fn too_little_history_says_nothing_at_all() {
    assert_eq!(FinishRate::rate(FinishCounts::new(2, 2)), None);
    assert_eq!(FinishRate::rate(FinishCounts::new(0, 0)), None);
    assert_eq!(FinishRate::rate(FinishCounts::new(3, 3)), Some(1.0));
}

#[test]
fn a_set_sums_its_counts_rather_than_averaging_its_rates() {
    let heavy = FinishCounts::new(50, 0);
    let light = FinishCounts::new(3, 3);
    let combined = FinishRate::rate_over([heavy, light]).unwrap();
    assert!((combined - 3.0 / 53.0).abs() < 0.0001);
    let mean_of_rates =
        (FinishRate::rate(heavy).unwrap_or(0.0) + FinishRate::rate(light).unwrap_or(0.0)) / 2.0;
    assert!(
        (combined - mean_of_rates).abs() > 0.4,
        "must not be the mean of the rates"
    );
}

#[test]
fn counted_from_the_store_only_decided_listens_every_id_present() {
    let store = store();
    for completed in [true, true, false] {
        let event = store
            .begin_play("t1", "k1", PlaySource::Unknown, None, now_millis())
            .unwrap();
        store
            .finish_play(
                &event,
                if completed { 200_000 } else { 3_000 },
                Some(200_000),
                now_millis(),
                None,
            )
            .unwrap();
    }
    // One still open: no verdict, so it counts for nothing.
    store
        .begin_play("t1", "k1", PlaySource::Unknown, None, now_millis())
        .unwrap();

    let counts = store
        .finish_counts(&["t1".to_owned(), "never-played".to_owned()])
        .unwrap();
    assert_eq!(counts["never-played"], FinishCounts::new(0, 0));
    let t1 = counts["t1"];
    assert_eq!(t1.starts, 3, "the open listen must not count");
    assert_eq!(t1.completed, 2);
    assert_eq!(FinishRate::percent_description(t1).as_deref(), Some("67%"));
}

#[test]
fn nothing_asked_for_nothing_returned() {
    let store = store();
    assert!(store.finish_counts(&[]).unwrap().is_empty());
    assert_eq!(store.finish_rate(&[]).unwrap(), None);
}

// MARK: - Activity tags

#[test]
fn the_set_is_closed_and_anything_outside_it_is_dropped() {
    assert_eq!(Activity::parse(Some("gaming")), Some(Activity::Gaming));
    assert_eq!(Activity::parse(None), None);
    assert_eq!(Activity::parse(Some("dancing")), None);
    assert_eq!(Activity::parse(Some("")), None);
    assert_eq!(
        Activity::parse(Some("Gaming")),
        None,
        "the raw value is exact"
    );
    let mut all: Vec<&str> = Activity::ALL.iter().map(|a| a.as_str()).collect();
    all.sort();
    assert_eq!(all, ["chores", "commute", "focus", "gaming"]);
}

#[test]
fn a_listen_carries_what_was_set_when_it_began() {
    let store = store();
    let event = store
        .begin_play(
            "t1",
            "k",
            PlaySource::Unknown,
            Some(Activity::Gaming),
            now_millis(),
        )
        .unwrap();
    assert_eq!(event.activity.as_deref(), Some("gaming"));
    store
        .finish_play(&event, 200_000, Some(200_000), now_millis(), None)
        .unwrap();
    let stored = store
        .read(|conn| PlayEvent::fetch(conn, &event.id))
        .unwrap()
        .unwrap();
    assert_eq!(stored.activity.as_deref(), Some("gaming"));
}

#[test]
fn an_untagged_listen_stays_untagged_rather_than_being_guessed_at() {
    let store = store();
    let event = store
        .begin_play("t1", "k", PlaySource::Unknown, None, now_millis())
        .unwrap();
    assert_eq!(event.activity, None);
}

#[test]
fn finish_counts_split_by_activity_ignoring_untagged_listens() {
    let store = store();
    for (activity, completed) in [
        (Activity::Gaming, true),
        (Activity::Gaming, true),
        (Activity::Gaming, false),
        (Activity::Focus, false),
    ] {
        let event = store
            .begin_play("t1", "k", PlaySource::Unknown, Some(activity), now_millis())
            .unwrap();
        store
            .finish_play(
                &event,
                if completed { 200_000 } else { 3_000 },
                Some(200_000),
                now_millis(),
                None,
            )
            .unwrap();
    }
    let untagged = store
        .begin_play("t1", "k", PlaySource::Unknown, None, now_millis())
        .unwrap();
    store
        .finish_play(&untagged, 200_000, Some(200_000), now_millis(), None)
        .unwrap();

    let split = store.finish_counts_by_activity(&["t1".to_owned()]).unwrap();
    assert_eq!(split[&Activity::Gaming], FinishCounts::new(3, 2));
    assert_eq!(split[&Activity::Focus], FinishCounts::new(1, 0));
    assert_eq!(split.len(), 2, "the untagged listen belongs to no activity");
}
