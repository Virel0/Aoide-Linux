//! Cover bytes: held here, else fetched from the server once and kept; and a cover
//! chosen from a file reaching the server before the op that names it.

use std::sync::Arc;

use aoide_store::{BlobDirectory, PlaylistCover, Store, new_id};
use serde_json::json;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use super::store;
use crate::{CoverFetchError, SidecarClient, SyncEngine, cover_bytes};

const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 7, 7, 7];

/// Accepts whatever it is sent.
struct AcceptAll;

impl Respond for AcceptAll {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap_or_default();
        let accepted: Vec<serde_json::Value> = body["ops"]
            .as_array()
            .map(|ops| ops.iter().map(|op| op["opId"].clone()).collect())
            .unwrap_or_default();
        ResponseTemplate::new(200).set_body_json(json!({"accepted": accepted, "rejected": []}))
    }
}

fn blobs() -> BlobDirectory {
    BlobDirectory::open(std::env::temp_dir().join(format!("aoide-covers-{}", new_id()))).unwrap()
}

#[tokio::test]
async fn a_cover_is_fetched_once_kept_and_never_uploaded_back() {
    let server = MockServer::start().await;
    let hash = Store::image_hash(PNG);
    Mock::given(method("GET"))
        .and(path(format!("/aoide/images/{hash}")))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(PNG))
        .expect(1)
        .mount(&server)
        .await;
    let client = SidecarClient::new(&server.uri(), "tok").unwrap();
    let phone = store("phone");
    let blobs = blobs();

    let fetched = cover_bytes(&phone, &blobs, Some(&client), &hash, None)
        .await
        .unwrap();
    assert_eq!(fetched.as_deref(), Some(PNG));
    assert!(blobs.contains(&hash));
    let blob = phone.image(&hash).unwrap().unwrap();
    assert_eq!(blob.mime, "image/png");
    assert!(blob.uploaded_at.is_some(), "the server already has it");

    // Drawn again: from disk, no request (the mock expects exactly one).
    let again = cover_bytes(&phone, &blobs, Some(&client), &hash, None)
        .await
        .unwrap();
    assert_eq!(again.as_deref(), Some(PNG));
    std::fs::remove_dir_all(blobs.root()).unwrap();
}

#[tokio::test]
async fn bytes_that_are_not_the_named_ones_are_refused_and_not_kept() {
    let server = MockServer::start().await;
    let hash = Store::image_hash(b"the real picture");
    Mock::given(method("GET"))
        .and(path(format!("/aoide/images/{hash}")))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(PNG))
        .mount(&server)
        .await;
    let client = SidecarClient::new(&server.uri(), "tok").unwrap();
    let phone = store("phone");
    let blobs = blobs();
    let result = cover_bytes(&phone, &blobs, Some(&client), &hash, None).await;
    assert!(matches!(result, Err(CoverFetchError::Mismatch(_))));
    assert!(!blobs.contains(&hash));
    assert_eq!(phone.image(&hash).unwrap(), None);
    std::fs::remove_dir_all(blobs.root()).unwrap();
}

#[tokio::test]
async fn a_cover_nobody_has_yet_or_no_sidecar_or_no_hash_is_nothing_to_draw() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex("^/aoide/images/.*$"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let client = SidecarClient::new(&server.uri(), "tok").unwrap();
    let phone = store("phone");
    let blobs = blobs();
    let hash = Store::image_hash(b"not uploaded yet");
    for sidecar in [Some(&client), None] {
        assert_eq!(
            cover_bytes(&phone, &blobs, sidecar, &hash, None)
                .await
                .unwrap(),
            None
        );
    }
    assert_eq!(
        cover_bytes(&phone, &blobs, Some(&client), "../../secrets", None)
            .await
            .unwrap(),
        None
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "the bad hash never reached the server");
    std::fs::remove_dir_all(blobs.root()).unwrap();
}

/// A picture chosen from a file: bytes up before the op, from the blob directory.
#[tokio::test]
async fn a_cover_chosen_from_a_file_reaches_the_server_before_its_op() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/aoide/sync/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"acceptedEntities": []})))
        .mount(&server)
        .await;
    Mock::given(method("HEAD"))
        .and(path_regex("^/aoide/images/[0-9a-f]{64}$"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path_regex("^/aoide/images/[0-9a-f]{64}$"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/aoide/sync/push"))
        .respond_with(AcceptAll)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/aoide/sync/pull"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"ops": [], "cursor": 0, "hasMore": false})),
        )
        .mount(&server)
        .await;

    let phone = store("phone");
    let blobs = Arc::new(blobs());
    let list = phone.create_playlist("Mine", None, None, 1_000).unwrap();
    let file = blobs.root().join("picked.png");
    std::fs::write(&file, PNG).unwrap();
    phone
        .set_cover_from_file(&list.id, &file, &blobs, 1_100)
        .unwrap();
    let hash = Store::image_hash(PNG);
    assert_eq!(
        phone
            .playlist_cover(&phone.playlist(&list.id).unwrap().unwrap())
            .unwrap(),
        PlaylistCover::Image {
            hash: hash.clone(),
            mime: Some("image/png".into())
        }
    );

    let client = SidecarClient::new(&server.uri(), "tok").unwrap();
    let report = SyncEngine::new(Arc::clone(&phone), client)
        .with_image_bytes(blobs.clone())
        .sync()
        .await
        .unwrap();
    assert_eq!(report.images_uploaded, 1);
    assert_eq!(report.pushed, 2, "the create and the cover");
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
        .expect("uploaded");
    let push = calls
        .iter()
        .position(|(_, route)| route.ends_with("/sync/push"))
        .expect("pushed");
    assert!(put < push, "{calls:?}");
    let uploaded = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|request| request.method.as_str() == "PUT")
        .unwrap();
    assert_eq!(uploaded.url.path(), format!("/aoide/images/{hash}"));
    assert_eq!(uploaded.body, PNG);
    assert!(phone.image(&hash).unwrap().unwrap().uploaded_at.is_some());
    std::fs::remove_dir_all(blobs.root()).unwrap();
}
