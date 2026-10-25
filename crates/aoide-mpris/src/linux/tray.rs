//! The tray icon: `org.kde.StatusNotifierItem` at `/StatusNotifierItem` and
//! its menu, `com.canonical.dbusmenu` at `/MenuBar`, on zbus.
//!
//! Written on zbus directly rather than with the `ksni` crate (maintained,
//! Unlicense): ksni always connects to the session bus, so it can neither be
//! pointed at a private bus in tests nor share [`Bus`] with the rest of this
//! crate, and the menu here is six fixed items, which is a small part of
//! DBusMenu to implement.
//!
//! The item registers with `org.kde.StatusNotifierWatcher`, and again each
//! time a watcher appears on the bus (a panel that starts after Aoide, or
//! restarts), so the icon comes back without the app doing anything.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use futures_util::StreamExt;
use tokio::sync::mpsc::{self, Receiver, Sender, error::TrySendError};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use zbus::fdo::{self, DBusProxy, RequestNameFlags};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Structure, Value};
use zbus::{Connection, interface};

use super::builder;
use crate::tray::{
    ItemKind, MenuItem, ROOT, TOOLTIP_TITLE, TrayChanges, TrayCommand, TrayIcon, TrayState,
    command_for_event, menu, tooltip_body, tray_changes,
};
use crate::{Bus, Result};

pub(crate) const ITEM_PATH: &str = "/StatusNotifierItem";
pub(crate) const MENU_PATH: &str = "/MenuBar";
const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const WATCHER_INTERFACE: &str = "org.kde.StatusNotifierWatcher";
/// The item's `Id`: stable across launches, as the spec asks.
const ITEM_ID: &str = "aoide";

/// Clicks waiting for the app; as with MPRIS, a flood gets an error.
const COMMAND_QUEUE: usize = 32;

/// `IconPixmap`'s `a(iiay)`.
type Pixmaps = Vec<(i32, i32, Vec<u8>)>;

/// `(ia{sv}av)`: one menu node.
type Node = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

#[derive(Debug)]
struct Inner {
    state: TrayState,
    /// The menu layout's revision, raised on every change.
    revision: u32,
}

type Shared = Arc<Mutex<Inner>>;

fn lock(shared: &Shared) -> MutexGuard<'_, Inner> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The tray icon. Dropping it removes the icon.
#[derive(Debug)]
pub struct Tray {
    connection: Connection,
    shared: Shared,
    registered: watch::Receiver<bool>,
    watcher_task: JoinHandle<()>,
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.watcher_task.abort();
    }
}

impl Tray {
    /// Connects to `bus`, serves the item and its menu under a name of its
    /// own (`org.kde.StatusNotifierItem-<pid>-<n>`) and registers with the
    /// panel's watcher. Clicks arrive on the receiver.
    ///
    /// No watcher on the bus is not an error: the item waits for one, and
    /// [`Tray::is_shown`] says whether a panel has it. Must be awaited inside
    /// a tokio runtime.
    pub async fn start(bus: &Bus, icon: TrayIcon) -> Result<(Tray, Receiver<TrayCommand>)> {
        let shared: Shared = Arc::new(Mutex::new(Inner {
            state: TrayState::default(),
            revision: 1,
        }));
        let (tx, rx) = mpsc::channel(COMMAND_QUEUE);
        let requests = Requests {
            tx,
            shared: shared.clone(),
        };
        let connection = builder(bus)?
            .serve_at(
                ITEM_PATH,
                Item {
                    requests: requests.clone(),
                    icon_name: icon.name,
                    pixmaps: icon
                        .pixmaps
                        .into_iter()
                        .map(|p| (p.width, p.height, p.argb))
                        .collect(),
                },
            )?
            .serve_at(MENU_PATH, Menu { requests })?
            .build()
            .await?;

        static INSTANCE: AtomicUsize = AtomicUsize::new(1);
        let name = format!(
            "org.kde.StatusNotifierItem-{}-{}",
            std::process::id(),
            INSTANCE.fetch_add(1, Ordering::Relaxed)
        );
        connection
            .request_name_with_flags(name.as_str(), RequestNameFlags::DoNotQueue.into())
            .await?;

        // Listen for watchers before trying the first, so one that appears
        // in between is not missed.
        let dbus = DBusProxy::new(&connection).await?;
        let mut owners = dbus
            .receive_name_owner_changed_with_args(&[(0, WATCHER_NAME)])
            .await?;
        let (registered_tx, registered) = watch::channel(register(&connection, &name).await);
        let task_connection = connection.clone();
        let watcher_task = tokio::spawn(async move {
            while let Some(signal) = owners.next().await {
                let Ok(args) = signal.args() else { continue };
                let shown = match args.new_owner().as_ref() {
                    Some(_) => register(&task_connection, &name).await,
                    None => false,
                };
                registered_tx.send_replace(shown);
            }
        });

        Ok((
            Tray {
                connection,
                shared,
                registered,
                watcher_task,
            },
            rx,
        ))
    }

    /// Whether a panel's watcher accepted the item. False means no tray will
    /// show it (no watcher, or it refused), so the app should not hide its
    /// window to the tray.
    pub fn is_shown(&self) -> bool {
        *self.registered.borrow()
    }

    /// Shows a new state. Announces the tooltip and the menu only if they
    /// changed, so it is cheap to call on every player change.
    pub async fn update(&self, state: TrayState) -> Result<()> {
        let (changes, revision) = {
            let mut inner = lock(&self.shared);
            let changes = tray_changes(&inner.state, &state);
            inner.state = state;
            if changes.menu {
                inner.revision = inner.revision.wrapping_add(1);
            }
            (changes, inner.revision)
        };
        let TrayChanges { tooltip, menu } = changes;
        if tooltip {
            let emitter = SignalEmitter::new(&self.connection, ITEM_PATH)?;
            Item::new_tool_tip(&emitter).await?;
        }
        if menu {
            let emitter = SignalEmitter::new(&self.connection, MENU_PATH)?;
            Menu::layout_updated(&emitter, revision, ROOT).await?;
        }
        Ok(())
    }
}

/// Registers `name` with the watcher; true if it accepted.
async fn register(connection: &Connection, name: &str) -> bool {
    connection
        .call_method(
            Some(WATCHER_NAME),
            WATCHER_PATH,
            Some(WATCHER_INTERFACE),
            "RegisterStatusNotifierItem",
            &(name,),
        )
        .await
        .is_ok()
}

#[derive(Debug, Clone)]
struct Requests {
    tx: Sender<TrayCommand>,
    shared: Shared,
}

impl Requests {
    fn send(&self, command: TrayCommand) -> fdo::Result<()> {
        match self.tx.try_send(command) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(fdo::Error::LimitsExceeded(
                "too many requests are waiting".into(),
            )),
            Err(TrySendError::Closed(_)) => {
                Err(fdo::Error::Failed("Aoide is shutting down".into()))
            }
        }
    }

    fn state(&self) -> TrayState {
        lock(&self.shared).state.clone()
    }

    fn revision(&self) -> u32 {
        lock(&self.shared).revision
    }
}

struct Item {
    requests: Requests,
    icon_name: String,
    pixmaps: Pixmaps,
}

#[interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    /// A left click shows or hides the window.
    fn activate(&self, _x: i32, _y: i32) -> fdo::Result<()> {
        self.requests.send(TrayCommand::ToggleWindow)
    }

    /// A middle click plays or pauses, as media players' tray icons do.
    fn secondary_activate(&self, _x: i32, _y: i32) -> fdo::Result<()> {
        let state = self.requests.state();
        match command_for_event(&state, crate::tray::PLAY_PAUSE, "clicked") {
            Some(command) => self.requests.send(command),
            None => Ok(()),
        }
    }

    /// Hosts open `Menu` themselves; nothing to do.
    fn context_menu(&self, _x: i32, _y: i32) {}

    fn scroll(&self, _delta: i32, _orientation: String) {}

    #[zbus(signal)]
    async fn new_tool_tip(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(property(emits_changed_signal = "const"))]
    fn category(&self) -> &str {
        "ApplicationStatus"
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn id(&self) -> &str {
        ITEM_ID
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn title(&self) -> &str {
        TOOLTIP_TITLE
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn status(&self) -> &str {
        "Active"
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn window_id(&self) -> i32 {
        0
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn icon_name(&self) -> &str {
        &self.icon_name
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn icon_pixmap(&self) -> Pixmaps {
        self.pixmaps.clone()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn overlay_icon_name(&self) -> &str {
        ""
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn overlay_icon_pixmap(&self) -> Pixmaps {
        Vec::new()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn attention_icon_name(&self) -> &str {
        ""
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn attention_icon_pixmap(&self) -> Pixmaps {
        Vec::new()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn attention_movie_name(&self) -> &str {
        ""
    }

    /// Announced by `NewToolTip`, as the spec has it, not `PropertiesChanged`.
    #[zbus(property(emits_changed_signal = "false"))]
    fn tool_tip(&self) -> (String, Pixmaps, String, String) {
        let state = self.requests.state();
        (
            self.icon_name.clone(),
            Vec::new(),
            TOOLTIP_TITLE.to_owned(),
            tooltip_body(&state),
        )
    }

    /// The item has a menu but a click is not "open the menu".
    #[zbus(property(emits_changed_signal = "const"))]
    fn item_is_menu(&self) -> bool {
        false
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn menu(&self) -> ObjectPath<'static> {
        ObjectPath::from_static_str_unchecked(MENU_PATH)
    }
}

/// One item's properties, keeping only `wanted` (all when empty).
fn properties(item: &MenuItem, wanted: &[String]) -> HashMap<String, OwnedValue> {
    let mut all: Vec<(&str, Value<'static>)> = Vec::new();
    match &item.kind {
        ItemKind::Standard { label, enabled } => {
            all.push(("label", Value::from(*label)));
            all.push(("enabled", Value::from(*enabled)));
        }
        ItemKind::Separator => all.push(("type", Value::from("separator"))),
    }
    all.into_iter()
        .filter(|(name, _)| wanted.is_empty() || wanted.iter().any(|w| w == name))
        .filter_map(|(name, value)| Some((name.to_owned(), OwnedValue::try_from(value).ok()?)))
        .collect()
}

fn root_properties(wanted: &[String]) -> HashMap<String, OwnedValue> {
    if !wanted.is_empty() && !wanted.iter().any(|w| w == "children-display") {
        return HashMap::new();
    }
    HashMap::from([(
        "children-display".to_owned(),
        OwnedValue::from(zbus::zvariant::Str::from_static("submenu")),
    )])
}

/// A child node as the `v` in a parent's `av`.
fn child_value(node: Node) -> Option<OwnedValue> {
    OwnedValue::try_from(Value::from(Structure::from(node))).ok()
}

struct Menu {
    requests: Requests,
}

#[interface(name = "com.canonical.dbusmenu")]
impl Menu {
    fn get_layout(
        &self,
        parent_id: i32,
        recursion_depth: i32,
        property_names: Vec<String>,
    ) -> fdo::Result<(u32, Node)> {
        let state = self.requests.state();
        let items = menu(&state);
        let revision = self.requests.revision();
        if parent_id == ROOT {
            let children = if recursion_depth == 0 {
                Vec::new()
            } else {
                items
                    .iter()
                    .filter_map(|item| {
                        child_value((item.id, properties(item, &property_names), Vec::new()))
                    })
                    .collect()
            };
            return Ok((revision, (ROOT, root_properties(&property_names), children)));
        }
        let item = items
            .iter()
            .find(|item| item.id == parent_id)
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("no menu item {parent_id}")))?;
        Ok((
            revision,
            (item.id, properties(item, &property_names), Vec::new()),
        ))
    }

    fn get_group_properties(
        &self,
        ids: Vec<i32>,
        property_names: Vec<String>,
    ) -> Vec<(i32, HashMap<String, OwnedValue>)> {
        let items = menu(&self.requests.state());
        items
            .iter()
            .filter(|item| ids.is_empty() || ids.contains(&item.id))
            .map(|item| (item.id, properties(item, &property_names)))
            .collect()
    }

    fn get_property(&self, id: i32, name: String) -> fdo::Result<OwnedValue> {
        let items = menu(&self.requests.state());
        let item = items
            .iter()
            .find(|item| item.id == id)
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("no menu item {id}")))?;
        properties(item, std::slice::from_ref(&name))
            .remove(&name)
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("item {id} has no {name}")))
    }

    fn event(
        &self,
        id: i32,
        event_id: String,
        _data: OwnedValue,
        _timestamp: u32,
    ) -> fdo::Result<()> {
        match command_for_event(&self.requests.state(), id, &event_id) {
            Some(command) => self.requests.send(command),
            None => Ok(()),
        }
    }

    /// Returns the ids that were not found, as the spec asks.
    fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> fdo::Result<Vec<i32>> {
        let state = self.requests.state();
        let items = menu(&state);
        let mut not_found = Vec::new();
        for (id, event_id, _, _) in events {
            if !items.iter().any(|item| item.id == id) {
                not_found.push(id);
                continue;
            }
            if let Some(command) = command_for_event(&state, id, &event_id) {
                self.requests.send(command)?;
            }
        }
        Ok(not_found)
    }

    /// The menu is always current; nothing to refresh before it opens.
    fn about_to_show(&self, _id: i32) -> bool {
        false
    }

    fn about_to_show_group(&self, _ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        (Vec::new(), Vec::new())
    }

    #[zbus(signal)]
    async fn layout_updated(
        emitter: &SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;

    #[zbus(property(emits_changed_signal = "const"))]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn text_direction(&self) -> &str {
        "ltr"
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn status(&self) -> &str {
        "normal"
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn icon_theme_path(&self) -> Vec<String> {
        Vec::new()
    }
}
