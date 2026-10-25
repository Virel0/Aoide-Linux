//! The tray icon over a private `dbus-daemon`, with a stand-in for the
//! panel's `org.kde.StatusNotifierWatcher`. Linux only, and ignored by default
//! because it needs `dbus-daemon` on the PATH. To run it:
//!
//! ```text
//! cargo test -p aoide-mpris -- --include-ignored
//! ```
#![cfg(target_os = "linux")]

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aoide_mpris::{Pixmap, PlaybackStatus, Tray, TrayCommand, TrayIcon, TrayState};
use common::PrivateBus;
use futures_util::StreamExt;
use tokio::sync::mpsc::Receiver;
use zbus::fdo::PropertiesProxy;
use zbus::names::InterfaceName;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Structure, Value};
use zbus::{Connection, MatchRule, MessageStream, interface};

const ITEM: InterfaceName<'static> =
    InterfaceName::from_static_str_unchecked("org.kde.StatusNotifierItem");
const ITEM_NAME: &str = "org.kde.StatusNotifierItem";
const MENU_NAME: &str = "com.canonical.dbusmenu";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WAIT: Duration = Duration::from_secs(5);

/// `ToolTip`'s `(sa(iiay)ss)`: icon name, pixmaps, title, body.
type ToolTip = (String, Vec<(i32, i32, Vec<u8>)>, String, String);

/// The panel's side: remembers who registered.
#[derive(Clone, Default)]
struct Watcher {
    items: Arc<Mutex<Vec<String>>>,
}

#[interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    fn register_status_notifier_item(&self, service: String) {
        self.items.lock().unwrap().push(service);
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }
}

/// Starts a watcher on its own connection. Dropping the connection is the
/// panel going away.
async fn start_watcher(bus: &PrivateBus) -> (Connection, Watcher) {
    let watcher = Watcher::default();
    let connection = zbus::connection::Builder::address(bus.address())
        .unwrap()
        .serve_at("/StatusNotifierWatcher", watcher.clone())
        .unwrap()
        .name(WATCHER)
        .unwrap()
        .build()
        .await
        .unwrap();
    (connection, watcher)
}

async fn registered(watcher: &Watcher) -> String {
    tokio::time::timeout(WAIT, async {
        loop {
            if let Some(name) = watcher.items.lock().unwrap().first().cloned() {
                return name;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the item never registered")
}

fn icon() -> TrayIcon {
    TrayIcon {
        name: "io.github.Virel0.Aoide".into(),
        pixmaps: vec![Pixmap::from_rgba(1, 1, &[1, 2, 3, 4]).unwrap()],
    }
}

fn playing() -> TrayState {
    TrayState {
        status: PlaybackStatus::Playing,
        now_playing: Some("Song — Artist".into()),
        can_go_next: true,
        can_go_previous: true,
        can_play: true,
    }
}

async fn next_command(rx: &mut Receiver<TrayCommand>) -> TrayCommand {
    tokio::time::timeout(WAIT, rx.recv())
        .await
        .expect("no command arrived")
        .expect("channel closed")
}

async fn call<B>(
    client: &Connection,
    name: &str,
    path: &str,
    interface: &str,
    method: &str,
    body: &B,
) -> zbus::message::Message
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    client
        .call_method(Some(name), path, Some(interface), method, body)
        .await
        .unwrap()
}

/// `GetLayout(0, -1, [])`'s children, as (id, label or "separator", enabled).
async fn menu_labels(client: &Connection, name: &str) -> (u32, Vec<(i32, String, bool)>) {
    let reply = call(
        client,
        name,
        "/MenuBar",
        MENU_NAME,
        "GetLayout",
        &(0i32, -1i32, Vec::<String>::new()),
    )
    .await;
    type Node = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);
    let (revision, (root, _, children)): (u32, Node) = reply.body().deserialize().unwrap();
    assert_eq!(root, 0);
    let items = children
        .into_iter()
        .map(|child| {
            let structure = Structure::try_from(Value::from(child)).unwrap();
            let (id, props, _): Node = structure.try_into().unwrap();
            let text = |key: &str| props.get(key).map(|v| String::try_from(v.clone()).unwrap());
            let label = text("label").or_else(|| text("type")).unwrap_or_default();
            let enabled = props
                .get("enabled")
                .map(|v| bool::try_from(v.clone()).unwrap())
                .unwrap_or(true);
            (id, label, enabled)
        })
        .collect();
    (revision, items)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts dbus-daemon"]
async fn the_item_registers_and_describes_itself() {
    let bus = PrivateBus::start();
    let (_watcher_connection, watcher) = start_watcher(&bus).await;
    let (tray, _rx) = Tray::start(&bus.bus(), icon()).await.unwrap();
    let name = registered(&watcher).await;
    assert!(name.starts_with("org.kde.StatusNotifierItem-"), "{name}");
    assert!(tray.is_shown());

    let client = bus.client().await;
    let props = PropertiesProxy::builder(&client)
        .destination(name.as_str())
        .unwrap()
        .path("/StatusNotifierItem")
        .unwrap()
        .build()
        .await
        .unwrap();
    let text = |v: OwnedValue| String::try_from(v).unwrap();
    assert_eq!(text(props.get(ITEM, "Title").await.unwrap()), "Aoide");
    assert_eq!(text(props.get(ITEM, "Id").await.unwrap()), "aoide");
    assert_eq!(text(props.get(ITEM, "Status").await.unwrap()), "Active");
    assert_eq!(
        text(props.get(ITEM, "IconName").await.unwrap()),
        "io.github.Virel0.Aoide"
    );
    let menu = OwnedObjectPath::try_from(props.get(ITEM, "Menu").await.unwrap()).unwrap();
    assert_eq!(menu.as_str(), "/MenuBar");

    let pixmaps: Vec<(i32, i32, Vec<u8>)> = props
        .get(ITEM, "IconPixmap")
        .await
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!(pixmaps, [(1, 1, vec![4, 1, 2, 3])]);

    tray.update(playing()).await.unwrap();
    let (_, _, title, body): ToolTip = props
        .get(ITEM, "ToolTip")
        .await
        .unwrap()
        .try_into()
        .unwrap();
    assert_eq!((title.as_str(), body.as_str()), ("Aoide", "Song — Artist"));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts dbus-daemon"]
async fn the_menu_follows_the_player_and_clicks_reach_the_app() {
    let bus = PrivateBus::start();
    let (_watcher_connection, watcher) = start_watcher(&bus).await;
    let (tray, mut rx) = Tray::start(&bus.bus(), icon()).await.unwrap();
    let name = registered(&watcher).await;
    let client = bus.client().await;

    let (first_revision, idle) = menu_labels(&client, &name).await;
    let enabled: Vec<_> = idle.iter().filter(|i| i.2).map(|i| i.1.as_str()).collect();
    assert_eq!(enabled, ["separator", "Open Aoide", "separator", "Quit"]);

    // The menu announces its new layout when the player starts.
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(MENU_NAME)
        .unwrap()
        .member("LayoutUpdated")
        .unwrap()
        .build();
    let mut signals = MessageStream::for_match_rule(rule, &client, None)
        .await
        .unwrap();
    tray.update(playing()).await.unwrap();
    let signal = tokio::time::timeout(WAIT, signals.next())
        .await
        .expect("no LayoutUpdated")
        .unwrap()
        .unwrap();
    let (revision, parent): (u32, i32) = signal.body().deserialize().unwrap();
    assert!(revision > first_revision);
    assert_eq!(parent, 0);

    let (_, items) = menu_labels(&client, &name).await;
    let labels: Vec<_> = items
        .iter()
        .filter(|i| i.1 != "separator")
        .map(|i| (i.1.as_str(), i.2))
        .collect();
    assert_eq!(
        labels,
        [
            ("Pause", true),
            ("Next", true),
            ("Previous", true),
            ("Stop", true),
            ("Open Aoide", true),
            ("Quit", true),
        ]
    );

    let expected = [
        ("Pause", TrayCommand::PlayPause),
        ("Next", TrayCommand::Next),
        ("Previous", TrayCommand::Previous),
        ("Stop", TrayCommand::Stop),
        ("Open Aoide", TrayCommand::OpenWindow),
        ("Quit", TrayCommand::Quit),
    ];
    for (label, command) in expected {
        let id = items.iter().find(|i| i.1 == label).unwrap().0;
        call(
            &client,
            &name,
            "/MenuBar",
            MENU_NAME,
            "Event",
            &(id, "clicked", Value::from(0i32), 0u32),
        )
        .await;
        assert_eq!(next_command(&mut rx).await, command, "{label}");
    }

    // A left click on the icon toggles the window, a middle click plays or
    // pauses.
    call(
        &client,
        &name,
        "/StatusNotifierItem",
        ITEM_NAME,
        "Activate",
        &(0i32, 0i32),
    )
    .await;
    assert_eq!(next_command(&mut rx).await, TrayCommand::ToggleWindow);
    call(
        &client,
        &name,
        "/StatusNotifierItem",
        ITEM_NAME,
        "SecondaryActivate",
        &(0i32, 0i32),
    )
    .await;
    assert_eq!(next_command(&mut rx).await, TrayCommand::PlayPause);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "starts dbus-daemon"]
async fn a_panel_that_starts_later_gets_the_item() {
    let bus = PrivateBus::start();
    let (tray, _rx) = Tray::start(&bus.bus(), icon()).await.unwrap();
    assert!(!tray.is_shown(), "no watcher yet");

    let (watcher_connection, watcher) = start_watcher(&bus).await;
    registered(&watcher).await;
    tokio::time::timeout(WAIT, async {
        while !tray.is_shown() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the tray never said it was shown");

    // The panel restarts: the item registers with the new one.
    drop(watcher_connection);
    let (_again, second) = start_watcher(&bus).await;
    registered(&second).await;
}
