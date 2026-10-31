//! Against a real Melete, when one is named: `MELETE_TEST_URL`, `MELETE_TEST_USER`
//! and `MELETE_TEST_PASSWORD` for a throwaway server with a music library (never
//! one with anyone's real account). Skipped, passing, when they aren't set.
//!
//! ```sh
//! MELETE_TEST_URL=http://127.0.0.1:8100 MELETE_TEST_USER=… MELETE_TEST_PASSWORD=… \
//!     cargo test -p aoide-melete --test live -- --nocapture
//! ```
//!
//! For a test user with two-factor sign-in on, give the code its authenticator
//! shows now as `MELETE_TEST_CODE`: only the two-factor test runs then, since the
//! others sign in with the password alone.

use aoide_melete::{
    Device, Error, ListQuery, ListenState, MeleteClient, ServerAddress, StreamFormat, is_stale,
};

fn server() -> Option<(MeleteClient, String, String)> {
    if std::env::var("MELETE_TEST_CODE").is_ok() {
        return None;
    }
    password_server()
}

fn password_server() -> Option<(MeleteClient, String, String)> {
    let url = std::env::var("MELETE_TEST_URL").ok()?;
    let user = std::env::var("MELETE_TEST_USER").ok()?;
    let password = std::env::var("MELETE_TEST_PASSWORD").ok()?;
    let client = MeleteClient::new(
        ServerAddress::parse(&url).unwrap(),
        Device::new("aoide-melete live test", "live-test"),
    )
    .unwrap();
    Some((client, user, password))
}

#[tokio::test]
async fn signs_in_lists_plays_and_signs_out() {
    let Some((client, user, password)) = server() else {
        eprintln!("MELETE_TEST_URL not set: skipped");
        return;
    };
    client.server_info().await.unwrap();
    let client = client
        .clone()
        .with_credentials(client.sign_in(&user, &password).await.unwrap());

    let albums = client.albums(&ListQuery::default()).await.unwrap();
    assert!(albums.total > 0, "the test server needs a music library");
    let album = &albums.items[0];
    let tracks = client.album_tracks(&album.id).await.unwrap();
    assert!(!tracks.is_empty());

    let found = client.items_by_ids(&[tracks[0].id.as_str()]).await.unwrap();
    assert_eq!(found[0].id, tracks[0].id);
    client.item_details(&tracks[0].id).await.unwrap();
    client.lyrics(&tracks[0].id).await.unwrap();
    client.genres().await.unwrap();
    client.home().await.unwrap();
    client.search(&album.name, 5).await.unwrap();

    let stream = client
        .play(&tracks[0].id, StreamFormat::Original)
        .await
        .unwrap();
    let response = reqwest::get(stream.url).await.unwrap();
    assert!(response.status().is_success(), "{}", response.status());

    let cover = reqwest::Client::new()
        .get(client.cover_url(&album.id))
        .header("authorization", client.authorization().unwrap())
        .send()
        .await
        .unwrap();
    assert!(
        cover.status().is_success() || cover.status() == 404,
        "{}",
        cover.status()
    );

    client.sign_out().await.unwrap();
    assert!(client.genres().await.is_err(), "the token is revoked");
}

/// Two devices of one listener: one starts, the other joins by the code, a
/// change against an old version is refused, and leaving ends it.
#[tokio::test]
async fn two_devices_listen_together() {
    let Some((client, user, password)) = server() else {
        eprintln!("MELETE_TEST_URL not set: skipped");
        return;
    };
    let desk = client
        .clone()
        .with_credentials(client.sign_in(&user, &password).await.unwrap());
    let phone = client
        .clone()
        .with_credentials(client.sign_in(&user, &password).await.unwrap());
    let albums = desk.albums(&ListQuery::default()).await.unwrap();
    let tracks = desk.album_tracks(&albums.items[0].id).await.unwrap();
    let queue: Vec<i64> = tracks.iter().map(|t| t.id.parse().unwrap()).collect();

    let state = ListenState {
        queue: queue.clone(),
        index: 0,
        position_ms: 0,
        playing: true,
    };
    let started = desk.start_listening(&state).await.unwrap().session;
    let joined = phone
        .join_listening(&started.code.to_lowercase())
        .await
        .unwrap()
        .session;
    assert_eq!((joined.id, &joined.queue), (started.id, &queue));
    assert_eq!(joined.members.len(), 2);

    let paused = ListenState {
        playing: false,
        position_ms: 5_000,
        ..state
    };
    let changed = phone
        .change_listening(joined.id, joined.version, &paused)
        .await
        .unwrap()
        .session;
    assert!(!changed.playing && changed.version > joined.version);
    let stale = desk
        .change_listening(started.id, started.version, &paused)
        .await
        .unwrap_err();
    assert!(is_stale(&stale), "{stale:?}");
    let seen = desk.listening(started.id).await.unwrap();
    assert_eq!(seen.session.position_at(seen.server_time), 5_000);

    phone.leave_listening(joined.id).await.unwrap();
    desk.leave_listening(started.id).await.unwrap();
    assert!(desk.listening_sessions().await.unwrap().is_empty());
    desk.sign_out().await.unwrap();
    phone.sign_out().await.unwrap();
}

/// A user with two-factor sign-in on: the password gets a challenge, and the
/// code finishes it.
#[tokio::test]
async fn two_factor_sign_in_finishes_with_the_code() {
    let (Some((client, user, password)), Ok(code)) =
        (password_server(), std::env::var("MELETE_TEST_CODE"))
    else {
        eprintln!("MELETE_TEST_CODE not set: skipped");
        return;
    };
    let Err(Error::SecondFactorNeeded { challenge }) = client.sign_in(&user, &password).await
    else {
        panic!("the test user should have two-factor sign-in on");
    };
    assert_eq!(
        client.finish_sign_in(&challenge, "000000").await,
        Err(Error::Unauthorized),
        "a wrong code leaves the challenge"
    );
    let signed = client
        .clone()
        .with_credentials(client.finish_sign_in(&challenge, &code).await.unwrap());
    signed.genres().await.unwrap();
    signed.sign_out().await.unwrap();
}
