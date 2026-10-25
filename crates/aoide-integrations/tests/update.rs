//! The update check against a local stand-in for GitHub's API. No network.

use std::time::{Duration, UNIX_EPOCH};

use aoide_integrations::update::{
    CHECK_INTERVAL, UpdateCheck, UpdateChecker, UpdateError, UpdateState,
};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PATH: &str = "/repos/Virel0/aoide-linux/releases/latest";

async fn github(status: u16, body: serde_json::Value) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(PATH))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(&server)
        .await;
    server
}

fn checker(server: &MockServer) -> UpdateChecker {
    UpdateChecker::with_url(&format!("{}{PATH}", server.uri())).unwrap()
}

fn latest(tag: &str) -> serde_json::Value {
    json!({
        "tag_name": tag,
        "name": "Aoide 0.2.0",
        "body": "Tray icon, Discord, mixes.",
        "html_url": format!("https://github.com/Virel0/aoide-linux/releases/tag/{tag}"),
        "draft": false,
        "prerelease": false,
    })
}

#[tokio::test]
async fn a_newer_release_is_reported_and_nothing_else_is_sent() {
    let server = github(200, latest("v0.2.0")).await;
    let mut state = UpdateState::default();
    let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let outcome = checker(&server)
        .check("0.1.0", &mut state, now)
        .await
        .unwrap();
    let UpdateCheck::Available(release) = outcome else {
        panic!("expected an update, got {outcome:?}");
    };
    assert_eq!(release.version.to_string(), "0.2.0");
    assert_eq!(release.notes, "Tray icon, Discord, mixes.");
    assert_eq!(state.last_checked, Some(1_800_000_000));

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert!(request.url.query().is_none(), "no query string");
    assert!(request.body.is_empty());
    assert_eq!(request.headers["user-agent"], "aoide-linux");
    assert_eq!(request.headers["accept"], "application/vnd.github+json");
    let mut names: Vec<_> = request
        .headers
        .keys()
        .map(|k| k.as_str().to_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["accept", "host", "user-agent", "x-github-api-version"]
    );
}

#[tokio::test]
async fn the_running_version_is_up_to_date() {
    let server = github(200, latest("v0.1.0")).await;
    let mut state = UpdateState::default();
    let outcome = checker(&server)
        .check("0.1.0", &mut state, UNIX_EPOCH + Duration::from_secs(1_000))
        .await;
    assert_eq!(outcome, Ok(UpdateCheck::UpToDate));
}

#[tokio::test]
async fn no_release_yet_is_up_to_date() {
    let server = github(404, json!({ "message": "Not Found" })).await;
    let mut state = UpdateState::default();
    let outcome = checker(&server)
        .check("0.1.0", &mut state, UNIX_EPOCH + Duration::from_secs(1_000))
        .await;
    assert_eq!(outcome, Ok(UpdateCheck::UpToDate));
}

#[tokio::test]
async fn at_most_one_request_a_day_even_after_a_failure() {
    let server = github(503, json!({})).await;
    let c = checker(&server);
    let mut state = UpdateState::default();
    let start = UNIX_EPOCH + Duration::from_secs(1_800_000_000);

    assert_eq!(
        c.check("0.1.0", &mut state, start).await,
        Err(UpdateError::Refused(503))
    );
    for later in [
        Duration::from_secs(60),
        CHECK_INTERVAL - Duration::from_secs(1),
    ] {
        assert_eq!(
            c.check("0.1.0", &mut state, start + later).await,
            Ok(UpdateCheck::NotDue)
        );
    }
    assert_eq!(server.received_requests().await.unwrap().len(), 1);

    let _ = c.check("0.1.0", &mut state, start + CHECK_INTERVAL).await;
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn a_bad_running_version_sends_nothing() {
    let server = github(200, latest("v0.2.0")).await;
    let mut state = UpdateState::default();
    let outcome = checker(&server)
        .check("dev", &mut state, UNIX_EPOCH + Duration::from_secs(1_000))
        .await;
    assert!(matches!(outcome, Err(UpdateError::BadCurrentVersion(_))));
    assert_eq!(state, UpdateState::default());
    assert!(server.received_requests().await.unwrap().is_empty());
}
