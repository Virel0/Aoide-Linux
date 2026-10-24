//! Ports of `NextChooserTests.swift` ("Asking the server what plays next") and
//! `DiscoverTests.swift` ("Discover, as the client reads it").

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::{DiscoverResponse, NextFactors, NextMode, NextRequest, NextResponse, iso_week};

fn owned(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

fn factors(
    taste: f64,
    kinship: Option<f64>,
    freshness: f64,
    similarity: f64,
    mixability: Option<f64>,
    arc: f64,
) -> NextFactors {
    NextFactors {
        taste,
        kinship,
        freshness,
        similarity,
        mixability,
        arc,
    }
}

#[test]
fn the_request_is_the_contracts_shape_with_the_mode_spelled_as_the_server_expects() {
    let request = NextRequest::new(
        "s",
        &owned(&["q1", "q2"]),
        &owned(&["r1"]),
        NextMode::Autodj,
        20,
    );
    let json: Value = serde_json::to_value(&request).unwrap();
    assert_eq!(json["seed"], "s");
    assert_eq!(json["queue"], serde_json::json!(["q1", "q2"]));
    assert_eq!(json["recent"], serde_json::json!(["r1"]));
    assert_eq!(json["mode"], "autodj");
    assert_eq!(json["limit"], 20);
    assert_eq!(
        serde_json::to_value(NextMode::Infinity).unwrap(),
        "infinity"
    );
}

#[test]
fn a_long_queue_and_a_long_history_are_cut_from_the_far_end_not_refused() {
    let queue: Vec<String> = (0..1200).map(|i| format!("q{i}")).collect();
    let recent: Vec<String> = (0..60).map(|i| format!("r{i}")).collect();
    let request = NextRequest::new("s", &queue, &recent, NextMode::Infinity, 20);
    assert_eq!(request.queue.len(), 1000);
    assert_eq!(request.queue[0], "q0");
    assert_eq!(request.queue.last().unwrap(), "q999");
    assert_eq!(request.recent.len(), 40);
    assert_eq!(request.recent[0], "r0");
}

#[test]
fn the_answer_decodes_with_mixability_absent_outside_auto_dj() {
    let body = r#"{ "candidates": [
        { "id": "a", "score": 0.83,
          "factors": { "taste": 0.71, "freshness": 1.0, "similarity": 0.62, "mixability": 0.79, "arc": 0.9 } },
        { "id": "b", "score": 0.4,
          "factors": { "taste": 0.2, "freshness": 0.5, "similarity": 0.5, "mixability": null, "arc": 0.5 } }
      ],
      "profile": { "events": 184, "since": 1749600000000 } }"#;
    let response: NextResponse = serde_json::from_str(body).unwrap();
    assert_eq!(
        response
            .candidates
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(response.candidates[0].factors.mixability, Some(0.79));
    assert_eq!(response.candidates[1].factors.mixability, None);
    assert_eq!(response.profile.events, 184);
    assert_eq!(response.profile.since, 1_749_600_000_000);
}

#[test]
fn the_summary_reads_as_percentages_names_a_mix_only_when_there_is_one_and_says_what_freshness_cost()
 {
    assert_eq!(
        factors(0.714, None, 1.0, 0.62, Some(0.79), 0.9).summary(),
        "Taste 71% · Fits 62% · Mixes 79% · Arc 90%"
    );
    assert_eq!(
        factors(0.2, None, 1.0, 0.5, None, 0.5).summary(),
        "Taste 20% · Fits 50% · Arc 50%"
    );
    assert!(
        factors(0.2, None, 0.5, 0.5, None, 0.5)
            .summary()
            .ends_with("· same artist lately")
    );
    assert!(
        factors(0.2, None, 0.0, 0.5, None, 0.5)
            .summary()
            .ends_with("· heard lately")
    );
    assert_eq!(
        factors(0.2, Some(1.0), 1.0, 0.5, Some(0.15), 0.5).summary(),
        "Taste 20% · Kin 100% · Fits 50% · Crossfade · Arc 50%"
    );
    assert_eq!(
        factors(0.2, Some(0.5), 1.0, 0.5, Some(0.16), 0.5).summary(),
        "Taste 20% · Kin 50% · Fits 50% · Mixes 16% · Arc 50%"
    );
}

#[test]
fn a_next_request_holds_its_limit_to_the_contracts_range() {
    // Not in the Swift suite, which leaves the limit to the caller.
    assert_eq!(
        NextRequest::new("s", &[], &[], NextMode::Infinity, 0).limit,
        1
    );
    assert_eq!(
        NextRequest::new("s", &[], &[], NextMode::Infinity, 500).limit,
        200
    );
}

// MARK: - Discover

/// The seed-only factors arrive as nulls and stay nulls.
#[test]
fn the_answer_decodes_with_the_seed_only_factors_sent_as_null() {
    let body = r#"{ "week": "2026-W43",
      "candidates": [
        { "id": "a", "score": 0.75,
          "factors": { "taste": 0.75, "kinship": null, "freshness": 1, "similarity": null, "mixability": null, "arc": null } }
      ],
      "profile": { "events": 184, "since": 1784592000000 } }"#;
    let response: DiscoverResponse = serde_json::from_str(body).unwrap();
    assert_eq!(response.week, "2026-W43");
    assert_eq!(
        response
            .candidates
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        ["a"]
    );
    let factors = response.candidates[0].factors;
    assert_eq!(factors.taste, 0.75);
    assert_eq!(factors.freshness, Some(1.0));
    assert_eq!(
        (
            factors.kinship,
            factors.similarity,
            factors.mixability,
            factors.arc
        ),
        (None, None, None, None)
    );
    assert_eq!(response.profile.events, 184);
}

fn utc(text: &str) -> i64 {
    DateTime::parse_from_rfc3339(text)
        .unwrap()
        .with_timezone(&Utc)
        .timestamp_millis()
}

#[test]
fn the_week_is_iso_and_utc_and_turns_at_monday_midnight_utc() {
    assert_eq!(iso_week(utc("2026-10-25T23:59:59Z")), "2026-W43");
    assert_eq!(iso_week(utc("2026-10-26T00:00:00Z")), "2026-W44");
    // 2027-01-01 is a Friday in ISO week 53 of 2026.
    assert_eq!(iso_week(utc("2027-01-01T12:00:00Z")), "2026-W53");
    assert_eq!(iso_week(utc("2027-01-04T00:00:00Z")), "2027-W01");
}

#[test]
fn every_week_name_in_the_sidecars_fixture_table_the_same_here() {
    // Copied from the sidecar's discover-fixture.json, "weeks", via the phone's suite.
    let rows: [(i64, &str); 7] = [
        (1792368000000, "2026-W43"),
        (1792367999999, "2026-W42"),
        (1767225600000, "2026-W01"),
        (1798718400000, "2026-W53"),
        (1799020799999, "2026-W53"),
        (1799020800000, "2027-W01"),
        (1735516800000, "2025-W01"),
    ];
    for (millis, week) in rows {
        assert_eq!(iso_week(millis), week, "{millis}");
    }
}
