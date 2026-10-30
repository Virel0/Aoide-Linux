//! Aoide playlists: the ones kept on this computer and synced through
//! Melete, filed in folders. The page shows one folder at a time (the top
//! level first), with its folders above its playlists; a folder can be made,
//! renamed and deleted (its contents move up a level, nothing is lost), and a
//! playlist moved into another folder.

use std::collections::{HashMap, HashSet};

use aoide_store::Folder;
use iced::Task;

use super::{Ctx, Load, PageMsg, PlaylistSummary};
use crate::app::Message as App;

mod view;

/// The folders, as the page needs them: every live one, to walk up from the
/// one shown and to offer as places to move to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Folders {
    pub all: Vec<Folder>,
}

impl Folders {
    pub fn get(&self, id: &str) -> Option<&Folder> {
        self.all.iter().find(|f| f.id == id)
    }

    /// The folders directly under `parent` (`None`: the top level), in order.
    pub fn children(&self, parent: Option<&str>) -> Vec<&Folder> {
        self.all
            .iter()
            .filter(|f| f.parent_id.as_deref() == parent)
            .collect()
    }

    /// From the top level down to `folder`, for the path at the top.
    pub fn path(&self, folder: Option<&str>) -> Vec<&Folder> {
        let mut path = Vec::new();
        let mut at = folder.and_then(|id| self.get(id));
        let mut seen = HashSet::new();
        while let Some(folder) = at {
            if !seen.insert(folder.id.as_str()) {
                break;
            }
            path.push(folder);
            at = folder.parent_id.as_deref().and_then(|id| self.get(id));
        }
        path.reverse();
        path
    }

    /// Every folder as "Parent › Child", for the Move list.
    pub fn labelled(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .all
            .iter()
            .map(|f| {
                let names: Vec<&str> = self
                    .path(Some(&f.id))
                    .iter()
                    .map(|f| f.name.as_str())
                    .collect();
                (f.id.clone(), names.join(" \u{203a} "))
            })
            .collect();
        out.sort_by_key(|(_, label)| label.to_lowercase());
        out
    }
}

/// The playlists to list in `folder`: those filed there, and at the top level
/// also any filed under a folder this computer doesn't know (yet), so nothing
/// disappears while a sync is half way.
pub fn playlists_in<'a>(
    playlists: &'a [PlaylistSummary],
    folders: &Folders,
    folder: Option<&str>,
) -> Vec<&'a PlaylistSummary> {
    playlists
        .iter()
        .filter(|p| match (p.folder_id.as_deref(), folder) {
            (inside, Some(shown)) => inside == Some(shown),
            (None, None) => true,
            (Some(inside), None) => folders.get(inside).is_none(),
        })
        .collect()
}

#[derive(Debug)]
pub struct State {
    new_name: String,
    error: Option<String>,
    /// Each playlist's cover, once found.
    covers: HashMap<String, crate::artwork::ArtKey>,
    folders: Load<Folders>,
    /// The folder shown; `None` is the top level.
    folder: Option<String>,
    new_folder: Option<String>,
    renaming: Option<(String, String)>,
    confirm_delete: Option<String>,
    /// The playlist whose Move list is open.
    moving: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    NewName(String),
    Create,
    Created(Result<String, String>),
    FindCovers,
    Covers(HashMap<String, crate::artwork::ArtKey>),
    Folders(Result<Vec<Folder>, String>),
    /// Show this folder (`None`: the top level).
    Enter(Option<String>),
    NewFolder(Option<String>),
    CreateFolder,
    Rename(Option<(String, String)>),
    SaveRename,
    Delete(Option<String>),
    ConfirmDelete(String),
    Moving(Option<String>),
    MoveTo {
        playlist: String,
        folder: Option<String>,
    },
    /// A folder change finished: read them again.
    Changed(Result<(), String>),
}

pub fn open(ctx: &Ctx<'_>) -> (State, Task<App>) {
    let state = State {
        new_name: String::new(),
        error: None,
        covers: HashMap::new(),
        folders: Load::Loading,
        folder: None,
        new_folder: None,
        renaming: None,
        confirm_delete: None,
        moving: None,
    };
    let task = Task::batch([find_covers(ctx.key), load_folders(ctx)]);
    (state, task)
}

/// Asks for every listed playlist's cover.
fn find_covers(key: u64) -> Task<App> {
    Task::done(App::Covers(crate::covers::Msg::Resolve {
        key,
        ids: Vec::new(),
    }))
}

fn load_folders(ctx: &Ctx<'_>) -> Task<App> {
    let store = ctx.store.clone();
    let key = ctx.key;
    Task::perform(
        async move {
            store
                .call(|w| w.store.folders().map_err(|e| e.to_string()))
                .await
                .unwrap_or_else(|| Err("the library is closing".into()))
        },
        move |r| App::Page(key, PageMsg::Playlists(Message::Folders(r))),
    )
}

/// Runs a folder change on the store thread, then reads everything again.
fn change(
    ctx: &Ctx<'_>,
    job: impl FnOnce(&aoide_store::Store, i64) -> aoide_store::Result<()> + Send + 'static,
) -> Task<App> {
    let store = ctx.store.clone();
    let key = ctx.key;
    Task::perform(
        async move {
            let now = aoide_store::now_millis();
            store
                .call(move |w| job(&w.store, now).map_err(|e| e.to_string()))
                .await
                .unwrap_or_else(|| Err("the library is closing".into()))
        },
        move |r| App::Page(key, PageMsg::Playlists(Message::Changed(r))),
    )
}

impl State {
    /// The covers to draw, for the artwork cache.
    pub fn artwork(&self) -> Vec<crate::artwork::ArtKey> {
        self.covers.values().cloned().collect()
    }

    pub fn update(&mut self, message: Message, ctx: &Ctx<'_>) -> Task<App> {
        match message {
            Message::NewName(name) => {
                self.new_name = name;
                Task::none()
            }
            Message::Create => self.create(ctx),
            Message::FindCovers => find_covers(ctx.key),
            Message::Covers(covers) => {
                self.covers = covers;
                Task::none()
            }
            Message::Created(result) => self.created(result),
            Message::Folders(result) => self.folders_loaded(result),
            Message::Enter(folder) => {
                self.folder = folder;
                self.moving = None;
                self.confirm_delete = None;
                Task::none()
            }
            Message::NewFolder(name) => {
                self.new_folder = name;
                Task::none()
            }
            Message::CreateFolder => self.create_folder(ctx),
            Message::Rename(draft) => {
                self.renaming = draft;
                Task::none()
            }
            Message::SaveRename => self.save_rename(ctx),
            Message::Delete(id) => {
                self.confirm_delete = id;
                Task::none()
            }
            Message::ConfirmDelete(id) => {
                self.confirm_delete = None;
                change(ctx, move |store, now| store.delete_folder(&id, now))
            }
            Message::Moving(playlist) => {
                self.moving = playlist;
                Task::none()
            }
            Message::MoveTo { playlist, folder } => self.move_playlist(playlist, folder, ctx),
            Message::Changed(Ok(())) => {
                self.error = None;
                Task::batch([load_folders(ctx), Task::done(App::PlaylistsChanged)])
            }
            Message::Changed(Err(error)) => {
                self.error = Some(error);
                load_folders(ctx)
            }
        }
    }

    /// New Playlist: made in the folder shown, under the name typed.
    fn create(&mut self, ctx: &Ctx<'_>) -> Task<App> {
        let name = self.new_name.trim().to_owned();
        if name.is_empty() {
            return Task::none();
        }
        self.new_name.clear();
        let store = ctx.store.clone();
        let key = ctx.key;
        // Made in the folder shown.
        let folder = self.folder.clone();
        Task::perform(
            async move {
                let now = aoide_store::now_millis();
                store
                    .call(move |w| {
                        w.store
                            .create_playlist(&name, folder.as_deref(), None, now)
                            .map(|p| p.id)
                            .map_err(|e| e.to_string())
                    })
                    .await
                    .unwrap_or_else(|| Err("the library is closing".into()))
            },
            move |r| App::Page(key, PageMsg::Playlists(Message::Created(r))),
        )
    }

    fn created(&mut self, result: Result<String, String>) -> Task<App> {
        match result {
            Ok(_) => {
                self.error = None;
                Task::done(App::PlaylistsChanged)
            }
            Err(error) => {
                self.error = Some(format!("The playlist couldn't be made: {error}"));
                Task::none()
            }
        }
    }

    fn folders_loaded(&mut self, result: Result<Vec<Folder>, String>) -> Task<App> {
        self.folders = Load::from(result.map(|all| Folders { all }));
        // The folder shown may have gone (deleted, here or elsewhere).
        if let (Some(shown), Load::Ready(folders)) = (&self.folder, &self.folders)
            && folders.get(shown).is_none()
        {
            self.folder = None;
        }
        Task::none()
    }

    fn create_folder(&mut self, ctx: &Ctx<'_>) -> Task<App> {
        let Some(name) = self.new_folder.take().map(|n| n.trim().to_owned()) else {
            return Task::none();
        };
        if name.is_empty() {
            return Task::none();
        }
        let parent = self.folder.clone();
        change(ctx, move |store, now| {
            store.create_folder(&name, parent.as_deref(), now).map(drop)
        })
    }

    fn save_rename(&mut self, ctx: &Ctx<'_>) -> Task<App> {
        let Some((id, name)) = self.renaming.take() else {
            return Task::none();
        };
        let name = name.trim().to_owned();
        if name.is_empty() {
            return Task::none();
        }
        change(ctx, move |store, now| {
            store.rename_folder(&id, &name, now).map(drop)
        })
    }

    fn move_playlist(
        &mut self,
        playlist: String,
        folder: Option<String>,
        ctx: &Ctx<'_>,
    ) -> Task<App> {
        self.moving = None;
        change(ctx, move |store, now| {
            store
                .move_playlist(&playlist, folder.as_deref(), now)
                .map(drop)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(id: &str, name: &str, parent: Option<&str>) -> Folder {
        let mut f = Folder::new(name, "a0", 1, "desk");
        f.id = id.into();
        f.parent_id = parent.map(str::to_owned);
        f
    }

    fn summary(id: &str, folder: Option<&str>) -> PlaylistSummary {
        PlaylistSummary {
            id: id.into(),
            name: id.into(),
            count: 0,
            smart: false,
            folder_id: folder.map(str::to_owned),
        }
    }

    #[test]
    fn a_folder_lists_what_is_filed_in_it_and_the_top_level_keeps_strays() {
        let folders = Folders {
            all: vec![
                folder("f1", "Moods", None),
                folder("f2", "Rainy", Some("f1")),
            ],
        };
        let lists = [
            summary("top", None),
            summary("in-f1", Some("f1")),
            summary("in-f2", Some("f2")),
            summary("stray", Some("gone")),
        ];
        let ids = |folder: Option<&str>| -> Vec<String> {
            playlists_in(&lists, &folders, folder)
                .iter()
                .map(|p| p.id.clone())
                .collect()
        };
        assert_eq!(ids(None), ["top", "stray"]);
        assert_eq!(ids(Some("f1")), ["in-f1"]);
        assert_eq!(ids(Some("f2")), ["in-f2"]);
        let names: Vec<&str> = folders
            .path(Some("f2"))
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(names, ["Moods", "Rainy"]);
        assert_eq!(
            folders.labelled(),
            [
                ("f1".to_owned(), "Moods".to_owned()),
                ("f2".to_owned(), "Moods \u{203a} Rainy".to_owned())
            ]
        );
    }

    #[test]
    fn deleting_a_folder_moves_its_playlists_up_a_level() {
        let store = aoide_store::Store::in_memory("desk").unwrap();
        let moods = store.create_folder("Moods", None, 1).unwrap();
        let rainy = store.create_folder("Rainy", Some(&moods.id), 2).unwrap();
        let list = store
            .create_playlist("Drizzle", Some(&rainy.id), None, 3)
            .unwrap();
        store.move_playlist(&list.id, Some(&moods.id), 4).unwrap();
        assert_eq!(store.playlists_in_folder(Some(&moods.id)).unwrap().len(), 1);
        store.delete_folder(&moods.id, 5).unwrap();
        assert_eq!(
            store.playlists_in_folder(None).unwrap().len(),
            1,
            "the playlist survived its folder"
        );
        assert_eq!(store.folders_in_parent(None).unwrap()[0].id, rainy.id);
    }
}
