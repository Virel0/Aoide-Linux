//! Handoff: `GET /aoide/queue` read leniently, the server judged on its own clock,
//! the op log as the fallback, and the queue row pushed on its own.

use std::sync::Arc;

use aoide_store::{
    QueueBroadcast, QueueFreshness, QueuePolicy, QueueSnapshot, QueueState, Record, now_millis,
};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::sidecar_stub::Sidecar;
use super::{pending, store};
use crate::{
    QueueEntry, QueueSource, SidecarClient, SidecarError, SyncEngine, queues_from_server,
    resumable_queues,
};

fn entry(device: &str, age: Option<f64>, received: Option<i64>, tracks: &[&str]) -> QueueEntry {
    QueueEntry {
        device_id: device.to_owned(),
        is_current_device: false,
        age_seconds: age,
        received_at: received,
        updated_at: None,
        device_name: Some(format!("{device} name")),
        track_ids: tracks.iter().map(|t| (*t).to_owned()).collect(),
        position: 0,
        elapsed_ms: 0,
    }
}

struct Fixed(Result<Vec<QueueEntry>, SidecarError>);

impl QueueSource for Fixed {
    async fn queues(&self) -> Result<Vec<QueueEntry>, SidecarError> {
        self.0.clone()
    }
}

// MARK: - The endpoint

#[tokio::test]
async fn queue_entries_are_read_from_the_payload_whichever_way_track_ids_travel() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/aoide/queue"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "deviceId": "laptop", "isCurrentDevice": false, "ageSeconds": 12,
              "receivedAt": 1786000000000_i64, "updatedAt": 1786000000000_i64,
              "payload": { "deviceName": "Desk PC", "trackIds": ["a", "b"], "position": 1, "elapsedMs": 41200 } },
            // The phone stores the list as JSON text, and the server relays it verbatim.
            { "deviceId": "phone", "isCurrentDevice": false, "ageSeconds": 30,
              "payload": { "deviceName": "Phone", "trackIds": "[\"c\",\"d\",\"e\"]", "position": 2, "elapsedMs": 5 } },
            // An older desktop's flat shape.
            { "deviceId": "old", "isCurrentDevice": true, "ageSeconds": 1,
              "deviceName": "Old", "trackIds": ["f"], "position": 0 },
            { "isCurrentDevice": false }
        ])))
        .mount(&server)
        .await;
    let client = SidecarClient::new(&server.uri(), "tok").unwrap();
    let entries = client.queues().await.unwrap();
    assert_eq!(entries.len(), 3, "the entry with no device id is skipped");

    assert_eq!(entries[0].device_id, "laptop");
    assert_eq!(entries[0].device_name.as_deref(), Some("Desk PC"));
    assert_eq!(entries[0].track_ids, ["a", "b"]);
    assert_eq!(entries[0].position, 1);
    assert_eq!(entries[0].elapsed_ms, 41_200);
    assert_eq!(entries[0].age_seconds, Some(12.0));
    assert_eq!(entries[0].received_at, Some(1_786_000_000_000));

    assert_eq!(entries[1].track_ids, ["c", "d", "e"]);
    assert_eq!(entries[1].position, 2);

    assert!(entries[2].is_current_device);
    assert_eq!(entries[2].device_name.as_deref(), Some("Old"));
    assert_eq!(entries[2].track_ids, ["f"]);
}

#[tokio::test]
async fn a_wrapped_list_reads_the_same_and_an_older_server_is_absent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/aoide/queue"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "entries": [ { "deviceId": "laptop", "ageSeconds": 1, "payload": { "trackIds": ["a"] } } ]
        })))
        .mount(&server)
        .await;
    let client = SidecarClient::new(&server.uri(), "tok").unwrap();
    assert_eq!(client.queues().await.unwrap().len(), 1);

    let empty = MockServer::start().await;
    let client = SidecarClient::new(&empty.uri(), "tok").unwrap();
    assert!(client.queues().await.unwrap_err().is_absent());
}

// MARK: - Choosing

/// A device whose clock is a year fast must not win by its `updatedAt`.
#[test]
fn the_server_age_decides_not_the_writers_clock() {
    let mut fast_clock = entry("fast", Some(600.0), None, &["x"]);
    fast_clock.updated_at = Some(i64::MAX / 2);
    let entries = [fast_clock, entry("phone", Some(20.0), None, &["y"])];
    let queues = queues_from_server(&entries, "desk", 1_000_000);
    let devices: Vec<&str> = queues.iter().map(|q| q.device_id.as_str()).collect();
    assert_eq!(devices, ["phone", "fast"]);
    assert_eq!(queues[0].age_ms, 20_000);
    assert_eq!(queues[0].freshness, QueueFreshness::Server);
    assert_eq!(queues[0].played_at(), 1_000_000 - 20_000);
}

#[test]
fn this_device_empty_queues_and_stale_ones_are_never_offered() {
    let mut mine = entry("other-id", Some(1.0), None, &["a"]);
    mine.is_current_device = true;
    let entries = [
        mine,
        entry("desk", Some(1.0), None, &["a"]),
        entry("empty", Some(1.0), None, &[]),
        entry(
            "stale",
            Some((QueuePolicy::RESUME_WINDOW / 1000 + 1) as f64),
            None,
            &["a"],
        ),
        entry("phone", Some(2.0), None, &["a"]),
    ];
    let queues = queues_from_server(&entries, "desk", 0);
    let devices: Vec<&str> = queues.iter().map(|q| q.device_id.as_str()).collect();
    assert_eq!(devices, ["phone"]);
}

/// Seconds and milliseconds are never compared with each other.
#[test]
fn without_an_age_on_every_entry_receipt_time_orders_them() {
    let entries = [
        entry("older", Some(5.0), Some(1_000), &["a"]),
        entry("newer", None, Some(9_000), &["a"]),
    ];
    let queues = queues_from_server(&entries, "desk", 10_000);
    let devices: Vec<&str> = queues.iter().map(|q| q.device_id.as_str()).collect();
    assert_eq!(devices, ["newer", "older"]);
    assert_eq!(queues[0].freshness, QueueFreshness::Writer);
}

#[test]
fn an_unnamed_device_is_still_offered_and_a_wild_position_is_clamped() {
    let mut nameless = entry("phone", Some(1.0), None, &["a", "b"]);
    nameless.device_name = None;
    nameless.position = 99;
    let queue = &queues_from_server(&[nameless], "desk", 0)[0];
    assert_eq!(queue.device_name, "Another device");
    assert_eq!(queue.position, 1);
}

// MARK: - Server first, then the op log

#[tokio::test]
async fn the_server_answer_wins_even_when_it_offers_nothing() {
    let store = store("desk");
    let now = now_millis();
    store
        .write_local(|tx| QueueState::new("phone", "Phone", &["a"], 0, 0, now).insert(tx))
        .unwrap();
    let server = Fixed(Ok(vec![]));
    let queues = resumable_queues(Some(&server), &store, now).await.unwrap();
    assert!(queues.is_empty());
}

#[tokio::test]
async fn the_op_log_rows_answer_when_the_server_cannot() {
    let store = store("desk");
    let now = now_millis();
    store
        .write_local(|tx| {
            QueueState::new("phone", "Phone", &["a", "b"], 1, 9, now - 5_000).insert(tx)
        })
        .unwrap();
    for source in [
        Some(Fixed(Err(SidecarError::Network("offline".into())))),
        None,
    ] {
        let queues = resumable_queues(source.as_ref(), &store, now)
            .await
            .unwrap();
        assert_eq!(queues.len(), 1);
        assert_eq!(queues[0].device_name, "Phone");
        assert_eq!(queues[0].current_track_id(), Some("b"));
        assert_eq!(queues[0].freshness, QueueFreshness::Writer);
        assert_eq!(queues[0].age_ms, 5_000);
    }
}

// MARK: - Publishing

#[tokio::test]
async fn publishing_pushes_the_queue_row_and_nothing_else() {
    let store = store("desk");
    let now = now_millis();
    store.create_playlist("Focus", None, None, now).unwrap();
    let mut broadcast = QueueBroadcast::new();
    broadcast.note(
        QueueSnapshot {
            device_name: "Desk".to_owned(),
            track_ids: vec!["a".to_owned(), "b".to_owned()],
            position: 1,
            elapsed_ms: 0,
        },
        now,
    );
    assert!(broadcast.flush_now(&store, now).unwrap());

    let server = Sidecar::new();
    let engine = SyncEngine::new(Arc::clone(&store), server.link("listener"));
    assert_eq!(engine.publish_queue().await.unwrap(), 1);
    assert_eq!(server.entities_for("listener"), ["queue_state"]);
    let left: Vec<String> = pending(&store).into_iter().map(|op| op.entity).collect();
    assert_eq!(left, ["playlist"], "the playlist waits for the full sync");
}
