//! The sync status panel, opened from Settings: `SyncEngine::overview()` in
//! plain words. When it last synced and what it did, what is waiting, what is
//! held back and why, what was set aside, with Sync Now and Retry for the
//! changes the server failed on.
//!
//! The overview is read on a runtime thread (it counts rows in the store),
//! when the panel opens, after anything it starts, after every sync, and once
//! a second while a sync runs, so the phase moves.

use std::collections::HashMap;
use std::time::Duration;

use aoide_sidecar::SyncOverview;
use aoide_store::{Millis, SyncPhase};
use iced::widget::{button, text};
use iced::{Element, Task};

use super::{Ctx, Load, PageMsg};
use crate::app::{App, Message as AppMessage};
use crate::theme;

mod view;

/// One line of the panel: what it says, and the longer explanation under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub detail: Option<String>,
    /// A problem, drawn as one.
    pub warning: bool,
}

impl Line {
    fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            detail: None,
            warning: false,
        }
    }

    fn explained(text: impl Into<String>, detail: impl Into<String>, warning: bool) -> Self {
        Self {
            text: text.into(),
            detail: Some(detail.into()),
            warning,
        }
    }
}

fn changes(n: i64) -> String {
    if n == 1 {
        "1 change".to_owned()
    } else {
        format!("{n} changes")
    }
}

/// "just now", "4 minutes ago", "2 hours ago", "yesterday", "3 days ago".
pub fn ago(then: Millis, now: Millis) -> String {
    let seconds = (now - then).max(0) / 1000;
    let plural = |n: i64, unit: &str| {
        if n == 1 {
            format!("1 {unit} ago")
        } else {
            format!("{n} {unit}s ago")
        }
    };
    match seconds {
        0..60 => "just now".to_owned(),
        60..3_600 => plural(seconds / 60, "minute"),
        3_600..86_400 => plural(seconds / 3_600, "hour"),
        86_400..172_800 => "yesterday".to_owned(),
        _ => plural(seconds / 86_400, "day"),
    }
}

/// A local table's changes as a person would name them.
fn kind(table: &str) -> &str {
    use aoide_store::SyncEntity as E;
    match E::from_local_table(table) {
        Some(E::TrackFlags) => "taste flags (not interested, don't count plays)",
        Some(E::Like) => "likes",
        Some(E::QueueState) => "the queue for handoff",
        Some(E::PlayEvent) => "plays",
        Some(E::Playlist | E::PlaylistItem) => "playlist edits",
        Some(E::Folder) => "folders",
        None => table,
    }
}

/// The overview in plain words, first line first. `names` turns a playlist
/// id into its name for the held edits.
pub fn describe(
    overview: &SyncOverview,
    names: &HashMap<String, String>,
    now: Millis,
) -> Vec<Line> {
    let mut lines = Vec::new();
    if overview.sidecar_absent {
        lines.push(Line::explained(
            "This server can't sync yet.",
            "It doesn't sync Aoide's playlists and history, so they stay on this computer. \
             They'll sync once the server is updated.",
            true,
        ));
        return lines;
    }
    lines.push(Line::plain(progress(overview, now)));
    lines.extend(last_outcome(overview));
    let held_here = overview.held_back + overview.blocked_by_share as i64;
    let waiting = overview.waiting - held_here;
    if waiting > 0 {
        lines.push(Line::plain(format!(
            "{} waiting to be sent.",
            changes(waiting)
        )));
    }
    lines.extend(held_back(overview));
    lines.extend(blocked_by_share(overview, names));
    lines.extend(set_aside(overview));
    if let Some(error) = &overview.share_error {
        lines.push(Line::explained(
            "The share list couldn't be read.",
            error.clone(),
            true,
        ));
    }
    lines
}

/// The first line: what the sync is doing, or when it last ran.
fn progress(overview: &SyncOverview, now: Millis) -> String {
    match (&overview.phase, overview.running) {
        (SyncPhase::Pushing { pushed }, true) => {
            format!("Syncing: sending changes, {pushed} so far.")
        }
        (SyncPhase::Pulling { applied }, true) => {
            format!("Syncing: bringing in changes, {applied} so far.")
        }
        (_, true) => "Syncing…".to_owned(),
        _ => match overview.last_synced_at {
            Some(at) => format!("Last synced {}.", ago(at, now)),
            None => "Not synced yet.".to_owned(),
        },
    }
}

/// How the last sync went: why it failed, or what it moved.
fn last_outcome(overview: &SyncOverview) -> Option<Line> {
    if let Some(error) = &overview.error {
        return Some(Line::explained(
            "The last sync didn't finish.",
            format!("{error}. Nothing is lost: it tries again at the next sync."),
            true,
        ));
    }
    overview.last_report.as_ref()?;
    let applied = if overview.applied == overview.pulled {
        String::new()
    } else {
        format!(", {} of which changed something here", overview.applied)
    };
    Some(Line::plain(format!(
        "Last time: sent {}, brought in {}{applied}.",
        changes(overview.pushed as i64),
        changes(overview.pulled as i64),
    )))
}

/// Changes the server's sidecar doesn't accept yet.
fn held_back(overview: &SyncOverview) -> Option<Line> {
    if overview.held_back <= 0 {
        return None;
    }
    let kinds: Vec<&str> = overview.held_entities.iter().map(|t| kind(t)).collect();
    Some(Line::explained(
        format!(
            "{} held back on this computer.",
            changes(overview.held_back)
        ),
        format!(
            "Your server doesn't accept {} yet. They're kept here and go as soon as the \
             server is updated.",
            if kinds.is_empty() {
                "them".to_owned()
            } else {
                kinds.join(" or ")
            }
        ),
        false,
    ))
}

/// Edits to playlists no longer shared with this listener for editing.
fn blocked_by_share(overview: &SyncOverview, names: &HashMap<String, String>) -> Option<Line> {
    if overview.blocked_by_share == 0 {
        return None;
    }
    let mut playlists: Vec<String> = overview
        .blocked_edits
        .iter()
        .filter_map(|edit| edit.playlist_id.as_ref())
        .map(|id| {
            names.get(id).map_or_else(
                || "a playlist".to_owned(),
                |n| format!("\u{201c}{n}\u{201d}"),
            )
        })
        .collect();
    playlists.sort();
    playlists.dedup();
    let which = if playlists.is_empty() {
        "a shared playlist".to_owned()
    } else {
        playlists.join(", ")
    };
    Some(Line::explained(
        format!(
            "{} can't be sent: {which} is no longer shared with you for editing.",
            changes(overview.blocked_by_share as i64)
        ),
        "They're kept, not thrown away. If the owner shares it with you again they go \
         with the next sync; the share list is checked every five minutes.",
        true,
    ))
}

/// Changes that can't sync as they are, and how many can be retried.
fn set_aside(overview: &SyncOverview) -> Option<Line> {
    if overview.quarantined <= 0 {
        return None;
    }
    let retry = match overview.retryable {
        0 => String::new(),
        1 => " The server failed on 1 of them; that one can be tried again.".to_owned(),
        n => format!(" The server failed on {n} of them; those can be tried again."),
    };
    Some(Line::explained(
        format!("{} set aside.", changes(overview.quarantined)),
        format!(
            "They can't sync as they are: the server refused them, or this version can't \
             read them. They're kept on this computer rather than lost.{retry}"
        ),
        true,
    ))
}

/// What the panel shows: the overview, and the names of held playlists.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub overview: SyncOverview,
    pub names: HashMap<String, String>,
}

#[derive(Debug)]
pub struct State {
    snapshot: Load<Option<Snapshot>>,
    /// What the last action said: "2 changes will be tried again."
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Snapshot(Result<Option<Box<Snapshot>>, String>),
    Note(String),
}

/// The app's side: reading the engine, and the panel's actions.
#[derive(Debug, Clone)]
pub enum Msg {
    /// Read the overview for the panel with this key.
    Refresh(u64),
    /// Put the server-faulted changes back in the queue, and sync.
    Retry(u64),
    /// Sync Now from the panel, which then shows it running.
    SyncNow(u64),
}

pub fn open(ctx: &Ctx<'_>) -> (State, Task<AppMessage>) {
    (
        State {
            snapshot: Load::Loading,
            note: None,
        },
        Task::done(AppMessage::SyncPanel(Msg::Refresh(ctx.key))),
    )
}

/// Reads the overview and the names it needs, off the UI thread.
pub async fn snapshot(
    engine: Option<crate::sync::Engine>,
    store: crate::store_worker::StoreHandle,
) -> Result<Option<Box<Snapshot>>, String> {
    let Some(engine) = engine else {
        return Ok(None);
    };
    let overview = tokio::task::spawn_blocking(move || engine.overview())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let ids: Vec<String> = overview
        .blocked_edits
        .iter()
        .filter_map(|e| e.playlist_id.clone())
        .collect();
    let names = store
        .call(move |w| {
            ids.iter()
                .filter_map(|id| {
                    let list = w.store.playlist(id).ok().flatten()?;
                    Some((id.clone(), list.name))
                })
                .collect()
        })
        .await
        .unwrap_or_default();
    Ok(Some(Box::new(Snapshot { overview, names })))
}

impl App {
    pub(crate) fn sync_panel(&mut self, message: Msg) -> Task<AppMessage> {
        let store = self.store.clone();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let engine = signed.sync.clone();
        match message {
            Msg::Refresh(key) => Task::perform(snapshot(engine, store), move |r| {
                AppMessage::Page(key, PageMsg::SyncStatus(Message::Snapshot(r)))
            }),
            Msg::Retry(key) => Task::perform(
                async move {
                    store
                        .call(|w| w.store.requeue_quarantined(None).unwrap_or(0))
                        .await
                        .unwrap_or(0)
                },
                move |moved| {
                    AppMessage::Page(
                        key,
                        PageMsg::SyncStatus(Message::Note(match moved {
                            0 => "Nothing could be put back: those changes predate retrying."
                                .to_owned(),
                            1 => "1 change will be tried again.".to_owned(),
                            n => format!("{n} changes will be tried again."),
                        })),
                    )
                },
            )
            .chain(Task::done(AppMessage::SyncNow))
            .chain(Task::done(AppMessage::SyncPanel(Msg::Refresh(key)))),
            Msg::SyncNow(key) => Task::batch([
                self.sync_now(),
                Task::done(AppMessage::SyncPanel(Msg::Refresh(key))),
            ]),
        }
    }

    /// Every open panel, asked to read the overview again: a sync finished.
    pub(crate) fn refresh_sync_panels(&mut self) -> Task<AppMessage> {
        let Some(nav) = self.session.as_mut().and_then(|s| s.nav.as_mut()) else {
            return Task::none();
        };
        let keys: Vec<u64> = nav
            .iter_mut()
            .filter(|e| matches!(e.page, super::Page::SyncStatus(_)))
            .map(|e| e.key)
            .collect();
        Task::batch(
            keys.into_iter()
                .map(|key| Task::done(AppMessage::SyncPanel(Msg::Refresh(key)))),
        )
    }
}

impl State {
    pub fn update(&mut self, message: Message, ctx: &Ctx<'_>) -> Task<AppMessage> {
        match message {
            Message::Snapshot(result) => {
                let running = result
                    .as_ref()
                    .ok()
                    .and_then(|s| s.as_ref())
                    .is_some_and(|s| s.overview.running);
                self.snapshot = Load::from(result.map(|s| s.map(|s| *s)));
                if running {
                    // Again in a second, so the phase moves while it runs.
                    let key = ctx.key;
                    return Task::perform(tokio::time::sleep(Duration::from_secs(1)), move |()| {
                        AppMessage::SyncPanel(Msg::Refresh(key))
                    });
                }
                Task::none()
            }
            Message::Note(note) => {
                self.note = Some(note);
                Task::none()
            }
        }
    }
}

/// The way in from Settings.
pub fn settings_link<'a>() -> Element<'a, AppMessage> {
    button(text("Details…"))
        .style(theme::tonal)
        .padding([6, 14])
        .on_press(AppMessage::Open(super::Route::SyncStatus))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoide_sidecar::BlockedEdit;
    use aoide_store::SyncReport;

    const NOW: Millis = 1_792_540_800_000;

    fn overview() -> SyncOverview {
        SyncOverview {
            running: false,
            phase: SyncPhase::Idle,
            last_synced_at: Some(NOW - 4 * 60_000),
            last_attempt_at: Some(NOW - 4 * 60_000),
            pushed: 3,
            pulled: 5,
            applied: 2,
            waiting: 0,
            quarantined: 0,
            retryable: 0,
            held_back: 0,
            held_entities: Vec::new(),
            blocked_by_share: 0,
            blocked_edits: Vec::new(),
            error: None,
            share_error: None,
            sidecar_absent: false,
            last_report: Some(SyncReport::default()),
        }
    }

    fn texts(lines: &[Line]) -> Vec<&str> {
        lines.iter().map(|l| l.text.as_str()).collect()
    }

    #[test]
    fn a_clean_sync_says_when_and_what() {
        let lines = describe(&overview(), &HashMap::new(), NOW);
        assert_eq!(
            texts(&lines),
            [
                "Last synced 4 minutes ago.",
                "Last time: sent 3 changes, brought in 5 changes, 2 of which changed something here."
            ]
        );
    }

    #[test]
    fn held_and_blocked_changes_are_not_counted_as_waiting() {
        let mut o = overview();
        o.waiting = 7;
        o.held_back = 2;
        o.held_entities = vec![aoide_store::SyncEntity::TrackFlags.local_table().into()];
        o.blocked_by_share = 1;
        o.blocked_edits = vec![BlockedEdit {
            op_id: "op".into(),
            playlist_id: Some("p1".into()),
            reason: "belongs to another user".into(),
            still_shared: Some(false),
        }];
        let names = HashMap::from([("p1".to_owned(), "Road Trip".to_owned())]);
        let lines = describe(&o, &names, NOW);
        let said = texts(&lines);
        assert!(said.contains(&"4 changes waiting to be sent."), "{said:?}");
        assert!(said.contains(&"2 changes held back on this computer."));
        assert!(said.contains(
            &"1 change can't be sent: \u{201c}Road Trip\u{201d} is no longer shared with you for editing."
        ));
        let held = lines.iter().find(|l| l.text.contains("held back")).unwrap();
        assert!(held.detail.as_ref().unwrap().contains("taste flags"));
        assert!(!held.warning, "waiting for an update is not a fault");
    }

    #[test]
    fn set_aside_changes_say_which_can_be_retried() {
        let mut o = overview();
        o.quarantined = 3;
        o.retryable = 1;
        let lines = describe(&o, &HashMap::new(), NOW);
        let aside = lines
            .iter()
            .find(|l| l.text == "3 changes set aside.")
            .unwrap();
        assert!(
            aside
                .detail
                .as_ref()
                .unwrap()
                .ends_with("that one can be tried again.")
        );
    }

    #[test]
    fn a_failure_and_a_missing_sidecar_say_so_plainly() {
        let mut o = overview();
        o.error = Some("aoide/sync/push returned HTTP 503: Database locked".into());
        let lines = describe(&o, &HashMap::new(), NOW);
        assert_eq!(lines[1].text, "The last sync didn't finish.");
        assert!(
            lines[1]
                .detail
                .as_ref()
                .unwrap()
                .contains("Database locked")
        );
        o.sidecar_absent = true;
        let lines = describe(&o, &HashMap::new(), NOW);
        assert_eq!(texts(&lines), ["This server can't sync yet."]);
    }

    #[test]
    fn a_running_sync_says_where_it_is() {
        let mut o = overview();
        o.running = true;
        o.phase = SyncPhase::Pulling { applied: 12 };
        assert_eq!(
            describe(&o, &HashMap::new(), NOW)[0].text,
            "Syncing: bringing in changes, 12 so far."
        );
    }

    #[test]
    fn ages_read_like_speech() {
        assert_eq!(ago(NOW - 5_000, NOW), "just now");
        assert_eq!(ago(NOW - 60_000, NOW), "1 minute ago");
        assert_eq!(ago(NOW - 3 * 3_600_000, NOW), "3 hours ago");
        assert_eq!(ago(NOW - 30 * 3_600_000, NOW), "yesterday");
        assert_eq!(ago(NOW - 5 * 86_400_000, NOW), "5 days ago");
    }
}
