//! The presence task against a stand-in Discord on a Unix socket in a
//! temporary directory. No network, no real Discord.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};
use tokio::net::{UnixListener, UnixStream};
use tokio::time::timeout;

use super::ipc::{CLOSE, FRAME, HANDSHAKE, PING, PONG, read_frame, write_frame};
use super::*;

const WAIT: Duration = Duration::from_secs(5);
const QUIET: Duration = Duration::from_millis(400);

const FAST: Timing = Timing {
    retry: Duration::from_millis(100),
    min_interval: Duration::from_millis(300),
};

struct FakeDiscord {
    listener: UnixListener,
    path: PathBuf,
    _dir: tempfile::TempDir,
}

impl FakeDiscord {
    fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("discord-ipc-0");
        Self {
            listener: UnixListener::bind(&path).unwrap(),
            path,
            _dir: dir,
        }
    }

    fn endpoint(&self) -> Endpoint {
        Endpoint::Paths(vec![self.path.clone()])
    }

    /// Accepts a client and answers its handshake.
    async fn accept(&self) -> Client {
        let (mut stream, _) = timeout(WAIT, self.listener.accept())
            .await
            .expect("nobody connected")
            .unwrap();
        let (opcode, handshake) = read_frame(&mut stream).await.unwrap();
        assert_eq!(opcode, HANDSHAKE);
        let ready = json!({ "cmd": "DISPATCH", "evt": "READY", "data": { "v": 1 } });
        write_frame(&mut stream, FRAME, &ready).await.unwrap();
        Client { stream, handshake }
    }

    async fn nobody_connects(&self) -> bool {
        timeout(QUIET, self.listener.accept()).await.is_err()
    }
}

struct Client {
    stream: UnixStream,
    handshake: Value,
}

impl Client {
    /// The next command, answered as Discord would.
    async fn command(&mut self) -> Value {
        let (opcode, command) = timeout(WAIT, read_frame(&mut self.stream))
            .await
            .expect("no command came")
            .unwrap();
        assert_eq!(opcode, FRAME);
        let reply =
            json!({ "cmd": command["cmd"], "nonce": command["nonce"], "evt": null, "data": {} });
        // The client may already have hung up (a last clear before closing).
        let _ = write_frame(&mut self.stream, FRAME, &reply).await;
        command
    }

    async fn silent(&mut self) -> bool {
        timeout(QUIET, read_frame(&mut self.stream)).await.is_err()
    }
}

fn on() -> PresenceOptions {
    PresenceOptions {
        enabled: true,
        ..PresenceOptions::default()
    }
}

fn song(title: &str) -> NowPlaying {
    NowPlaying {
        track: Track {
            title: title.into(),
            artists: vec!["Artist".into()],
            album: Some("Album".into()),
            length: Some(Duration::from_secs(180)),
            ..Track::default()
        },
        playing: true,
        position: Duration::ZERO,
        at: SystemTime::now(),
    }
}

async fn wait_for(presence: &Presence, wanted: PresenceStatus) {
    let mut status = presence.subscribe_status();
    timeout(WAIT, status.wait_for(|s| *s == wanted))
        .await
        .unwrap_or_else(|_| panic!("status never became {wanted:?}"))
        .unwrap();
}

#[tokio::test]
async fn disabled_connects_to_nothing() {
    let discord = FakeDiscord::start();
    let presence = Presence::spawn_with(discord.endpoint(), PresenceOptions::default(), FAST);
    presence.set_now_playing(Some(song("Song")));
    assert!(discord.nobody_connects().await);
    assert_eq!(presence.status(), PresenceStatus::Off);
}

#[tokio::test]
async fn the_handshake_names_aoide_and_the_song_goes_as_listening() {
    let discord = FakeDiscord::start();
    let presence = Presence::spawn_with(discord.endpoint(), on(), FAST);
    let mut client = discord.accept().await;
    assert_eq!(
        client.handshake,
        json!({ "v": 1, "client_id": "1547687515279069215" })
    );
    wait_for(&presence, PresenceStatus::Connected).await;

    presence.set_now_playing(Some(song("Song")));
    let command = client.command().await;
    assert_eq!(command["cmd"], "SET_ACTIVITY");
    assert_eq!(command["args"]["pid"], std::process::id());
    let activity = &command["args"]["activity"];
    assert_eq!(activity["type"], 2);
    assert_eq!(activity["details"], "Song");
    assert_eq!(activity["state"], "Artist");
    assert!(
        activity["timestamps"]["end"].as_u64().unwrap()
            > activity["timestamps"]["start"].as_u64().unwrap()
    );
}

#[tokio::test]
async fn nothing_is_sent_until_something_plays() {
    let discord = FakeDiscord::start();
    let _presence = Presence::spawn_with(discord.endpoint(), on(), FAST);
    let mut client = discord.accept().await;
    assert!(client.silent().await);
}

#[tokio::test]
async fn a_burst_of_changes_sends_at_most_one_a_second() {
    let discord = FakeDiscord::start();
    let presence = Presence::spawn_with(discord.endpoint(), on(), TIMING);
    let mut client = discord.accept().await;
    wait_for(&presence, PresenceStatus::Connected).await;

    presence.set_now_playing(Some(song("First")));
    let first = client.command().await;
    let sent_first = tokio::time::Instant::now();
    assert_eq!(first["args"]["activity"]["details"], "First");

    for title in ["Two", "Three", "Four", "Last"] {
        presence.set_now_playing(Some(song(title)));
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let second = client.command().await;
    assert!(
        sent_first.elapsed() >= Duration::from_millis(900),
        "sent after {:?}",
        sent_first.elapsed()
    );
    assert_eq!(second["args"]["activity"]["details"], "Last");
    assert!(client.silent().await, "the burst sent more than one");
}

#[tokio::test]
async fn the_same_song_a_moment_later_is_not_sent_again() {
    let discord = FakeDiscord::start();
    let presence = Presence::spawn_with(discord.endpoint(), on(), FAST);
    let mut client = discord.accept().await;
    let started = song("Song");
    presence.set_now_playing(Some(started.clone()));
    client.command().await;

    // Half a second on, read half a second later: the bar is where it was.
    let mut later = started;
    later.at += Duration::from_millis(500);
    later.position += Duration::from_millis(500);
    presence.set_now_playing(Some(later));
    assert!(client.silent().await);
}

#[tokio::test]
async fn pausing_with_show_when_paused_off_clears() {
    let discord = FakeDiscord::start();
    let options = PresenceOptions {
        show_when_paused: false,
        ..on()
    };
    let presence = Presence::spawn_with(discord.endpoint(), options, FAST);
    let mut client = discord.accept().await;
    presence.set_now_playing(Some(song("Song")));
    client.command().await;

    let mut paused = song("Song");
    paused.playing = false;
    presence.set_now_playing(Some(paused));
    let command = client.command().await;
    assert_eq!(command["cmd"], "SET_ACTIVITY");
    assert!(command["args"].get("activity").is_none());
}

#[tokio::test]
async fn turning_it_off_clears_and_disconnects() {
    let discord = FakeDiscord::start();
    let presence = Presence::spawn_with(discord.endpoint(), on(), FAST);
    let mut client = discord.accept().await;
    presence.set_now_playing(Some(song("Song")));
    client.command().await;

    presence.set_options(PresenceOptions::default());
    let command = client.command().await;
    assert!(command["args"].get("activity").is_none());
    let closed = timeout(WAIT, read_frame(&mut client.stream)).await.unwrap();
    assert!(closed.is_err(), "the socket stayed open");
    wait_for(&presence, PresenceStatus::Off).await;
    assert!(discord.nobody_connects().await);
}

#[tokio::test]
async fn a_discord_started_later_is_found() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("discord-ipc-0");
    let presence = Presence::spawn_with(Endpoint::Paths(vec![path.clone()]), on(), FAST);
    presence.set_now_playing(Some(song("Song")));
    wait_for(&presence, PresenceStatus::Searching).await;
    tokio::time::sleep(Duration::from_millis(250)).await;

    let discord = FakeDiscord {
        listener: UnixListener::bind(&path).unwrap(),
        path,
        _dir: dir,
    };
    let mut client = discord.accept().await;
    let command = client.command().await;
    assert_eq!(command["args"]["activity"]["details"], "Song");
}

#[tokio::test]
async fn after_discord_quits_it_reconnects_and_says_it_again() {
    let discord = FakeDiscord::start();
    let presence = Presence::spawn_with(discord.endpoint(), on(), FAST);
    let mut client = discord.accept().await;
    presence.set_now_playing(Some(song("Song")));
    client.command().await;

    write_frame(
        &mut client.stream,
        CLOSE,
        &json!({ "code": 1000, "message": "bye" }),
    )
    .await
    .unwrap();
    drop(client);

    let mut again = discord.accept().await;
    let command = again.command().await;
    assert_eq!(command["args"]["activity"]["details"], "Song");
}

#[tokio::test]
async fn pings_are_answered() {
    let discord = FakeDiscord::start();
    let _presence = Presence::spawn_with(discord.endpoint(), on(), FAST);
    let mut client = discord.accept().await;
    let ping = json!({ "nonce": "p1" });
    write_frame(&mut client.stream, PING, &ping).await.unwrap();
    let (opcode, payload) = timeout(WAIT, read_frame(&mut client.stream))
        .await
        .unwrap()
        .unwrap();
    assert_eq!((opcode, payload), (PONG, ping));
}

#[tokio::test]
async fn a_socket_that_is_not_discord_is_passed_over() {
    // The first candidate accepts but never answers the handshake properly.
    let impostor = FakeDiscord::start();
    let discord = FakeDiscord::start();
    let endpoint = Endpoint::Paths(vec![impostor.path.clone(), discord.path.clone()]);
    let _presence = Presence::spawn_with(endpoint, on(), FAST);
    let (mut stream, _) = timeout(WAIT, impostor.listener.accept())
        .await
        .unwrap()
        .unwrap();
    read_frame(&mut stream).await.unwrap();
    write_frame(&mut stream, FRAME, &json!({ "evt": "ERROR" }))
        .await
        .unwrap();
    discord.accept().await;
}
