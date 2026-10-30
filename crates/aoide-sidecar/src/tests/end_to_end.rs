//! The engine over the real HTTP client, against a mock server that keeps an op log —
//! so the request and response bodies that cross the wire are the ones that sync.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use aoide_store::{
    PullCursor, PullResponse, PulledOp, PushCursor, PushRequest, PushResponse, Store, TrackLink,
};
use serde_json::json;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use super::{item_ids, pending, store, synced_rows};
use crate::{ImageByteSource, SidecarClient, SyncEngine};

#[derive(Default)]
struct Log {
    rows: Vec<PulledOp>,
}

#[derive(Clone, Default)]
struct LogSidecar(Arc<Mutex<Log>>);

struct PushEndpoint(LogSidecar);
struct PullEndpoint(LogSidecar);

impl Respond for PushEndpoint {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let Ok(push) = serde_json::from_slice::<PushRequest>(&request.body) else {
            return ResponseTemplate::new(400);
        };
        let mut log = self.0.0.lock().unwrap();
        let mut accepted = Vec::new();
        for op in push.ops {
            accepted.push(op.op_id.clone());
            if log.rows.iter().any(|row| row.op_id == op.op_id) {
                continue;
            }
            let seq = log.rows.len() as i64 + 1;
            log.rows.push(PulledOp {
                op_id: op.op_id,
                entity: op.entity,
                entity_id: op.entity_id,
                operation: op.operation,
                payload: op.payload,
                created_at: op.created_at,
                seq,
                device_id: push.device_id.clone(),
                received_at: 2_000_000_000_000,
                author_user_id: None,
            });
        }
        let cursor = Some(PushCursor(log.rows.len() as i64));
        ResponseTemplate::new(200).set_body_json(PushResponse {
            accepted,
            rejected: vec![],
            cursor,
        })
    }
}

impl Respond for PullEndpoint {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let query: BTreeMap<String, String> = request
            .url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let since: i64 = query.get("since").and_then(|s| s.parse().ok()).unwrap_or(0);
        let limit: usize = query
            .get("limit")
            .and_then(|s| s.parse().ok())
            .unwrap_or(500);
        let log = self.0.0.lock().unwrap();
        let above: Vec<PulledOp> = log
            .rows
            .iter()
            .filter(|row| row.seq > since)
            .cloned()
            .collect();
        let page: Vec<PulledOp> = above.iter().take(limit).cloned().collect();
        let cursor = PullCursor(page.last().map_or(since, |row| row.seq));
        ResponseTemplate::new(200).set_body_json(PullResponse {
            has_more: above.len() > page.len(),
            ops: page,
            cursor,
            unreadable_ops: 0,
        })
    }
}

async fn sidecar(accepted: serde_json::Value) -> (MockServer, LogSidecar) {
    let server = MockServer::start().await;
    let log = LogSidecar::default();
    Mock::given(method("POST"))
        .and(path("/melete/aoide/sync/push"))
        .respond_with(PushEndpoint(log.clone()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/melete/aoide/sync/pull"))
        .respond_with(PullEndpoint(log.clone()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/melete/aoide/sync/status"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"acceptedEntities": accepted})),
        )
        .mount(&server)
        .await;
    (server, log)
}

fn engine(server: &MockServer, store: &Arc<Store>) -> SyncEngine<SidecarClient> {
    // Behind a reverse-proxy subpath, as a real install may be.
    let client = SidecarClient::new(&format!("{}/melete", server.uri()), "tok-123").unwrap();
    SyncEngine::new(Arc::clone(store), client).with_clock(|| 2_000_000_000_000)
}

#[tokio::test]
async fn a_library_crosses_the_wire_and_arrives_byte_for_byte() {
    let all = json!([
        "playlists",
        "playlist_items",
        "folders",
        "likes",
        "play_events",
        "queue_state",
        "track_flags"
    ]);
    let (server, _log) = sidecar(all).await;
    let phone = store("phone");
    let mac = store("mac");

    let list = phone
        .create_playlist("Late Night", None, Some("for \"the\" drive"), 1_000)
        .unwrap();
    phone
        .add_tracks(
            &[TrackLink::new("t1", "k1"), TrackLink::new("t2", "k2")],
            &list.id,
            1_100,
        )
        .unwrap();
    phone.set_not_interested(true, "t9", "k9", 1_200).unwrap();
    let queued = pending(&phone).len();

    let sent = engine(&server, &phone).sync().await.unwrap();
    assert_eq!(sent.pushed, queued);
    let received = engine(&server, &mac).sync().await.unwrap();
    assert_eq!(received.applied, queued);
    assert_eq!(synced_rows(&mac), synced_rows(&phone));
    assert_eq!(item_ids(&mac, &list.id), ["t1", "t2"]);
    // Taste flags went because the server listed them; they are not in `synced_rows`.
    assert!(mac.flags("t9").unwrap().unwrap().not_interested);
    assert!(pending(&mac).is_empty());
}

#[tokio::test]
async fn a_server_that_does_not_list_taste_flags_gets_none_of_them() {
    let older = json!([
        "playlists",
        "playlist_items",
        "folders",
        "likes",
        "play_events",
        "queue_state"
    ]);
    let (server, log) = sidecar(older).await;
    let phone = store("phone");
    phone.create_playlist("Focus", None, None, 1_000).unwrap();
    phone.set_not_interested(true, "t9", "k9", 1_200).unwrap();

    engine(&server, &phone).sync().await.unwrap();
    let entities: Vec<String> = log
        .0
        .lock()
        .unwrap()
        .rows
        .iter()
        .map(|row| row.entity.clone())
        .collect();
    assert_eq!(entities, ["playlists"]);
    assert_eq!(
        pending(&phone)
            .iter()
            .map(|op| op.entity.as_str())
            .collect::<Vec<_>>(),
        ["trackFlags"]
    );
    assert_eq!(phone.quarantine_count().unwrap(), 0);
}

struct Bytes(Vec<u8>);

impl ImageByteSource for Bytes {
    fn bytes(&self, _sha256: &str) -> Option<Vec<u8>> {
        Some(self.0.clone())
    }
}

/// The ordering rule: the cover's bytes reach the server before the op naming them.
#[tokio::test]
async fn a_cover_is_uploaded_before_the_op_that_names_it() {
    let (server, _log) = sidecar(json!(["playlists"])).await;
    Mock::given(method("HEAD"))
        .and(path_regex("^/melete/aoide/images/[0-9a-f]+$"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex("^/melete/aoide/images/[0-9a-f]+$"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let phone = store("phone");
    let bytes = b"\x89PNG not really".to_vec();
    let hash = Store::image_hash(&bytes);
    phone
        .register_image(&hash, "image/png", bytes.len() as i64, 1_000)
        .unwrap();
    let mut list = phone
        .create_playlist("Photographed", None, None, 1_000)
        .unwrap();
    list.image_hash = Some(hash.clone());
    list.image_mime = Some("image/png".into());
    phone.record(list, false, 1_100).unwrap();

    let report = engine(&server, &phone)
        .with_image_bytes(Arc::new(Bytes(bytes.clone())))
        .sync()
        .await
        .unwrap();
    assert_eq!(report.images_uploaded, 1);
    assert!(phone.image(&hash).unwrap().unwrap().uploaded_at.is_some());

    let calls: Vec<(String, String)> = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .map(|request| (request.method.to_string(), request.url.path().to_owned()))
        .collect();
    let put = calls
        .iter()
        .position(|(verb, _)| verb == "PUT")
        .expect("the cover was uploaded");
    let push = calls
        .iter()
        .position(|(_, route)| route.ends_with("/sync/push"))
        .expect("the playlist was pushed");
    assert!(put < push, "uploaded after the op that names it: {calls:?}");
    let uploaded = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.method.as_str() == "PUT")
        .unwrap();
    assert_eq!(uploaded.url.path(), format!("/melete/aoide/images/{hash}"));
    assert_eq!(uploaded.body, bytes);
}
