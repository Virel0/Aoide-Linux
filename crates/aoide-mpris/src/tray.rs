//! The tray icon's model: what the menu holds for a given state, what a click
//! on it means, and the icon in the byte order StatusNotifierItem wants. Pure
//! data, tested everywhere; `linux/tray.rs` puts it on the bus.

use crate::types::PlaybackStatus;

/// What the tray shows. The app sends a fresh one to [`crate::Tray::update`]
/// whenever any of it changes; an unchanged state sends nothing on the bus.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrayState {
    pub status: PlaybackStatus,
    /// What plays, as one line ("Title — Artist"), shown under "Aoide" in the
    /// tooltip. `None` when nothing is loaded.
    pub now_playing: Option<String>,
    pub can_go_next: bool,
    pub can_go_previous: bool,
    /// Whether anything is loaded that Play could start.
    pub can_play: bool,
}

/// A request from the tray. Arrives on the receiver [`crate::Tray::start`]
/// returns, like MPRIS's [`crate::Command`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    PlayPause,
    Next,
    Previous,
    Stop,
    /// "Open Aoide" in the menu: show the window and bring it to the front.
    OpenWindow,
    /// A left click on the icon: hide the window if it shows, show it if not.
    /// The crate does not know which; the app does.
    ToggleWindow,
    Quit,
}

/// The tray's icon. Hosts that follow the icon theme use `name`; the rest
/// draw the first of `pixmaps` that fits, so give one or more sizes (22, 32
/// and 48 px cover the common panels).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrayIcon {
    /// A freedesktop icon name, normally the app's desktop id.
    pub name: String,
    pub pixmaps: Vec<Pixmap>,
}

/// One size of the icon, as StatusNotifierItem's `IconPixmap` carries it:
/// ARGB32, one `u32` per pixel in network (big-endian) byte order, rows top
/// to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pixmap {
    pub width: i32,
    pub height: i32,
    pub argb: Vec<u8>,
}

impl Pixmap {
    /// From straight RGBA bytes (what the `image` crate decodes to). `None` if
    /// `rgba` is not `width * height * 4` bytes or a side is zero.
    pub fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> Option<Pixmap> {
        let pixels = (width as usize).checked_mul(height as usize)?;
        if pixels == 0 || rgba.len() != pixels.checked_mul(4)? {
            return None;
        }
        let (chunks, _) = rgba.as_chunks::<4>();
        let argb = chunks
            .iter()
            .flat_map(|&[r, g, b, a]| [a, r, g, b])
            .collect();
        Some(Pixmap {
            width: i32::try_from(width).ok()?,
            height: i32::try_from(height).ok()?,
            argb,
        })
    }
}

/// The tray's title and the tooltip's heading.
pub const TOOLTIP_TITLE: &str = "Aoide";

/// The tooltip's second line: what plays, and whether it is paused.
pub(crate) fn tooltip_body(state: &TrayState) -> String {
    match (&state.now_playing, state.status) {
        (Some(what), PlaybackStatus::Paused) => format!("{what} (paused)"),
        (Some(what), _) => what.clone(),
        (None, _) => String::new(),
    }
}

/// Menu item ids. 0 is the root, as the DBusMenu spec has it.
pub(crate) const ROOT: i32 = 0;
pub(crate) const PLAY_PAUSE: i32 = 1;
pub(crate) const NEXT: i32 = 2;
pub(crate) const PREVIOUS: i32 = 3;
pub(crate) const STOP: i32 = 4;
const SEPARATOR_1: i32 = 5;
pub(crate) const OPEN: i32 = 6;
const SEPARATOR_2: i32 = 7;
pub(crate) const QUIT: i32 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ItemKind {
    Standard { label: &'static str, enabled: bool },
    Separator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MenuItem {
    pub id: i32,
    pub kind: ItemKind,
}

/// The menu, top to bottom, for this state.
pub(crate) fn menu(state: &TrayState) -> Vec<MenuItem> {
    let playing = state.status == PlaybackStatus::Playing;
    let item = |id, label, enabled| MenuItem {
        id,
        kind: ItemKind::Standard { label, enabled },
    };
    let separator = |id| MenuItem {
        id,
        kind: ItemKind::Separator,
    };
    vec![
        item(
            PLAY_PAUSE,
            if playing { "Pause" } else { "Play" },
            playing || state.can_play,
        ),
        item(NEXT, "Next", state.can_go_next),
        item(PREVIOUS, "Previous", state.can_go_previous),
        item(STOP, "Stop", state.status != PlaybackStatus::Stopped),
        separator(SEPARATOR_1),
        item(OPEN, "Open Aoide", true),
        separator(SEPARATOR_2),
        item(QUIT, "Quit", true),
    ]
}

/// What a DBusMenu event on item `id` asks for. Only `clicked` on an
/// enabled item does anything; hovering, opening and closing do not.
pub(crate) fn command_for_event(state: &TrayState, id: i32, event: &str) -> Option<TrayCommand> {
    if event != "clicked" {
        return None;
    }
    let enabled = menu(state)
        .into_iter()
        .any(|item| item.id == id && matches!(item.kind, ItemKind::Standard { enabled: true, .. }));
    if !enabled {
        return None;
    }
    match id {
        PLAY_PAUSE => Some(TrayCommand::PlayPause),
        NEXT => Some(TrayCommand::Next),
        PREVIOUS => Some(TrayCommand::Previous),
        STOP => Some(TrayCommand::Stop),
        OPEN => Some(TrayCommand::OpenWindow),
        QUIT => Some(TrayCommand::Quit),
        _ => None,
    }
}

/// What changed between two states, as the tray's signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct TrayChanges {
    /// `NewToolTip` on the item.
    pub tooltip: bool,
    /// `LayoutUpdated` on the menu, with a new revision.
    pub menu: bool,
}

pub(crate) fn tray_changes(prev: &TrayState, next: &TrayState) -> TrayChanges {
    TrayChanges {
        tooltip: tooltip_body(prev) != tooltip_body(next),
        menu: menu(prev) != menu(next),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing() -> TrayState {
        TrayState {
            status: PlaybackStatus::Playing,
            now_playing: Some("Song — Artist".into()),
            can_go_next: true,
            can_go_previous: true,
            can_play: true,
        }
    }

    fn label(state: &TrayState, id: i32) -> (&'static str, bool) {
        match menu(state).into_iter().find(|i| i.id == id).unwrap().kind {
            ItemKind::Standard { label, enabled } => (label, enabled),
            ItemKind::Separator => panic!("{id} is a separator"),
        }
    }

    #[test]
    fn the_menu_holds_the_six_actions_in_order() {
        let labels: Vec<_> = menu(&playing())
            .into_iter()
            .filter_map(|i| match i.kind {
                ItemKind::Standard { label, .. } => Some(label),
                ItemKind::Separator => None,
            })
            .collect();
        assert_eq!(
            labels,
            ["Pause", "Next", "Previous", "Stop", "Open Aoide", "Quit"]
        );
    }

    #[test]
    fn play_pause_says_what_it_will_do() {
        assert_eq!(label(&playing(), PLAY_PAUSE), ("Pause", true));
        let paused = TrayState {
            status: PlaybackStatus::Paused,
            ..playing()
        };
        assert_eq!(label(&paused, PLAY_PAUSE), ("Play", true));
    }

    #[test]
    fn with_nothing_loaded_only_open_and_quit_work() {
        let idle = TrayState::default();
        for id in [PLAY_PAUSE, NEXT, PREVIOUS, STOP] {
            assert!(!label(&idle, id).1, "{id} should be disabled");
            assert_eq!(command_for_event(&idle, id, "clicked"), None);
        }
        assert_eq!(
            command_for_event(&idle, OPEN, "clicked"),
            Some(TrayCommand::OpenWindow)
        );
        assert_eq!(
            command_for_event(&idle, QUIT, "clicked"),
            Some(TrayCommand::Quit)
        );
    }

    #[test]
    fn clicks_become_commands() {
        let state = playing();
        let expected = [
            (PLAY_PAUSE, TrayCommand::PlayPause),
            (NEXT, TrayCommand::Next),
            (PREVIOUS, TrayCommand::Previous),
            (STOP, TrayCommand::Stop),
            (OPEN, TrayCommand::OpenWindow),
            (QUIT, TrayCommand::Quit),
        ];
        for (id, command) in expected {
            assert_eq!(command_for_event(&state, id, "clicked"), Some(command));
        }
    }

    #[test]
    fn other_events_and_unknown_items_do_nothing() {
        let state = playing();
        for event in ["hovered", "opened", "closed", "Clicked", ""] {
            assert_eq!(command_for_event(&state, PLAY_PAUSE, event), None);
        }
        for id in [ROOT, SEPARATOR_1, SEPARATOR_2, 99, -1] {
            assert_eq!(command_for_event(&state, id, "clicked"), None);
        }
    }

    #[test]
    fn next_and_previous_follow_the_queue() {
        let state = TrayState {
            can_go_next: false,
            ..playing()
        };
        assert_eq!(command_for_event(&state, NEXT, "clicked"), None);
        assert_eq!(
            command_for_event(&state, PREVIOUS, "clicked"),
            Some(TrayCommand::Previous)
        );
    }

    #[test]
    fn the_tooltip_says_what_plays() {
        assert_eq!(tooltip_body(&playing()), "Song — Artist");
        let paused = TrayState {
            status: PlaybackStatus::Paused,
            ..playing()
        };
        assert_eq!(tooltip_body(&paused), "Song — Artist (paused)");
        assert_eq!(tooltip_body(&TrayState::default()), "");
    }

    #[test]
    fn changes_name_only_what_moved() {
        let a = playing();
        assert_eq!(tray_changes(&a, &a), TrayChanges::default());

        let paused = TrayState {
            status: PlaybackStatus::Paused,
            ..a.clone()
        };
        assert_eq!(
            tray_changes(&a, &paused),
            TrayChanges {
                tooltip: true,
                menu: true
            }
        );

        let other_song = TrayState {
            now_playing: Some("Other — Artist".into()),
            ..a.clone()
        };
        assert_eq!(
            tray_changes(&a, &other_song),
            TrayChanges {
                tooltip: true,
                menu: false
            }
        );

        let last_track = TrayState {
            can_go_next: false,
            ..a.clone()
        };
        assert_eq!(
            tray_changes(&a, &last_track),
            TrayChanges {
                tooltip: false,
                menu: true
            }
        );
    }

    #[test]
    fn pixmaps_are_argb_in_network_order() {
        // Two pixels: opaque red, half-transparent blue.
        let rgba = [255, 0, 0, 255, 0, 0, 255, 128];
        let pixmap = Pixmap::from_rgba(2, 1, &rgba).unwrap();
        assert_eq!((pixmap.width, pixmap.height), (2, 1));
        assert_eq!(pixmap.argb, [255, 255, 0, 0, 128, 0, 0, 255]);
    }

    #[test]
    fn pixmaps_refuse_the_wrong_length() {
        assert_eq!(Pixmap::from_rgba(2, 2, &[0; 15]), None);
        assert_eq!(Pixmap::from_rgba(0, 0, &[]), None);
        assert_eq!(Pixmap::from_rgba(1, 1, &[0; 8]), None);
    }
}
