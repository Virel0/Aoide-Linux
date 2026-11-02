//! Requests: the music this listener has asked Melete for, newest first, with
//! where each stands (waiting, declined and why, or how its download is going,
//! kept up to date while the page is open) and Withdraw for one still waiting;
//! and the playlists asked for song by song, each with its songs and Remove.
//! Asking happens in Search and Import.

use aoide_melete::{MusicRequest, PlaylistImport, PlaylistImportSummary};
use iced::Task;

use super::{Ctx, Load, PageMsg, fetch};
use crate::app::Message as App;
use crate::failure::Failure;

mod live;
mod view;
mod words;

pub use words::standing;

#[derive(Debug)]
pub struct State {
    requests: Load<Vec<MusicRequest>>,
    imports: Load<Vec<PlaylistImportSummary>>,
    /// The import whose songs are shown, and them once read.
    open_import: Option<(i64, Load<PlaylistImport>)>,
    /// A withdrawal on its way, by request id.
    withdrawing: Option<i64>,
    /// The import Remove was pressed on: asking once more.
    confirm_remove: Option<i64>,
    /// A removal on its way, by import id.
    removing: Option<i64>,
    /// Why the last withdrawal or removal failed.
    problem: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<MusicRequest>, Failure>),
    /// The newest requests as they stand now, from the live feed.
    Live(Vec<MusicRequest>),
    Refresh,
    Withdraw(i64),
    Withdrawn(Result<(), Failure>),
    ImportsLoaded(Result<Vec<PlaylistImportSummary>, Failure>),
    /// An import is still asking: how far it has got.
    RefreshImports,
    /// Shows an import's songs, or hides them.
    ShowImport(i64),
    ImportRead(i64, Result<PlaylistImport, Failure>),
    Remove(i64),
    KeepImport,
    ConfirmRemove(i64),
    Removed(Result<(), Failure>),
}

fn wrap(message: Message) -> PageMsg {
    PageMsg::Requests(message)
}

fn load(ctx: &Ctx<'_>) -> Task<App> {
    let client = ctx.client.clone();
    fetch(ctx.key, async move { client.music_requests().await }, |r| {
        wrap(Message::Loaded(r))
    })
}

fn load_imports(ctx: &Ctx<'_>) -> Task<App> {
    let client = ctx.client.clone();
    fetch(
        ctx.key,
        async move { client.playlist_imports().await },
        |r| wrap(Message::ImportsLoaded(r)),
    )
}

fn read_import(ctx: &Ctx<'_>, id: i64) -> Task<App> {
    let client = ctx.client.clone();
    fetch(
        ctx.key,
        async move { client.playlist_import(id).await },
        move |r| wrap(Message::ImportRead(id, r)),
    )
}

pub fn open(ctx: &Ctx<'_>) -> (State, Task<App>) {
    let state = State {
        requests: Load::Loading,
        imports: Load::Loading,
        open_import: None,
        withdrawing: None,
        confirm_remove: None,
        removing: None,
        problem: None,
    };
    (state, Task::batch([load(ctx), load_imports(ctx)]))
}

/// The feed's newest requests over the list: it follows the 50 newest, so
/// older ones stay as the list read them.
fn merge(listed: &[MusicRequest], live: Vec<MusicRequest>) -> Vec<MusicRequest> {
    let Some(oldest) = live.iter().map(|r| r.id).min() else {
        return listed.to_vec();
    };
    let mut merged = live;
    merged.extend(listed.iter().filter(|r| r.id < oldest).cloned());
    merged
}

impl State {
    /// An import is still working through its songs, so worth asking about.
    pub fn imports_running(&self) -> bool {
        self.imports
            .ready()
            .is_some_and(|all| all.iter().any(words::import_running))
    }

    pub fn update(&mut self, message: Message, ctx: &Ctx<'_>) -> Task<App> {
        match message {
            Message::Loaded(result) => {
                self.requests = Load::from(result);
                Task::none()
            }
            Message::Live(live) => {
                let listed = self.requests.ready().map(Vec::as_slice).unwrap_or_default();
                self.requests = Load::Ready(merge(listed, live));
                Task::none()
            }
            Message::Refresh => Task::batch([load(ctx), load_imports(ctx)]),
            Message::Withdraw(id) => {
                self.withdrawing = Some(id);
                self.problem = None;
                let client = ctx.client.clone();
                fetch(
                    ctx.key,
                    async move { client.withdraw_request(id).await },
                    |r| wrap(Message::Withdrawn(r)),
                )
            }
            Message::Withdrawn(result) => {
                self.withdrawing = None;
                if let Err(error) = result {
                    self.problem = Some(error.to_string());
                }
                load(ctx)
            }
            Message::ImportsLoaded(result) => {
                self.imports = Load::from(result);
                Task::none()
            }
            Message::RefreshImports => match self.open_import {
                Some((id, _)) => Task::batch([load_imports(ctx), read_import(ctx, id)]),
                None => load_imports(ctx),
            },
            Message::ShowImport(id) => {
                if self
                    .open_import
                    .as_ref()
                    .is_some_and(|(open, _)| *open == id)
                {
                    self.open_import = None;
                    return Task::none();
                }
                self.open_import = Some((id, Load::Loading));
                read_import(ctx, id)
            }
            Message::ImportRead(id, result) => {
                if let Some((open, songs)) = &mut self.open_import
                    && *open == id
                {
                    *songs = Load::from(result);
                }
                Task::none()
            }
            Message::Remove(id) => {
                self.confirm_remove = Some(id);
                Task::none()
            }
            Message::KeepImport => {
                self.confirm_remove = None;
                Task::none()
            }
            Message::ConfirmRemove(id) => {
                self.confirm_remove = None;
                self.removing = Some(id);
                self.problem = None;
                let client = ctx.client.clone();
                fetch(
                    ctx.key,
                    async move { client.remove_playlist_import(id).await },
                    |r| wrap(Message::Removed(r)),
                )
            }
            Message::Removed(result) => {
                if self.open_import.as_ref().map(|(id, _)| *id) == self.removing {
                    self.open_import = None;
                }
                self.removing = None;
                if let Err(error) = result {
                    self.problem = Some(error.to_string());
                }
                // Its waiting requests went with it.
                Task::batch([load(ctx), load_imports(ctx)])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoide_melete::{RequestKind, RequestState};

    fn request(id: i64, state: RequestState) -> MusicRequest {
        MusicRequest {
            id,
            kind: RequestKind::Song,
            title: format!("Song {id}"),
            year: None,
            artist: None,
            state,
            reason: None,
            created_at: id,
            decided_at: None,
            decided_by: None,
            progress: None,
        }
    }

    #[test]
    fn the_feed_updates_the_newest_and_keeps_the_older() {
        let listed = [3, 2, 1].map(|id| request(id, RequestState::Requested));
        let live = vec![
            request(4, RequestState::Requested),
            request(3, RequestState::Approved),
        ];
        let merged = merge(&listed, live);
        let ids: Vec<_> = merged.iter().map(|r| (r.id, r.state)).collect();
        assert_eq!(
            ids,
            [
                (4, RequestState::Requested),
                (3, RequestState::Approved),
                (2, RequestState::Requested),
                (1, RequestState::Requested),
            ]
        );
        assert_eq!(merge(&listed, Vec::new()).len(), 3);
    }
}
