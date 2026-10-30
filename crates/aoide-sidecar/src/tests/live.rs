//! Against a real Melete, when one is named: `MELETE_TEST_URL`, `MELETE_TEST_USER`
//! and `MELETE_TEST_PASSWORD` for a throwaway server whose library has tracks 2
//! and 3 (never one with anyone's real account: this writes to its sync log).
//! Skipped, passing, when they aren't set.

use std::sync::Arc;

use aoide_store::{Store, TrackLink};
use serde_json::json;

use super::{item_ids, store, synced_rows};
use crate::{NextMode, NextRequest, SidecarClient, SyncEngine};

/// A session token from the named server, signed in as `device`.
async fn sign_in(device: &str) -> Option<(String, String)> {
    let url = std::env::var("MELETE_TEST_URL").ok()?;
    let user = std::env::var("MELETE_TEST_USER").ok()?;
    let password = std::env::var("MELETE_TEST_PASSWORD").ok()?;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let body = json!({"username": user, "password": password, "device_name": device});
    let bytes = reqwest::Client::new()
        .post(format!("{url}/api/v1/auth/login"))
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let answer: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    Some((url, answer["token"].as_str().unwrap().to_owned()))
}

fn engine(url: &str, token: &str, store: &Arc<Store>) -> SyncEngine<SidecarClient> {
    SyncEngine::new(Arc::clone(store), SidecarClient::new(url, token).unwrap())
}

#[tokio::test]
async fn a_playlist_crosses_a_real_melete_and_melete_reads_it_too() {
    let Some((url, phone_token)) = sign_in("live test phone").await else {
        eprintln!("MELETE_TEST_URL not set: skipped");
        return;
    };
    let (_, mac_token) = sign_in("live test mac").await.unwrap();
    let phone = store("live-phone");
    let mac = store("live-mac");
    let name = format!("Live test {}", aoide_store::now_millis());
    let list = phone
        .create_playlist(&name, None, None, aoide_store::now_millis())
        .unwrap();
    phone
        .add_tracks(
            &[TrackLink::new("2", "k2"), TrackLink::new("3", "k3")],
            &list.id,
            aoide_store::now_millis(),
        )
        .unwrap();

    let sent = engine(&url, &phone_token, &phone).sync().await.unwrap();
    assert!(sent.pushed >= 3, "{sent:?}");
    engine(&url, &mac_token, &mac).sync().await.unwrap();
    assert_eq!(item_ids(&mac, &list.id), ["2", "3"]);
    let mine = |rows: String| -> Vec<String> {
        rows.lines()
            .filter(|row| row.contains(&list.id))
            .map(str::to_owned)
            .collect()
    };
    assert_eq!(mine(synced_rows(&mac)), mine(synced_rows(&phone)));

    // Melete works the playlist out of the same log for its own API.
    let bytes = reqwest::Client::new()
        .get(format!("{url}/api/v1/playlists"))
        .bearer_auth(&phone_token)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let lists: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let found = lists
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == list.id.as_str())
        .expect("Melete lists the pushed playlist");
    assert_eq!(found["name"], name.as_str());
    assert_eq!(found["tracks"], 2);

    // And the routes Infinity and Discover ask.
    let client = SidecarClient::new(&url, &phone_token).unwrap();
    client.status().await.unwrap();
    client
        .next(&NextRequest::new("2", &[], &[], NextMode::Infinity, 5))
        .await
        .unwrap();
    client.discover(10).await.unwrap();
}
