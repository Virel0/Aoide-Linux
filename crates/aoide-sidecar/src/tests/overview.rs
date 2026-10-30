//! The sync status panel's one plain struct: what it says before, after and while
//! things go wrong, with the server's own words.

use std::sync::Arc;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::sidecar_stub::Sidecar;
use super::store;
use crate::{SidecarClient, SidecarError, SyncEngine};
use aoide_store::SyncPhase;

#[tokio::test]
async fn before_anything_it_says_what_waits_and_that_nothing_has_synced() {
    let store = store("desk");
    store.create_playlist("Focus", None, None, 1_000).unwrap();
    let server = Sidecar::with_clock(2_000);
    let engine = SyncEngine::new(Arc::clone(&store), server.link("listener"));
    let overview = engine.overview().unwrap();
    assert!(!overview.running);
    assert_eq!(overview.phase, SyncPhase::Idle);
    assert_eq!(overview.last_synced_at, None);
    assert_eq!(overview.last_attempt_at, None);
    assert_eq!(overview.waiting, 1);
    assert_eq!(
        (overview.pushed, overview.pulled, overview.applied),
        (0, 0, 0)
    );
    assert_eq!(overview.quarantined, 0);
    assert_eq!(overview.held_back, 0);
    assert_eq!(overview.blocked_by_share, 0);
    assert_eq!(overview.error, None);
    assert!(!overview.sidecar_absent);
}

#[tokio::test]
async fn after_a_clean_sync_it_has_the_counts_and_remembers_when_across_restarts() {
    let store = store("desk");
    store.create_playlist("Focus", None, None, 1_000).unwrap();
    store.set_not_interested(true, "t1", "k1", 1_100).unwrap();
    let server = Sidecar::with_clock(2_000);
    let engine = SyncEngine::new(Arc::clone(&store), server.link("listener")).with_clock(|| 50_000);
    engine.sync().await.unwrap();

    let overview = engine.overview().unwrap();
    assert_eq!(overview.last_synced_at, Some(50_000));
    assert_eq!(overview.last_attempt_at, Some(50_000));
    assert_eq!(overview.pushed, 1);
    assert_eq!(overview.pulled, 1, "our own op came back");
    // The stub cannot say what it accepts, so the flag waits rather than dying.
    assert_eq!(overview.held_back, 1);
    assert_eq!(overview.held_entities, ["trackFlags"]);
    assert_eq!(overview.waiting, 1);
    assert_eq!(overview.error, None);
    assert!(overview.last_report.is_some());

    let restarted = SyncEngine::new(Arc::clone(&store), server.link("listener"));
    assert_eq!(restarted.overview().unwrap().last_synced_at, Some(50_000));
}

#[tokio::test]
async fn a_refused_change_shows_as_set_aside() {
    let store = store("desk");
    let list = store.create_playlist("Focus", None, None, 1_000).unwrap();
    let server = Sidecar::with_clock(2_000);
    let op = super::pending(&store)[0].op_id.clone();
    server.reject(&op, "payload must be a JSON object");
    let engine = SyncEngine::new(Arc::clone(&store), server.link("listener"));
    engine.sync().await.unwrap();
    let overview = engine.overview().unwrap();
    assert_eq!(overview.quarantined, 1);
    assert_eq!(overview.waiting, 0);
    assert!(
        store.playlist(&list.id).unwrap().is_some(),
        "the row itself stays"
    );
}

/// The Jellyfin sidecar's 503 named the real cause in a ProblemDetails body; that
/// is what a person needs to read, not "HTTP 503".
#[tokio::test]
async fn a_failed_sync_carries_the_servers_own_words() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/aoide/sync/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"acceptedEntities": []})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/aoide/sync/push"))
        .respond_with(ResponseTemplate::new(503).set_body_json(json!({
            "title": "Sync storage unavailable",
            "detail": "SqliteException: SQLite Error 26: 'file is not a database'.",
            "instance": "/config/data/aoide-sidecar/aoide-sync.db"
        })))
        .mount(&server)
        .await;
    let store = store("desk");
    store.create_playlist("Focus", None, None, 1_000).unwrap();
    let client = SidecarClient::new(&server.uri(), "tok").unwrap();
    let engine = SyncEngine::new(Arc::clone(&store), client).with_clock(|| 7_000);
    assert!(engine.sync().await.is_err());

    let overview = engine.overview().unwrap();
    let error = overview.error.unwrap();
    assert!(error.contains("HTTP 503"), "{error}");
    assert!(
        error.contains(
            "Sync storage unavailable: SqliteException: SQLite Error 26: 'file is not a database'."
        ),
        "{error}"
    );
    assert!(matches!(overview.phase, SyncPhase::Failed { .. }));
    assert_eq!(overview.last_synced_at, None);
    assert_eq!(overview.last_attempt_at, Some(7_000));
    assert_eq!(overview.waiting, 1, "kept queued, not quarantined");
    assert_eq!(overview.quarantined, 0);
    assert!(!overview.sidecar_absent);
}

#[tokio::test]
async fn a_server_with_no_sidecar_says_so() {
    let server = MockServer::start().await;
    let store = store("desk");
    store.create_playlist("Focus", None, None, 1_000).unwrap();
    let client = SidecarClient::new(&server.uri(), "tok").unwrap();
    let engine = SyncEngine::new(Arc::clone(&store), client);
    assert!(engine.sync().await.is_err());
    assert!(engine.overview().unwrap().sidecar_absent);
}

#[test]
fn the_servers_words_come_out_of_a_problem_body_or_a_plain_one() {
    let status = |detail: Option<&str>| SidecarError::Status {
        status: 500,
        path: "aoide/sync/push".into(),
        detail: detail.map(str::to_owned),
    };
    assert_eq!(
        status(Some(
            r#"{"error":"the seed isn't a track this user can see"}"#
        ))
        .server_text()
        .as_deref(),
        Some("the seed isn't a track this user can see"),
        "Melete's shape"
    );
    assert_eq!(
        status(Some(r#"{"title":"Malformed request body"}"#))
            .server_text()
            .as_deref(),
        Some("Malformed request body")
    );
    assert_eq!(
        status(Some("Error processing request."))
            .server_text()
            .as_deref(),
        Some("Error processing request.")
    );
    assert_eq!(status(None).server_text(), None);
    assert_eq!(SidecarError::Unauthorized.server_text(), None);
}
