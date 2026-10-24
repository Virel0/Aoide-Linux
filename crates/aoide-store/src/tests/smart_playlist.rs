//! Port of `SmartPlaylistTests.swift` ("Smart playlists").

use chrono::{TimeZone, Utc};

use super::{pending, store};
use crate::{
    Calendar, Field, FractionalIndex, Like, Match, Millis, Operator, PlayEvent, PlaySource,
    Playlist, Record, RelativeInterval, SmartPlaylistError, SmartRule, SmartRules, Sort,
    SortDirection, Store, StoreError, TimeUnit, TrackRef, Value, now_millis,
};

const NOW: Millis = 1_700_000_000_000;
const DAY: Millis = 86_400_000;

type Clause = (Field, Operator, Value);

fn track(
    id: &str,
    title: &str,
    artist: &str,
    album: &str,
    album_artist: Option<&str>,
    (duration, year): (i64, i64),
    genres: &[&str],
) -> TrackRef {
    let mut row = TrackRef::new(id, id, title, artist, album, NOW).with_genres(genres);
    row.album_artist = album_artist.map(str::to_owned);
    row.duration_ms = Some(duration);
    row.year = Some(year);
    row
}

fn seed(store: &Store, track: TrackRef) {
    store.write_local(|tx| track.insert(tx)).unwrap();
}

fn play(track: &str, started: Millis, played: i64, completed: bool) -> PlayEvent {
    PlayEvent::new(track, track, started, "phone")
        .with_source(PlaySource::Album)
        .finished(Some(started + played), played, completed, false)
}

/// Four tracks: played often and recently, played once long ago and skipped since,
/// skipped only, and never touched.
fn library() -> Store {
    let store = store();
    seed(
        &store,
        track(
            "so-what",
            "So What",
            "Miles Davis",
            "Kind of Blue",
            Some("Miles Davis"),
            (545_000, 1959),
            &["Jazz", "Modal"],
        ),
    );
    seed(
        &store,
        track(
            "paranoid",
            "Paranoid Android",
            "Radiohead",
            "OK Computer",
            Some("Radiohead"),
            (383_000, 1997),
            &["Alternative Rock"],
        ),
    );
    // No album artist: negative text rules have to cope with NULL.
    seed(
        &store,
        track(
            "autumn",
            "Autumn Leaves",
            "Bill Evans",
            "Portrait in Jazz",
            None,
            (350_000, 1960),
            &["Jazz", "Bebop"],
        ),
    );
    seed(
        &store,
        track(
            "xtal",
            "Xtal",
            "Aphex Twin",
            "Selected Ambient Works",
            Some("Aphex Twin"),
            (293_000, 1992),
            &["Electronic"],
        ),
    );

    for offset in [1, 40, 400] {
        store
            .append(play("so-what", NOW - offset * DAY, 545_000, true))
            .unwrap();
    }
    store
        .append(play("paranoid", NOW - 200 * DAY, 200_000, false))
        .unwrap();
    store
        .append(play("paranoid", NOW - 3 * DAY, 10_000, false))
        .unwrap();
    store
        .append(play("paranoid", NOW - 2 * DAY, 10_000, false))
        .unwrap();
    store
        .append(play("autumn", NOW - 5 * DAY, 20_000, false))
        .unwrap();

    store
        .record(
            Like::new("so-what", "k1", true, NOW, ""),
            false,
            now_millis(),
        )
        .unwrap();
    store
        .record(
            Like::new("paranoid", "k2", false, NOW, ""),
            false,
            now_millis(),
        )
        .unwrap();
    // Soft-deleted, so it reads as unliked rather than liked.
    store
        .record(Like::new("autumn", "k3", true, NOW, ""), true, now_millis())
        .unwrap();
    store
}

fn rules(
    match_mode: Match,
    clauses: Vec<Clause>,
    sort: Option<Sort>,
    limit: Option<i64>,
) -> SmartRules {
    let built = clauses
        .into_iter()
        .map(|(f, o, v)| SmartRule::new(f, o, v).unwrap())
        .collect();
    SmartRules::new(match_mode, built, sort, limit).unwrap()
}

fn matches_at(store: &Store, match_mode: Match, clauses: Vec<Clause>, at: Millis) -> Vec<String> {
    store
        .evaluate(&rules(match_mode, clauses, None, None), at, Calendar::Utc)
        .unwrap()
}

fn matches(store: &Store, match_mode: Match, clauses: Vec<Clause>) -> Vec<String> {
    matches_at(store, match_mode, clauses, NOW)
}

fn one(field: Field, op: Operator, value: impl Into<Value>) -> Vec<Clause> {
    vec![(field, op, value.into())]
}

// MARK: - Text operators

#[test]
fn text_operators_compare_case_insensitively() {
    let store = library();
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::Artist, Operator::Is, "miles davis")
        ),
        ["so-what"]
    );
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::Artist, Operator::IsNot, "Miles Davis")
        ),
        ["autumn", "paranoid", "xtal"]
    );
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::Title, Operator::Contains, "an")
        ),
        ["paranoid"]
    );
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::Album, Operator::BeginsWith, "kind")
        ),
        ["so-what"]
    );
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::Album, Operator::DoesNotContain, "jazz")
        ),
        ["paranoid", "so-what", "xtal"]
    );
}

#[test]
fn a_missing_album_artist_still_satisfies_is_not() {
    let store = library();
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::AlbumArtist, Operator::IsNot, "Radiohead")
        ),
        ["autumn", "so-what", "xtal"]
    );
}

/// Negative genre operators negate the whole existence test.
#[test]
fn genre_rules_ask_about_any_genre_and_is_not_about_none() {
    let store = library();
    assert_eq!(
        matches(&store, Match::All, one(Field::Genre, Operator::Is, "jazz")),
        ["autumn", "so-what"]
    );
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::Genre, Operator::IsNot, "Jazz")
        ),
        ["paranoid", "xtal"]
    );
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::Genre, Operator::Contains, "rock")
        ),
        ["paranoid"]
    );
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::Genre, Operator::BeginsWith, "Elect")
        ),
        ["xtal"]
    );
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::Genre, Operator::DoesNotContain, "e")
        ),
        ["so-what"]
    );
}

// MARK: - Numeric operators

#[test]
fn numeric_operators_including_on_a_column_that_may_be_unknown() {
    let store = library();
    assert_eq!(
        matches(&store, Match::All, one(Field::Year, Operator::Is, 1960)),
        ["autumn"]
    );
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::Year, Operator::GreaterThan, 1990)
        ),
        ["paranoid", "xtal"]
    );
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::Year, Operator::LessThan, 1960)
        ),
        ["so-what"]
    );
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::DurationMs, Operator::GreaterThan, 400_000)
        ),
        ["so-what"]
    );
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::SkipCount, Operator::GreaterThan, 1)
        ),
        ["paranoid"]
    );
}

/// A track nobody has played has a play count of zero, not "no row".
#[test]
fn a_never_played_track_has_played_fewer_than_three_times() {
    let store = library();
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::PlayCount, Operator::LessThan, 3)
        ),
        ["autumn", "paranoid", "xtal"]
    );
    assert_eq!(
        matches(&store, Match::All, one(Field::PlayCount, Operator::Is, 0)),
        ["autumn", "xtal"]
    );
}

// MARK: - Dates

/// `autumn` was skipped five days ago: a skip does not move the last-played date.
#[test]
fn in_the_last_and_not_in_the_last_resolved_against_the_injected_clock() {
    let store = library();
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::LastPlayed, Operator::InTheLast, "30d")
        ),
        ["so-what"]
    );
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::LastPlayed, Operator::NotInTheLast, "30d")
        ),
        ["autumn", "paranoid", "xtal"]
    );
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::FirstPlayed, Operator::Before, "-1y")
        ),
        ["so-what"]
    );
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::LastPlayed, Operator::After, "-30d")
        ),
        ["so-what"]
    );
}

#[test]
fn not_in_the_last_includes_never_played_before_does_not() {
    let store = library();
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::LastPlayed, Operator::Before, "-30d")
        ),
        ["paranoid"]
    );
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::LastPlayed, Operator::NotInTheLast, "-30d")
        ),
        ["autumn", "paranoid", "xtal"]
    );
}

#[test]
fn a_relative_rule_moves_with_the_clock_it_is_evaluated_against() {
    let store = library();
    let later = NOW + 40 * DAY;
    assert!(
        matches_at(
            &store,
            Match::All,
            one(Field::LastPlayed, Operator::InTheLast, "30d"),
            later
        )
        .is_empty()
    );
    assert_eq!(
        matches_at(
            &store,
            Match::All,
            one(Field::LastPlayed, Operator::InTheLast, "1y"),
            later
        ),
        ["paranoid", "so-what"]
    );
}

#[test]
fn an_absolute_epoch_millisecond_is_accepted_where_an_instant_is_wanted() {
    let store = library();
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::LastPlayed, Operator::After, NOW - 100 * DAY)
        ),
        ["so-what"]
    );
}

// MARK: - Likes

#[test]
fn liked_reads_through_the_like_table_and_a_soft_deleted_like_is_not_a_like() {
    let store = library();
    assert_eq!(
        matches(&store, Match::All, one(Field::Liked, Operator::Is, true)),
        ["so-what"]
    );
    assert_eq!(
        matches(&store, Match::All, one(Field::Liked, Operator::Is, false)),
        ["autumn", "paranoid", "xtal"]
    );
    assert_eq!(
        matches(&store, Match::All, one(Field::Liked, Operator::IsNot, true)),
        ["autumn", "paranoid", "xtal"]
    );
}

// MARK: - Combination, sort, limit

#[test]
fn all_narrows_and_any_widens() {
    let store = library();
    let clauses = || {
        vec![
            (Field::Genre, Operator::Is, Value::from("Jazz")),
            (Field::Year, Operator::GreaterThan, Value::from(1990)),
        ]
    };
    assert!(matches(&store, Match::All, clauses()).is_empty());
    assert_eq!(
        matches(&store, Match::Any, clauses()),
        ["autumn", "paranoid", "so-what", "xtal"]
    );
}

#[test]
fn sort_and_limit_are_applied_by_the_database() {
    let store = library();
    let top = rules(
        Match::All,
        vec![],
        Some(Sort::new(Field::PlayCount, SortDirection::Desc)),
        Some(2),
    );
    assert_eq!(
        store.evaluate(&top, NOW, Calendar::Utc).unwrap(),
        ["so-what", "paranoid"]
    );
    let by_title = rules(
        Match::All,
        vec![],
        Some(Sort::new(Field::Title, SortDirection::Asc)),
        None,
    );
    assert_eq!(
        store.evaluate(&by_title, NOW, Calendar::Utc).unwrap(),
        ["autumn", "paranoid", "so-what", "xtal"]
    );
    // Never played sorts last descending, ties broken on id.
    let recent = rules(
        Match::All,
        vec![],
        Some(Sort::new(Field::LastPlayed, SortDirection::Desc)),
        None,
    );
    assert_eq!(
        store.evaluate(&recent, NOW, Calendar::Utc).unwrap(),
        ["so-what", "paranoid", "autumn", "xtal"]
    );
}

#[test]
fn no_rules_at_all_means_every_track() {
    let store = library();
    assert_eq!(
        matches(&store, Match::All, vec![]),
        ["autumn", "paranoid", "so-what", "xtal"]
    );
}

// MARK: - Injection

#[test]
fn a_rule_value_that_is_sql_is_treated_as_text() {
    let store = store();
    let payload = "'; DROP TABLE playlist; --";
    let mut bobby = TrackRef::new("bobby", "kb", payload, payload, "Exploits of a Mom", NOW);
    bobby.duration_ms = Some(1000);
    seed(&store, bobby);
    let mut innocent = TrackRef::new("innocent", "ki", "Innocent", "Somebody", "Elsewhere", NOW);
    innocent.duration_ms = Some(1000);
    seed(&store, innocent);
    store
        .record(
            Playlist::new("Survivor", FractionalIndex::initial(), NOW, ""),
            false,
            now_millis(),
        )
        .unwrap();

    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::Artist, Operator::Is, payload)
        ),
        ["bobby"]
    );
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::Title, Operator::Contains, "DROP TABLE")
        ),
        ["bobby"]
    );
    assert!(matches(&store, Match::All, one(Field::Genre, Operator::Is, payload)).is_empty());
    assert!(
        matches(
            &store,
            Match::All,
            one(Field::Album, Operator::BeginsWith, "' OR 1=1 --")
        )
        .is_empty()
    );
    let survivors = store.playlists().unwrap();
    assert_eq!(
        survivors
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["Survivor"]
    );
}

#[test]
fn wildcards_inside_a_rule_value_are_literal() {
    let store = store();
    seed(
        &store,
        TrackRef::new("endurance", "ke", "100% Endurance", "A", "A", NOW),
    );
    seed(
        &store,
        TrackRef::new("years", "ky", "100 Years", "B", "B", NOW),
    );
    assert_eq!(
        matches(
            &store,
            Match::All,
            one(Field::Title, Operator::Contains, "100%")
        ),
        ["endurance"]
    );
    assert!(
        matches(
            &store,
            Match::All,
            one(Field::Title, Operator::Contains, "_")
        )
        .is_empty()
    );
}

// MARK: - What counts as a play

#[test]
fn the_play_and_skip_thresholds() {
    let store = store();
    for (id, duration) in [
        ("short", Some(180_000)),
        ("long", Some(1_200_000)),
        ("unknown", None),
    ] {
        let mut row = TrackRef::new(id, id, id, "A", "A", NOW);
        row.duration_ms = duration;
        seed(&store, row);
    }
    store
        .append(play("short", NOW - DAY, 90_000, false))
        .unwrap(); // exactly half: a play
    store
        .append(play("short", NOW - 2 * DAY, 60_000, false))
        .unwrap(); // neither
    store
        .append(play("short", NOW - 3 * DAY, 30_000, false))
        .unwrap(); // under a fifth: a skip
    store
        .append(play("long", NOW - DAY, 240_000, false))
        .unwrap(); // four minutes of twenty
    store
        .append(play("unknown", NOW - DAY, 10_000, false))
        .unwrap(); // under 48s: a skip
    store
        .append(play("unknown", NOW - 2 * DAY, 300_000, false))
        .unwrap(); // four minutes alone

    assert_eq!(
        matches(&store, Match::All, one(Field::PlayCount, Operator::Is, 1)),
        ["long", "short", "unknown"]
    );
    assert_eq!(
        matches(&store, Match::All, one(Field::SkipCount, Operator::Is, 1)),
        ["short", "unknown"]
    );
    assert_eq!(
        matches(&store, Match::All, one(Field::SkipCount, Operator::Is, 0)),
        ["long"]
    );
}

// MARK: - The stored JSON

#[test]
fn the_documents_own_example_decodes_and_evaluates() {
    let json = r#"{
      "match": "all",
      "rules": [
        { "field": "genre",       "op": "is",     "value": "Jazz" },
        { "field": "play_count",  "op": ">=",     "value": 3 },
        { "field": "last_played", "op": "before", "value": "-30d" }
      ],
      "sort":  { "field": "last_played", "dir": "desc" },
      "limit": 100
    }"#;
    let decoded = SmartRules::from_json(json).unwrap();
    assert_eq!(decoded.rules.len(), 3);
    // `>= 3` is exactly `> 2` on an integer.
    assert_eq!(decoded.rules[1].op, Operator::GreaterThan);
    assert_eq!(decoded.rules[1].value, Value::Number(2));

    let store = library();
    assert!(
        store
            .evaluate(&decoded, NOW, Calendar::Utc)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .evaluate(&decoded, NOW + 60 * DAY, Calendar::Utc)
            .unwrap(),
        ["so-what"]
    );
}

#[test]
fn rules_round_trip_through_the_documents_field_spelling() {
    let original = rules(
        Match::Any,
        vec![
            (Field::PlayCount, Operator::GreaterThan, Value::from(5)),
            (
                Field::LastPlayed,
                Operator::NotInTheLast,
                Value::from("-6m"),
            ),
        ],
        Some(Sort::new(Field::LastPlayed, SortDirection::Desc)),
        Some(25),
    );
    let json = original.to_json();
    assert!(json.contains("\"play_count\""));
    assert!(json.contains("\"last_played\""));
    assert_eq!(SmartRules::from_json(&json).unwrap(), original);
}

#[test]
fn camel_case_field_names_are_accepted_on_the_way_in() {
    let json = r#"{ "match": "all", "rules": [{ "field": "playCount", "op": "greaterThan", "value": 1 }] }"#;
    assert_eq!(
        SmartRules::from_json(json).unwrap().rules[0].field,
        Field::PlayCount
    );
}

// MARK: - Rejecting the impossible

#[test]
fn illegal_rules_are_refused_rather_than_quietly_matching_nothing() {
    assert_eq!(
        SmartRule::new(Field::PlayCount, Operator::Contains, "3"),
        Err(SmartPlaylistError::OperatorNotApplicable {
            field: Field::PlayCount,
            op: Operator::Contains
        })
    );
    assert_eq!(
        SmartRule::new(Field::Title, Operator::InTheLast, "30d"),
        Err(SmartPlaylistError::OperatorNotApplicable {
            field: Field::Title,
            op: Operator::InTheLast
        })
    );
    assert_eq!(
        SmartRule::new(Field::Title, Operator::Is, 3),
        Err(SmartPlaylistError::ValueTypeMismatch {
            field: Field::Title,
            op: Operator::Is
        })
    );
    assert_eq!(
        SmartRule::new(Field::Liked, Operator::Is, "yes"),
        Err(SmartPlaylistError::ValueTypeMismatch {
            field: Field::Liked,
            op: Operator::Is
        })
    );
    // A span in unstated units is not a span.
    assert_eq!(
        SmartRule::new(Field::LastPlayed, Operator::InTheLast, 30),
        Err(SmartPlaylistError::ValueTypeMismatch {
            field: Field::LastPlayed,
            op: Operator::InTheLast
        })
    );
    assert_eq!(
        SmartRule::new(Field::LastPlayed, Operator::InTheLast, "a fortnight"),
        Err(SmartPlaylistError::MalformedInterval("a fortnight".into()))
    );
    assert_eq!(
        SmartRules::new(Match::Any, vec![], None, None),
        Err(SmartPlaylistError::NoRules)
    );
    assert_eq!(
        SmartRules::new(Match::All, vec![], None, Some(0)),
        Err(SmartPlaylistError::NonPositiveLimit(0))
    );

    let by_genre = SmartRules::new(
        Match::All,
        vec![],
        Some(Sort::new(Field::Genre, SortDirection::Asc)),
        None,
    )
    .unwrap();
    assert!(matches!(
        store().evaluate(&by_genre, NOW, Calendar::Utc),
        Err(StoreError::Smart(SmartPlaylistError::FieldNotSortable(
            Field::Genre
        )))
    ));
}

#[test]
fn unknown_fields_and_operators_are_named_not_ignored() {
    assert_eq!(
        SmartRules::from_json(
            r#"{"match":"all","rules":[{"field":"titel","op":"is","value":"x"}]}"#
        ),
        Err(SmartPlaylistError::UnknownField("titel".into()))
    );
    assert_eq!(
        SmartRules::from_json(
            r#"{"match":"all","rules":[{"field":"title","op":"soundsLike","value":"x"}]}"#
        ),
        Err(SmartPlaylistError::UnknownOperator("soundsLike".into()))
    );
    assert_eq!(
        SmartRules::from_json(
            r#"{"match":"all","rules":[{"field":"title FROM track; DROP TABLE track; --","op":"is","value":"x"}]}"#
        ),
        Err(SmartPlaylistError::UnknownField(
            "title FROM track; DROP TABLE track; --".into()
        ))
    );
    assert_eq!(
        SmartRules::from_json(
            r#"{"match":"all","rules":[{"field":"year","op":"is","value":1959.5}]}"#
        ),
        Err(SmartPlaylistError::MalformedValue)
    );
}

// MARK: - Relative intervals

#[test]
fn months_and_years_are_calendar_arithmetic_not_multiples_of_a_day() {
    let instant = |y: i32, m: u32, d: u32| {
        Utc.with_ymd_and_hms(y, m, d, 12, 0, 0)
            .unwrap()
            .timestamp_millis()
    };
    let end_of_march = instant(2024, 3, 31);
    let ago = |written: &str| {
        RelativeInterval::parse(written)
            .unwrap()
            .ago(end_of_march, Calendar::Utc)
    };
    assert_eq!(ago("-1m"), instant(2024, 2, 29));
    assert_eq!(ago("-1y"), instant(2023, 3, 31));
    assert_eq!(ago("-30d"), end_of_march - 30 * DAY);
    assert_eq!(ago("-2w"), end_of_march - 14 * DAY);
}

#[test]
fn intervals_parse_with_or_without_the_leading_minus_and_refuse_anything_else() {
    assert_eq!(
        RelativeInterval::parse("-6m"),
        Ok(RelativeInterval {
            amount: 6,
            unit: TimeUnit::Months
        })
    );
    assert_eq!(
        RelativeInterval::parse("6m"),
        Ok(RelativeInterval {
            amount: 6,
            unit: TimeUnit::Months
        })
    );
    assert_eq!(
        RelativeInterval::parse(" -1Y "),
        Ok(RelativeInterval {
            amount: 1,
            unit: TimeUnit::Years
        })
    );
    for bad in ["banana", "30", "d", "3.5d", "--3d", "30 days", ""] {
        assert_eq!(
            RelativeInterval::parse(bad),
            Err(SmartPlaylistError::MalformedInterval(bad.into())),
            "{bad}"
        );
    }
}

// MARK: - Playlists

#[test]
fn rules_attached_to_a_playlist_come_back_out_of_it() {
    let store = library();
    let set = rules(
        Match::All,
        one(Field::Genre, Operator::Is, "Jazz"),
        Some(Sort::new(Field::Title, SortDirection::Asc)),
        None,
    );
    let playlist =
        Playlist::new("Jazz", FractionalIndex::initial(), NOW, "").setting_smart_rules(&set);
    let saved = store.record(playlist, false, now_millis()).unwrap();
    assert!(saved.is_smart);
    assert_eq!(saved.smart_rule_set().unwrap(), Some(set));
    assert_eq!(
        store.resolve_smart(&saved, NOW, Calendar::Utc).unwrap(),
        ["autumn", "so-what"]
    );
    assert!(
        pending(&store)
            .iter()
            .any(|op| op.entity_id == saved.id && op.payload.contains("smartRules"))
    );
}

#[test]
fn resolving_a_playlist_that_carries_no_rules_is_an_error_not_an_empty_list() {
    let store = library();
    let plain = Playlist::new("Hand-made", FractionalIndex::initial(), NOW, "");
    assert!(matches!(
        store.resolve_smart(&plain, NOW, Calendar::Utc),
        Err(StoreError::Smart(SmartPlaylistError::NotSmart { playlist_id })) if playlist_id == plain.id
    ));
    assert_eq!(plain.smart_rule_set().unwrap(), None);
}

// MARK: - Counting without listing

#[test]
fn the_count_agrees_with_the_list_it_is_not_building() {
    let store = library();
    let sets = [
        rules(Match::All, vec![], None, None),
        rules(
            Match::All,
            one(Field::Genre, Operator::Is, "Jazz"),
            None,
            None,
        ),
        rules(
            Match::Any,
            vec![
                (Field::PlayCount, Operator::GreaterThan, Value::from(1)),
                (Field::Liked, Operator::Is, Value::from(true)),
            ],
            None,
            None,
        ),
        rules(
            Match::All,
            one(Field::PlayCount, Operator::Is, 0),
            None,
            None,
        ),
        rules(
            Match::All,
            one(Field::Artist, Operator::Is, "Nobody At All"),
            None,
            None,
        ),
        rules(
            Match::All,
            vec![],
            Some(Sort::new(Field::PlayCount, SortDirection::Desc)),
            Some(2),
        ),
        rules(
            Match::All,
            one(Field::Genre, Operator::Is, "Jazz"),
            Some(Sort::new(Field::Title, SortDirection::Asc)),
            Some(1),
        ),
        // A limit larger than the result set must not be reported as the limit.
        rules(
            Match::All,
            one(Field::Genre, Operator::Is, "Electronic"),
            None,
            Some(50),
        ),
    ];
    for set in sets {
        let listed = store.evaluate(&set, NOW, Calendar::Utc).unwrap().len() as i64;
        assert_eq!(store.match_count(&set, NOW, Calendar::Utc).unwrap(), listed);
    }
}

#[test]
fn counting_a_playlist_that_carries_no_rules_is_an_error_like_resolving_one() {
    let store = library();
    let plain = Playlist::new("Hand-made", FractionalIndex::initial(), NOW, "");
    assert!(matches!(
        store.match_count_of(&plain, NOW, Calendar::Utc),
        Err(StoreError::Smart(SmartPlaylistError::NotSmart { .. }))
    ));
}
