//! The HTTP client against a local mock server. Includes the applicable cases of
//! JellyfinKit's `SyncEndpointTests.swift` ("Sidecar requests").

use std::collections::BTreeMap;

use aoide_store::{PullCursor, PushRequest, WireOp};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use crate::{
    Measurement, NextMode, NextRequest, SidecarClient, SidecarError, SoundBounds, SyncTransport,
};

const TOKEN: &str = "tok-123";

async fn mock() -> (MockServer, SidecarClient) {
    let server = MockServer::start().await;
    let client = SidecarClient::new(&server.uri(), TOKEN).unwrap();
    (server, client)
}

async fn requests(server: &MockServer) -> Vec<Request> {
    server.received_requests().await.unwrap_or_default()
}

fn respond(
    server: &MockServer,
    verb: &str,
    route: &str,
    template: ResponseTemplate,
) -> impl Future<Output = ()> {
    Mock::given(method(verb))
        .and(path(route))
        .respond_with(template)
        .mount(server)
}

fn header_value(request: &Request, name: &str) -> Option<String> {
    request
        .headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

// MARK: - Addresses and credentials

#[test]
fn the_sidecar_hangs_off_the_server_root() {
    let client = SidecarClient::new("https://media.example.com", TOKEN).unwrap();
    assert_eq!(
        client.url("aoide/sync/push", &[]).as_str(),
        "https://media.example.com/aoide/sync/push"
    );
}

/// A Jellyfin behind a reverse proxy lives at a subpath, and the sidecar under it.
#[test]
fn a_reverse_proxy_subpath_is_preserved() {
    for base in [
        "https://example.com/jellyfin",
        "https://example.com/jellyfin/",
    ] {
        let client = SidecarClient::new(base, TOKEN).unwrap();
        assert_eq!(
            client.url("aoide/sync/pull", &[]).as_str(),
            "https://example.com/jellyfin/aoide/sync/pull"
        );
    }
}

#[test]
fn an_address_that_is_not_http_is_refused() {
    assert!(matches!(
        SidecarClient::new("not a url", TOKEN),
        Err(SidecarError::InvalidUrl(_))
    ));
    assert!(matches!(
        SidecarClient::new("ftp://example.com", TOKEN),
        Err(SidecarError::InvalidUrl(_))
    ));
}

#[test]
fn the_token_stays_out_of_debug_output() {
    let client = SidecarClient::new("https://example.com", TOKEN).unwrap();
    assert!(!format!("{client:?}").contains(TOKEN));
}

#[tokio::test]
async fn the_sidecar_gets_the_same_mediabrowser_token_as_everything_else() {
    let (server, client) = mock().await;
    respond(
        &server,
        "GET",
        "/aoide/sync/status",
        ResponseTemplate::new(200).set_body_json(json!({})),
    )
    .await;
    client.status().await.unwrap();
    let request = &requests(&server).await[0];
    let expected = format!("MediaBrowser Token=\"{TOKEN}\"");
    assert_eq!(
        header_value(request, "authorization").as_deref(),
        Some(expected.as_str())
    );
    // 10.8 and earlier read the other one.
    assert_eq!(
        header_value(request, "x-emby-authorization").as_deref(),
        Some(expected.as_str())
    );
}

// MARK: - Pull

/// Not omitted on a new device: a server binding `since` as required would fail the
/// first request a new device makes.
#[tokio::test]
async fn a_device_that_has_never_synced_still_names_a_position() {
    let (server, client) = mock().await;
    Mock::given(method("GET"))
        .and(path("/aoide/sync/pull"))
        .and(query_param("since", "0"))
        .and(query_param("limit", "500"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ops": [], "cursor": 0, "hasMore": false})),
        )
        .expect(1)
        .mount(&server)
        .await;
    client.pull(None, 500).await.unwrap();
}

#[tokio::test]
async fn a_stored_cursor_is_sent_as_itself() {
    let (server, client) = mock().await;
    Mock::given(method("GET"))
        .and(path("/aoide/sync/pull"))
        .and(query_param("since", "12345"))
        .and(query_param("limit", "250"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ops": [], "cursor": 12345, "hasMore": false})),
        )
        .expect(1)
        .mount(&server)
        .await;
    client.pull(Some(PullCursor(12345)), 250).await.unwrap();
}

#[tokio::test]
async fn a_pulled_page_keeps_every_payload_key_exactly_as_it_was_written() {
    let (server, client) = mock().await;
    let body = json!({"ops": [
        {"opId": "o1", "entity": "playlists", "entityId": "l1", "operation": "upsert",
         "payload": {"id": "l1", "Notes": "capitalised elsewhere", "name": "x"},
         "createdAt": 1, "seq": 1, "deviceId": "mac", "receivedAt": 2, "authorUserId": "u1"},
        {"opId": "o2", "seq": 2}
    ], "cursor": 2, "hasMore": false});
    respond(
        &server,
        "GET",
        "/aoide/sync/pull",
        ResponseTemplate::new(200).set_body_json(body),
    )
    .await;
    let page = client.pull(None, 500).await.unwrap();
    assert_eq!(page.ops.len(), 1);
    assert_eq!(page.unreadable_ops, 1);
    assert_eq!(page.ops[0].payload["Notes"], "capitalised elsewhere");
    assert!(page.ops[0].payload.get("notes").is_none());
    assert_eq!(page.ops[0].author_user_id.as_deref(), Some("u1"));
}

// MARK: - Push

#[tokio::test]
async fn a_push_sends_the_contracts_envelope_and_reads_the_answer() {
    let (server, client) = mock().await;
    let payload = json!({"id": "l1", "Notes": "kept as written", "deleted": false});
    let request = PushRequest {
        device_id: "phone".into(),
        ops: vec![WireOp {
            op_id: "op-1".into(),
            entity: "playlists".into(),
            entity_id: "l1".into(),
            operation: "upsert".into(),
            payload: payload.clone(),
            created_at: 1_700_000_000_000,
        }],
    };
    Mock::given(method("POST"))
        .and(path("/aoide/sync/push"))
        .and(header("content-type", "application/json"))
        .and(body_json(json!({"deviceId": "phone", "ops": [{
            "opId": "op-1", "entity": "playlists", "entityId": "l1", "operation": "upsert",
            "payload": payload, "createdAt": 1_700_000_000_000_i64
        }]})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "accepted": ["op-1"], "rejected": [{"opId": "op-2", "reason": "bad entity"}], "cursor": 12345
        })))
        .expect(1)
        .mount(&server)
        .await;
    let response = client.push(&request).await.unwrap();
    assert_eq!(response.accepted, ["op-1"]);
    assert_eq!(response.rejected[0].reason, "bad entity");
    let sent = String::from_utf8(requests(&server).await[0].body.clone()).unwrap();
    assert!(sent.contains(r#""Notes":"kept as written""#), "{sent}");
}

// MARK: - Telling "no sidecar" from "not right now"

#[tokio::test]
async fn a_missing_plugin_is_absent_not_a_failure_to_retry() {
    let (server, client) = mock().await;
    respond(&server, "POST", "/aoide/next", ResponseTemplate::new(404)).await;
    respond(
        &server,
        "GET",
        "/aoide/discover",
        ResponseTemplate::new(405),
    )
    .await;
    let request = NextRequest::new("seed", &[], &[], NextMode::Infinity, 20);
    let next = client.next(&request).await.unwrap_err();
    assert_eq!(
        next,
        SidecarError::Absent {
            path: "aoide/next".into()
        }
    );
    assert!(next.is_absent());
    assert!(client.discover(30).await.unwrap_err().is_absent());
}

#[tokio::test]
async fn a_server_that_is_merely_down_is_worth_trying_again() {
    let (server, client) = mock().await;
    respond(
        &server,
        "GET",
        "/aoide/sync/pull",
        ResponseTemplate::new(502),
    )
    .await;
    respond(
        &server,
        "GET",
        "/aoide/sync/status",
        ResponseTemplate::new(401),
    )
    .await;
    let bad_gateway = client.pull(None, 500).await.unwrap_err();
    assert!(!bad_gateway.is_absent());
    assert!(
        !bad_gateway.is_server_fault(),
        "a gateway error is not the op's fault"
    );
    let unauthorized = client.status().await.unwrap_err();
    assert_eq!(unauthorized, SidecarError::Unauthorized);
    assert!(!unauthorized.is_absent());

    // Nothing listening at all: a local port that was free a moment ago.
    let port = std::net::TcpListener::bind(("localhost", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let closed = SidecarClient::new(&format!("http://localhost:{port}"), TOKEN).unwrap();
    assert!(matches!(
        closed.status().await,
        Err(SidecarError::Network(_))
    ));
}

/// "HTTP 500" alone tells nobody anything; the server's own words are the diagnosis.
#[tokio::test]
async fn the_servers_own_message_reaches_the_caller() {
    let (server, client) = mock().await;
    respond(
        &server,
        "POST",
        "/aoide/sync/push",
        ResponseTemplate::new(500).set_body_string("NullReferenceException in SyncController"),
    )
    .await;
    let error = client
        .push(&PushRequest {
            device_id: "phone".into(),
            ops: vec![],
        })
        .await
        .unwrap_err();
    assert!(error.is_server_fault());
    let described = error.to_string();
    assert!(described.contains("500"));
    assert!(described.contains("NullReferenceException"));
    assert!(described.contains("aoide/sync/push"));
}

#[tokio::test]
async fn an_error_with_no_body_still_reads_as_a_sentence() {
    let (server, client) = mock().await;
    respond(
        &server,
        "GET",
        "/aoide/sync/pull",
        ResponseTemplate::new(503),
    )
    .await;
    let error = client.pull(None, 500).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "aoide/sync/pull returned an error (HTTP 503)"
    );
}

/// The contract's storage failure: keep the ops queued, never isolate them.
#[tokio::test]
async fn storage_unavailable_is_transient_and_carries_the_problem_details() {
    let (server, client) = mock().await;
    let problem =
        json!({"title": "Sync storage unavailable", "detail": "SqliteException: SQLite Error 26"});
    respond(
        &server,
        "POST",
        "/aoide/sync/push",
        ResponseTemplate::new(503).set_body_json(problem),
    )
    .await;
    let error = client
        .push(&PushRequest {
            device_id: "phone".into(),
            ops: vec![],
        })
        .await
        .unwrap_err();
    assert!(
        matches!(&error, SidecarError::Status { status: 503, detail: Some(d), .. } if d.contains("Sync storage unavailable"))
    );
    assert!(!error.is_server_fault());
}

// MARK: - Status and capabilities

#[tokio::test]
async fn the_status_reads_and_names_the_accepted_entities() {
    let (server, client) = mock().await;
    let body = json!({"databasePath": "/config/data/aoide-sidecar/aoide-sync.db", "schemaVersion": 1, "writable": true,
        "journalMode": "wal", "directoryWritable": true, "cursor": 2, "opCount": 2, "pluginVersion": "1.18.0.0",
        "acceptedEntities": ["playlists", "track_flags"]});
    respond(
        &server,
        "GET",
        "/aoide/sync/status",
        ResponseTemplate::new(200).set_body_json(body),
    )
    .await;
    let status = client.status().await.unwrap();
    assert_eq!(status.journal_mode.as_deref(), Some("wal"));
    assert_eq!(status.writable, Some(true));
    let accepted = SyncTransport::accepted_entities(&client).await.unwrap();
    assert!(accepted.contains("track_flags"));
}

/// An older server answers without the list: nothing new is accepted.
#[tokio::test]
async fn a_status_without_the_list_accepts_nothing_new_and_no_status_says_nothing() {
    let (server, client) = mock().await;
    respond(
        &server,
        "GET",
        "/aoide/sync/status",
        ResponseTemplate::new(200).set_body_json(json!({"schemaVersion": 1})),
    )
    .await;
    assert_eq!(
        SyncTransport::accepted_entities(&client).await,
        Some(Default::default())
    );

    let (server, client) = mock().await;
    respond(
        &server,
        "GET",
        "/aoide/sync/status",
        ResponseTemplate::new(404),
    )
    .await;
    assert_eq!(SyncTransport::accepted_entities(&client).await, None);
}

// MARK: - Covers

#[tokio::test]
async fn covers_are_asked_for_uploaded_and_fetched_by_hash() {
    let (server, client) = mock().await;
    let hash = "8581e780";
    let route = format!("/aoide/images/{hash}");
    respond(
        &server,
        "HEAD",
        "/aoide/images/missing",
        ResponseTemplate::new(404),
    )
    .await;
    respond(&server, "HEAD", &route, ResponseTemplate::new(200)).await;
    respond(
        &server,
        "GET",
        &route,
        ResponseTemplate::new(200).set_body_bytes(b"cover bytes".to_vec()),
    )
    .await;
    respond(
        &server,
        "GET",
        "/aoide/images/missing",
        ResponseTemplate::new(404),
    )
    .await;
    Mock::given(method("PUT"))
        .and(path(route.as_str()))
        .and(header("content-type", "image/png"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    assert!(client.has_image(hash).await.unwrap());
    assert!(!client.has_image("missing").await.unwrap());
    assert_eq!(
        client.get_image(hash).await.unwrap().as_deref(),
        Some(&b"cover bytes"[..])
    );
    assert_eq!(client.get_image("missing").await.unwrap(), None);
    client
        .put_image(hash, b"png bytes".to_vec(), "image/png")
        .await
        .unwrap();
    let put = requests(&server)
        .await
        .into_iter()
        .find(|r| r.method.as_str() == "PUT")
        .unwrap();
    assert_eq!(put.body, b"png bytes");
}

/// An older sidecar without the image endpoints.
#[tokio::test]
async fn uploading_to_a_sidecar_without_covers_is_absent() {
    let (_server, client) = mock().await;
    assert!(
        client
            .put_image("abc", vec![1], "image/jpeg")
            .await
            .unwrap_err()
            .is_absent()
    );
}

// MARK: - Infinity and Discover

#[tokio::test]
async fn next_posts_the_contracts_request_and_reads_every_factor() {
    let (server, client) = mock().await;
    let response = json!({"candidates": [
        {"id": "a", "score": 0.83, "factors": {"taste": 0.71, "freshness": 1.0, "kinship": 1.0, "similarity": 0.62, "mixability": 0.79, "arc": 0.9}},
        {"id": "b", "score": 0.4, "factors": {"taste": 0.2, "freshness": 0.5, "kinship": 0.5, "similarity": 0.5, "mixability": null, "arc": 0.5}}
    ], "profile": {"events": 184, "since": 1749600000000_i64}});
    Mock::given(method("POST"))
        .and(path("/aoide/next"))
        .and(body_json(
            json!({"seed": "s", "queue": ["q1"], "recent": ["r1"], "mode": "autodj", "limit": 20}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .expect(1)
        .mount(&server)
        .await;
    let request = NextRequest::new(
        "s",
        &["q1".to_owned()],
        &["r1".to_owned()],
        NextMode::Autodj,
        20,
    );
    let answer = client.next(&request).await.unwrap();
    assert_eq!(answer.candidates[0].factors.kinship, Some(1.0));
    assert_eq!(answer.candidates[1].factors.mixability, None);
    assert_eq!(answer.profile.events, 184);
}

#[tokio::test]
async fn discover_asks_for_a_limit_in_range_and_keeps_the_nulls() {
    let (server, client) = mock().await;
    let body = json!({"week": "2026-W43", "candidates": [
        {"id": "3b1c", "score": 0.75, "factors": {"taste": 0.75, "kinship": null, "freshness": 1, "similarity": null, "mixability": null, "arc": null}}
    ], "profile": {"events": 184, "since": 1784592000000_i64}});
    for limit in ["30", "1", "100"] {
        Mock::given(method("GET"))
            .and(path("/aoide/discover"))
            .and(query_param("limit", limit))
            .respond_with(ResponseTemplate::new(200).set_body_json(body.clone()))
            .expect(1)
            .mount(&server)
            .await;
    }
    let answer = client.discover(30).await.unwrap();
    assert_eq!(answer.week, "2026-W43");
    assert_eq!(answer.candidates[0].factors.similarity, None);
    client.discover(0).await.unwrap();
    client.discover(500).await.unwrap();
}

// MARK: - Measurements

#[tokio::test]
async fn sound_bounds_tell_measured_from_nothing_to_trim_from_pending_from_unknown() {
    let (server, client) = mock().await;
    let body = json!({"bounds": {"3b1c": {"soundStartMs": 1940, "soundEndMs": 5200}, "a71f": null}, "pending": ["9c0e"]});
    Mock::given(method("GET"))
        .and(path("/aoide/sound-bounds"))
        .and(query_param("ids", "3b1c,a71f,9c0e,ffff"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .expect(1)
        .mount(&server)
        .await;
    let ids: Vec<String> = ["3b1c", "a71f", "9c0e", "ffff"].map(str::to_owned).to_vec();
    let bounds = client.sound_bounds(&ids).await.unwrap();
    assert_eq!(
        bounds.get("3b1c"),
        Measurement::Measured(&SoundBounds {
            sound_start_ms: 1940,
            sound_end_ms: 5200
        })
    );
    assert_eq!(bounds.get("a71f"), Measurement::NothingToReport);
    assert_eq!(bounds.get("9c0e"), Measurement::Pending);
    assert_eq!(bounds.get("ffff"), Measurement::Unknown);
}

/// The endpoint takes at most 200 ids; more go in several calls, merged.
#[tokio::test]
async fn more_than_two_hundred_ids_go_in_several_calls() {
    let (server, client) = mock().await;
    respond(
        &server,
        "GET",
        "/aoide/sound-bounds",
        ResponseTemplate::new(200).set_body_json(json!({"bounds": {}, "pending": ["x"]})),
    )
    .await;
    let ids: Vec<String> = (0..450).map(|i| format!("t{i}")).collect();
    let merged = client.sound_bounds(&ids).await.unwrap();
    let calls = requests(&server).await;
    assert_eq!(calls.len(), 3);
    for call in &calls {
        let ids = call
            .url
            .query_pairs()
            .find(|(k, _)| k == "ids")
            .unwrap()
            .1
            .split(',')
            .count();
        assert!(ids <= crate::MAX_IDS_PER_CALL);
    }
    assert_eq!(merged.pending.len(), 3);
    assert!(client.sound_bounds(&[]).await.unwrap().bounds.is_empty());
    assert_eq!(requests(&server).await.len(), 3, "no ids, no request");
}

#[tokio::test]
async fn measured_bounds_are_posted_with_nothing_to_trim_as_null() {
    let (server, client) = mock().await;
    Mock::given(method("POST"))
        .and(path("/aoide/sound-bounds"))
        .and(body_json(
            json!({"bounds": {"a": {"soundStartMs": 10, "soundEndMs": 20}, "b": null}}),
        ))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let mut bounds = BTreeMap::new();
    bounds.insert(
        "a".to_owned(),
        Some(SoundBounds {
            sound_start_ms: 10,
            sound_end_ms: 20,
        }),
    );
    bounds.insert("b".to_owned(), None);
    client.post_sound_bounds(&bounds).await.unwrap();
}

/// Every field independently nullable; `bpmStability` absent from servers before 1.12.
#[tokio::test]
async fn audio_analysis_keeps_each_field_independently_nullable() {
    let (server, client) = mock().await;
    let body = json!({"analysis": {
        "3b1c": {"loudnessLufs": -9.7, "truePeakDbfs": -0.3, "bpm": 127.97, "bpmConfidence": 1.0, "bpmStability": 1.0},
        "a71f": {"loudnessLufs": -14.2, "truePeakDbfs": -1.1, "bpm": null, "bpmConfidence": null},
        "c904": null
    }, "pending": ["9c0e"]});
    respond(
        &server,
        "GET",
        "/aoide/audio-analysis",
        ResponseTemplate::new(200).set_body_json(body),
    )
    .await;
    let ids: Vec<String> = ["3b1c", "a71f", "c904", "9c0e"].map(str::to_owned).to_vec();
    let analysis = client.audio_analysis(&ids).await.unwrap();
    let Measurement::Measured(tempo) = analysis.get("3b1c") else {
        panic!("3b1c measured")
    };
    assert_eq!(tempo.bpm, Some(127.97));
    let Measurement::Measured(ambient) = analysis.get("a71f") else {
        panic!("a71f measured")
    };
    assert_eq!(ambient.loudness_lufs, Some(-14.2));
    assert_eq!((ambient.bpm, ambient.bpm_stability), (None, None));
    assert_eq!(analysis.get("c904"), Measurement::NothingToReport);
    assert_eq!(analysis.get("9c0e"), Measurement::Pending);
}

#[tokio::test]
async fn coverage_reads_the_contracts_example() {
    let (server, client) = mock().await;
    let body = json!({"tracks": 4210, "soundBounds": 4198, "audioAnalysis": 4198, "beatGrids": 4198,
        "gridded": 3806, "arrangements": 4198, "described": 3790, "measuring": 1, "sweepEnabled": false,
        "summary": "4198 of 4210 tracks measured (100%), 3806 with a beat grid. Nothing outstanding."});
    respond(
        &server,
        "GET",
        "/aoide/analysis/coverage",
        ResponseTemplate::new(200).set_body_json(body),
    )
    .await;
    let coverage = client.analysis_coverage().await.unwrap();
    assert_eq!(coverage.tracks, 4210);
    assert_eq!(coverage.gridded, Some(3806));
    assert_eq!(coverage.sweep_enabled, Some(false));

    // An older server without the later counts still reads.
    let (server, client) = mock().await;
    let older = json!({"tracks": 10, "soundBounds": 5, "audioAnalysis": 5, "measuring": 0});
    respond(
        &server,
        "GET",
        "/aoide/analysis/coverage",
        ResponseTemplate::new(200).set_body_json(older),
    )
    .await;
    assert_eq!(client.analysis_coverage().await.unwrap().arrangements, None);
}

#[tokio::test]
async fn a_body_this_client_cannot_read_is_a_decode_error_naming_the_path() {
    let (server, client) = mock().await;
    respond(
        &server,
        "GET",
        "/aoide/analysis/coverage",
        ResponseTemplate::new(200).set_body_string("<html>"),
    )
    .await;
    let error = client.analysis_coverage().await.unwrap_err();
    assert!(
        matches!(error, SidecarError::Decode { ref path, .. } if path == "aoide/analysis/coverage")
    );
}

#[tokio::test]
async fn the_token_never_follows_a_redirect_to_another_origin() {
    let elsewhere = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(0)
        .mount(&elsewhere)
        .await;
    let (server, client) = mock().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(307)
                .insert_header("Location", format!("{}/aoide/sync/status", elsewhere.uri())),
        )
        .mount(&server)
        .await;
    assert!(client.status().await.is_err());
}
