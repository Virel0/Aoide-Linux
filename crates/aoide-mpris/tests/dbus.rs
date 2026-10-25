//! Drives the real interfaces over a private `dbus-daemon`, with zbus as the
//! client. Linux only, and ignored by default because it needs `dbus-daemon`
//! on the PATH (package `dbus` on Ubuntu and Arch). To run them:
//!
//! ```text
//! cargo test -p aoide-mpris -- --include-ignored
//! ```
//!
//! Each test starts its own bus from a minimal config with no service
//! directories, so nothing on the machine (a real portal, a real screensaver)
//! is activated into it.
#![cfg(target_os = "linux")]

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aoide_mpris::{
    Command, Error, Inhibitor, Instance, LoopStatus, MPRIS_BUS_NAME, Metadata, Mpris,
    PlaybackStatus, PlayerState, single_instance, track_object_path,
};
use common::PrivateBus;
use futures_util::StreamExt;
use tokio::sync::mpsc::Receiver;
use zbus::fdo::PropertiesProxy;
use zbus::message::Type as MessageType;
use zbus::names::InterfaceName;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, MatchRule, MessageStream};

const PATH: &str = "/org/mpris/MediaPlayer2";
const ROOT: InterfaceName<'static> =
    InterfaceName::from_static_str_unchecked("org.mpris.MediaPlayer2");
const PLAYER: InterfaceName<'static> =
    InterfaceName::from_static_str_unchecked("org.mpris.MediaPlayer2.Player");
const ROOT_NAME: &str = "org.mpris.MediaPlayer2";
const PLAYER_NAME: &str = "org.mpris.MediaPlayer2.Player";
const WAIT: Duration = Duration::from_secs(5);
/// How long to listen to be sure a signal does not come.
const QUIET: Duration = Duration::from_millis(400);

async fn properties(client: &Connection) -> PropertiesProxy<'static> {
    PropertiesProxy::builder(client)
        .destination(MPRIS_BUS_NAME)
        .unwrap()
        .path(PATH)
        .unwrap()
        .build()
        .await
        .unwrap()
}

async fn call<B>(client: &Connection, interface: &str, method: &str, body: &B) -> zbus::Result<()>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    client
        .call_method(Some(MPRIS_BUS_NAME), PATH, Some(interface), method, body)
        .await
        .map(|_| ())
}

async fn next_command(rx: &mut Receiver<Command>) -> Command {
    tokio::time::timeout(WAIT, rx.recv())
        .await
        .expect("no command arrived")
        .expect("channel closed")
}

fn track(id: &str) -> Metadata {
    Metadata {
        jellyfin_id: id.into(),
        title: format!("Track {id}"),
        artists: vec!["Artist".into()],
        album: Some("Album".into()),
        album_artists: vec!["Artist".into()],
        length: Some(Duration::from_secs(240)),
        art_url: Some(format!(
            "https://jellyfin.example.org/Items/{id}/Images/Primary"
        )),
        track_number: Some(3),
    }
}

fn playing(id: &str, position: Duration) -> PlayerState {
    PlayerState {
        status: PlaybackStatus::Playing,
        metadata: Some(track(id)),
        position,
        can_go_next: true,
        can_go_previous: true,
        can_play: true,
        can_pause: true,
        can_seek: true,
        ..PlayerState::default()
    }
}

fn string(value: OwnedValue) -> String {
    String::try_from(value).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts dbus-daemon"]
async fn the_player_identifies_itself() {
    let bus = PrivateBus::start();
    let (_mpris, _rx) = Mpris::start(&bus.bus()).await.unwrap();
    let client = bus.client().await;
    let props = properties(&client).await;

    assert_eq!(string(props.get(ROOT, "Identity").await.unwrap()), "Aoide");
    assert_eq!(
        string(props.get(ROOT, "DesktopEntry").await.unwrap()),
        "io.github.Virel0.Aoide"
    );
    assert!(bool::try_from(props.get(ROOT, "CanRaise").await.unwrap()).unwrap());
    assert_eq!(
        string(props.get(PLAYER, "PlaybackStatus").await.unwrap()),
        "Stopped"
    );
    assert_eq!(
        f64::try_from(props.get(PLAYER, "Rate").await.unwrap()).unwrap(),
        1.0
    );
    let metadata: HashMap<String, OwnedValue> = props
        .get(PLAYER, "Metadata")
        .await
        .unwrap()
        .try_into()
        .unwrap();
    assert!(metadata.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts dbus-daemon"]
async fn a_second_server_on_the_same_bus_is_refused() {
    let bus = PrivateBus::start();
    let _first = Mpris::start(&bus.bus()).await.unwrap();
    assert!(matches!(
        Mpris::start(&bus.bus()).await,
        Err(Error::NameTaken(_))
    ));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts dbus-daemon"]
async fn updates_announce_only_what_changed() {
    let bus = PrivateBus::start();
    let (mpris, _rx) = Mpris::start(&bus.bus()).await.unwrap();
    let client = bus.client().await;
    let props = properties(&client).await;
    let mut changes = props.receive_properties_changed().await.unwrap();

    let state = playing("a", Duration::ZERO);
    mpris.update(state.clone()).await.unwrap();
    let signal = tokio::time::timeout(WAIT, changes.next())
        .await
        .unwrap()
        .unwrap();
    let args = signal.args().unwrap();
    assert_eq!(args.interface_name.as_str(), PLAYER_NAME);
    let mut keys: Vec<_> = args.changed_properties.keys().copied().collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "CanGoNext",
            "CanGoPrevious",
            "CanPause",
            "CanPlay",
            "CanSeek",
            "Metadata",
            "PlaybackStatus"
        ]
    );

    // Only the volume changes.
    mpris
        .update(PlayerState {
            volume: 0.5,
            ..state.clone()
        })
        .await
        .unwrap();
    let signal = tokio::time::timeout(WAIT, changes.next())
        .await
        .unwrap()
        .unwrap();
    let args = signal.args().unwrap();
    let keys: Vec<_> = args.changed_properties.keys().copied().collect();
    assert_eq!(keys, ["Volume"]);
    assert_eq!(
        args.changed_properties["Volume"],
        Value::from(0.5),
        "the new value travels with the signal"
    );

    // Nothing changes: no signal at all.
    mpris
        .update(PlayerState {
            volume: 0.5,
            ..state
        })
        .await
        .unwrap();
    assert!(tokio::time::timeout(QUIET, changes.next()).await.is_err());

    let metadata: HashMap<String, OwnedValue> = props
        .get(PLAYER, "Metadata")
        .await
        .unwrap()
        .try_into()
        .unwrap();
    let track_id =
        OwnedObjectPath::try_from(metadata["mpris:trackid"].try_clone().unwrap()).unwrap();
    assert_eq!(track_id.as_str(), track_object_path("a"));
    assert_eq!(
        string(metadata["xesam:title"].try_clone().unwrap()),
        "Track a"
    );
    assert_eq!(
        i64::try_from(metadata["mpris:length"].try_clone().unwrap()).unwrap(),
        240_000_000
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts dbus-daemon"]
async fn seeked_comes_on_a_jump_not_on_ticks() {
    let bus = PrivateBus::start();
    let (mpris, _rx) = Mpris::start(&bus.bus()).await.unwrap();
    let client = bus.client().await;
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface(PLAYER_NAME)
        .unwrap()
        .member("Seeked")
        .unwrap()
        .build();
    let mut seeked = MessageStream::for_match_rule(rule, &client, None)
        .await
        .unwrap();

    mpris.update(playing("a", Duration::ZERO)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    mpris
        .update(playing("a", Duration::from_millis(300)))
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(QUIET, seeked.next()).await.is_err(),
        "steady playback must not emit Seeked"
    );

    mpris
        .update(playing("a", Duration::from_secs(100)))
        .await
        .unwrap();
    let message = tokio::time::timeout(WAIT, seeked.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(message.body().deserialize::<i64>().unwrap(), 100_000_000);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts dbus-daemon"]
async fn position_is_interpolated_between_updates() {
    let bus = PrivateBus::start();
    let (mpris, _rx) = Mpris::start(&bus.bus()).await.unwrap();
    let client = bus.client().await;
    let props = properties(&client).await;

    mpris
        .update(playing("a", Duration::from_secs(10)))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let position = i64::try_from(props.get(PLAYER, "Position").await.unwrap()).unwrap();
    assert!(
        (10_400_000..12_000_000).contains(&position),
        "position {position}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts dbus-daemon"]
async fn requests_reach_the_app() {
    let bus = PrivateBus::start();
    let (mpris, mut rx) = Mpris::start(&bus.bus()).await.unwrap();
    mpris
        .update(playing("a", Duration::from_secs(10)))
        .await
        .unwrap();
    let client = bus.client().await;
    let props = properties(&client).await;
    let player = PLAYER_NAME;

    call(&client, player, "PlayPause", &()).await.unwrap();
    assert_eq!(next_command(&mut rx).await, Command::PlayPause);
    call(&client, player, "Next", &()).await.unwrap();
    assert_eq!(next_command(&mut rx).await, Command::Next);
    call(&client, player, "Previous", &()).await.unwrap();
    assert_eq!(next_command(&mut rx).await, Command::Previous);
    call(&client, player, "Seek", &(-5_000_000i64,))
        .await
        .unwrap();
    assert_eq!(next_command(&mut rx).await, Command::Seek(-5_000_000));

    let path = ObjectPath::try_from(track_object_path("a")).unwrap();
    call(&client, player, "SetPosition", &(&path, 30_000_000i64))
        .await
        .unwrap();
    assert_eq!(
        next_command(&mut rx).await,
        Command::SetPosition("a".into(), Duration::from_secs(30))
    );

    // A stale track id is ignored, as the spec says; the next request is
    // the next thing the app sees.
    let stale = ObjectPath::try_from(track_object_path("b")).unwrap();
    call(&client, player, "SetPosition", &(&stale, 30_000_000i64))
        .await
        .unwrap();
    call(&client, player, "Stop", &()).await.unwrap();
    assert_eq!(next_command(&mut rx).await, Command::Stop);

    props.set(PLAYER, "Volume", Value::from(2.0)).await.unwrap();
    assert_eq!(next_command(&mut rx).await, Command::SetVolume(1.0));
    props
        .set(PLAYER, "Shuffle", Value::from(true))
        .await
        .unwrap();
    assert_eq!(next_command(&mut rx).await, Command::SetShuffle(true));
    props
        .set(PLAYER, "LoopStatus", Value::from("Track"))
        .await
        .unwrap();
    assert_eq!(
        next_command(&mut rx).await,
        Command::SetLoop(LoopStatus::Track)
    );
    assert!(
        props
            .set(PLAYER, "LoopStatus", Value::from("Sometimes"))
            .await
            .is_err()
    );

    call(&client, ROOT_NAME, "Raise", &()).await.unwrap();
    assert_eq!(next_command(&mut rx).await, Command::Raise);
    call(&client, ROOT_NAME, "Quit", &()).await.unwrap();
    assert_eq!(next_command(&mut rx).await, Command::Quit);
    assert!(rx.try_recv().is_err());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts dbus-daemon"]
async fn requests_the_state_forbids_are_dropped() {
    let bus = PrivateBus::start();
    // Nothing loaded, nothing allowed.
    let (_mpris, mut rx) = Mpris::start(&bus.bus()).await.unwrap();
    let client = bus.client().await;
    let player = PLAYER_NAME;

    call(&client, player, "Next", &()).await.unwrap();
    call(&client, player, "Play", &()).await.unwrap();
    call(&client, player, "Seek", &(1_000_000i64,))
        .await
        .unwrap();
    assert!(call(&client, player, "PlayPause", &()).await.is_err());
    assert!(
        call(&client, player, "OpenUri", &("file:///a.flac",))
            .await
            .is_err()
    );
    call(&client, player, "Stop", &()).await.unwrap();
    assert_eq!(next_command(&mut rx).await, Command::Stop);
    assert!(rx.try_recv().is_err());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts dbus-daemon"]
async fn a_second_launch_raises_the_first() {
    let bus = PrivateBus::start();
    let Instance::First { guard, mut raise } = single_instance(&bus.bus()).await.unwrap() else {
        panic!("the first launch must be first");
    };
    let second = single_instance(&bus.bus()).await.unwrap();
    assert!(matches!(second, Instance::Other { raised: true }));
    assert_eq!(
        tokio::time::timeout(WAIT, raise.recv()).await.unwrap(),
        Some(())
    );

    // Once the first is gone, the next launch is first again.
    drop(guard);
    let mut became_first = false;
    for _ in 0..20 {
        if let Instance::First { .. } = single_instance(&bus.bus()).await.unwrap() {
            became_first = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(became_first);
}

/// Stands in for KDE's or GNOME's screensaver service.
struct FakeScreenSaver {
    calls: Arc<Mutex<Vec<String>>>,
}

#[zbus::interface(name = "org.freedesktop.ScreenSaver")]
impl FakeScreenSaver {
    fn inhibit(&self, application_name: String, _reason: String) -> u32 {
        self.calls
            .lock()
            .unwrap()
            .push(format!("Inhibit {application_name}"));
        7
    }

    #[zbus(name = "UnInhibit")]
    fn un_inhibit(&self, cookie: u32) {
        self.calls
            .lock()
            .unwrap()
            .push(format!("UnInhibit {cookie}"));
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts dbus-daemon"]
async fn the_inhibitor_falls_back_to_the_screensaver() {
    let bus = PrivateBus::start();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let _screensaver = zbus::connection::Builder::address(bus.address())
        .unwrap()
        .serve_at(
            "/org/freedesktop/ScreenSaver",
            FakeScreenSaver {
                calls: calls.clone(),
            },
        )
        .unwrap()
        .name("org.freedesktop.ScreenSaver")
        .unwrap()
        .build()
        .await
        .unwrap();

    let mut inhibitor = Inhibitor::new(&bus.bus()).await.unwrap();
    inhibitor.set(true).await.unwrap();
    inhibitor.set(true).await.unwrap();
    inhibitor.set(false).await.unwrap();
    inhibitor.set(false).await.unwrap();
    assert_eq!(*calls.lock().unwrap(), ["Inhibit Aoide", "UnInhibit 7"]);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts dbus-daemon"]
async fn no_inhibitor_on_the_bus_is_an_error() {
    let bus = PrivateBus::start();
    let mut inhibitor = Inhibitor::new(&bus.bus()).await.unwrap();
    assert!(matches!(inhibitor.set(true).await, Err(Error::NoInhibitor)));
    // Releasing when nothing is held is fine.
    inhibitor.set(false).await.unwrap();
}
