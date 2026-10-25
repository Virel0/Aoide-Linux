//! Aoide playlists: the ones kept on this computer and synced through the
//! sidecar, filed in folders. The page shows one folder at a time (the top
//! level first), with its folders above its playlists; a folder can be made,
//! renamed and deleted (its contents move up a level, nothing is lost), and a
//! playlist moved into another folder.

use std::collections::{HashMap, HashSet};

use aoide_store::Folder;
use iced::widget::{Column, button, column, container, row, text, text_input};
use iced::{Center, Element, Fill, Task};

use super::{Ctx, Load, PageMsg, PlaylistSummary, Route, View};
use crate::actions::{Collection, RowAction};
use crate::app::Message as App;
use crate::icons::{Icon, Tone, icon};
use crate::theme;
use crate::widgets;

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
            Message::Create => {
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
            Message::FindCovers => find_covers(ctx.key),
            Message::Covers(covers) => {
                self.covers = covers;
                Task::none()
            }
            Message::Created(result) => match result {
                Ok(_) => {
                    self.error = None;
                    Task::done(App::PlaylistsChanged)
                }
                Err(error) => {
                    self.error = Some(format!("The playlist couldn't be made: {error}"));
                    Task::none()
                }
            },
            Message::Folders(result) => {
                self.folders = Load::from(result.map(|all| Folders { all }));
                // The folder shown may have gone (deleted, here or elsewhere).
                if let (Some(shown), Load::Ready(folders)) = (&self.folder, &self.folders)
                    && folders.get(shown).is_none()
                {
                    self.folder = None;
                }
                Task::none()
            }
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
            Message::CreateFolder => {
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
            Message::Rename(draft) => {
                self.renaming = draft;
                Task::none()
            }
            Message::SaveRename => {
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
            Message::MoveTo { playlist, folder } => {
                self.moving = None;
                change(ctx, move |store, now| {
                    store
                        .move_playlist(&playlist, folder.as_deref(), now)
                        .map(drop)
                })
            }
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

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let message = move |m: Message| App::Page(key, PageMsg::Playlists(m));
        let folders: &Folders = match &self.folders {
            Load::Ready(folders) => folders,
            _ => &NO_FOLDERS,
        };
        let shown = self.folder.as_deref();

        let create = row![
            text_input("New playlist name", &self.new_name)
                .on_input(move |s| message(Message::NewName(s)))
                .on_submit(message(Message::Create))
                .padding(8)
                .style(theme::field),
            button(text("Import…"))
                .style(theme::tonal)
                .padding([8, 14])
                .on_press(App::Open(Route::Import)),
            button(text("New folder…"))
                .style(theme::tonal)
                .padding([8, 14])
                .on_press(message(Message::NewFolder(Some(String::new())))),
            button(text("New playlist"))
                .style(theme::filled)
                .padding([8, 14])
                .on_press_maybe(
                    (!self.new_name.trim().is_empty()).then(|| message(Message::Create))
                ),
        ]
        .spacing(10)
        .align_y(Center);
        let mut body = column![
            super::jellyfin_playlists::header(false, view.can_go_back),
            create
        ]
        .spacing(20);

        // Where we are: All Playlists › Folder › Folder, each a way back up.
        if shown.is_some() {
            let mut path = row![
                button(text("All playlists").size(14))
                    .style(theme::row)
                    .padding([4, 6])
                    .on_press(message(Message::Enter(None)))
            ]
            .spacing(4)
            .align_y(Center);
            for folder in folders.path(shown) {
                path = path.push(widgets::secondary("\u{203a}"));
                path = path.push(
                    button(text(folder.name.as_str()).size(14))
                        .style(theme::row)
                        .padding([4, 6])
                        .on_press(message(Message::Enter(Some(folder.id.clone())))),
                );
            }
            body = body.push(path);
        }
        if let Some(draft) = &self.new_folder {
            body = body.push(
                row![
                    text_input("Folder name", draft)
                        .on_input(move |s| message(Message::NewFolder(Some(s))))
                        .on_submit(message(Message::CreateFolder))
                        .padding(8)
                        .style(theme::field),
                    button(text("Make folder"))
                        .style(theme::filled)
                        .padding([8, 14])
                        .on_press_maybe(
                            (!draft.trim().is_empty()).then(|| message(Message::CreateFolder))
                        ),
                    button(text("Cancel"))
                        .style(theme::tonal)
                        .padding([8, 14])
                        .on_press(message(Message::NewFolder(None))),
                ]
                .spacing(10)
                .align_y(Center),
            );
        }
        if let Some(error) = &self.error {
            body = body.push(widgets::plain_failure(error));
        }

        let children = folders.children(shown);
        let lists = playlists_in(view.playlists, folders, shown);
        if children.is_empty() && lists.is_empty() {
            body = body.push(widgets::empty(if shown.is_some() {
                "This folder is empty. Move a playlist here with its Move… button, or make one here."
            } else {
                "No playlists yet. Make one here, or add songs to a new one from any song's … menu."
            }));
        }

        let mut list = Column::new().spacing(2);
        for folder in children {
            list = list.push(self.folder_row(folder, view.playlists, folders, key));
        }
        for playlist in lists {
            list = list.push(self.playlist_row(playlist, folders, view, key));
            if self.moving.as_deref() == Some(playlist.id.as_str()) {
                list = list.push(move_to(playlist, folders, key));
            }
        }
        widgets::page(body.push(list))
    }

    fn folder_row<'a>(
        &'a self,
        folder: &'a Folder,
        playlists: &[PlaylistSummary],
        folders: &Folders,
        key: u64,
    ) -> Element<'a, App> {
        let message = move |m: Message| App::Page(key, PageMsg::Playlists(m));
        if let Some((id, draft)) = self.renaming.as_ref().filter(|(id, _)| *id == folder.id) {
            return row![
                text_input("Folder name", draft)
                    .on_input(move |s| message(Message::Rename(Some((id.clone(), s)))))
                    .on_submit(message(Message::SaveRename))
                    .padding(8)
                    .style(theme::field),
                button(text("Save"))
                    .style(theme::filled)
                    .padding([8, 14])
                    .on_press(message(Message::SaveRename)),
                button(text("Cancel"))
                    .style(theme::tonal)
                    .padding([8, 14])
                    .on_press(message(Message::Rename(None))),
            ]
            .spacing(10)
            .align_y(Center)
            .into();
        }
        let inside = playlists_in(playlists, folders, Some(&folder.id)).len()
            + folders.children(Some(&folder.id)).len();
        let count = match inside {
            0 => "Empty".to_owned(),
            1 => "1 item".to_owned(),
            n => format!("{n} items"),
        };
        let small = |label: &'a str, m: Message| {
            button(text(label).size(13))
                .style(theme::row)
                .padding([4, 8])
                .on_press(message(m))
        };
        let mut line = row![
            container(icon(Icon::Folder, 22.0, Tone::Accent))
                .width(36)
                .align_x(Center),
            text(folder.name.as_str()).size(15).width(Fill),
            widgets::secondary(count).size(13),
        ]
        .spacing(12)
        .align_y(Center);
        if self.confirm_delete.as_deref() == Some(folder.id.as_str()) {
            line = line
                .push(text("Delete this folder? What's in it moves up a level.").size(13))
                .push(
                    button(text("Delete").size(13))
                        .style(theme::destructive)
                        .padding([4, 10])
                        .on_press(message(Message::ConfirmDelete(folder.id.clone()))),
                )
                .push(small("Cancel", Message::Delete(None)));
        } else {
            line = line
                .push(small(
                    "Rename",
                    Message::Rename(Some((folder.id.clone(), folder.name.clone()))),
                ))
                .push(small("Delete", Message::Delete(Some(folder.id.clone()))));
        }
        button(line)
            .width(Fill)
            .padding([8, 10])
            .style(theme::row)
            .on_press(message(Message::Enter(Some(folder.id.clone()))))
            .into()
    }

    fn playlist_row<'a>(
        &'a self,
        playlist: &'a PlaylistSummary,
        folders: &Folders,
        view: &View<'a>,
        key: u64,
    ) -> Element<'a, App> {
        let message = move |m: Message| App::Page(key, PageMsg::Playlists(m));
        let count = match (playlist.smart, playlist.count) {
            (true, 1) => "Smart · 1 song".to_owned(),
            (true, n) => format!("Smart · {n} songs"),
            (false, 1) => "1 song".to_owned(),
            (false, n) => format!("{n} songs"),
        };
        let glyph = if playlist.smart {
            Icon::Sparkle
        } else {
            Icon::Playlist
        };
        let more = button(icon(Icon::More, 18.0, Tone::Secondary))
            .style(theme::row)
            .padding(6)
            .on_press(App::Row(RowAction::OpenCollection {
                anchor: format!("playlist:{}", playlist.id),
                collection: Collection::Playlist(playlist.id.clone()),
                title: playlist.name.clone(),
            }));
        let open = self.moving.as_deref() == Some(playlist.id.as_str());
        let mut line = row![
            match self.covers.get(&playlist.id) {
                Some(cover) =>
                    widgets::art(Some(cover), view.artwork, iced::Length::Fixed(36.0), 4.0),
                None => container(icon(glyph, 22.0, Tone::Accent))
                    .width(36)
                    .align_x(Center)
                    .into(),
            },
            text(playlist.name.as_str()).size(15).width(Fill),
            widgets::secondary(count).size(13),
        ]
        .spacing(12)
        .align_y(Center);
        // Somewhere to move it: another folder, or out to the top level.
        if !folders.all.is_empty() || playlist.folder_id.is_some() {
            line = line.push(
                button(text(if open { "Cancel" } else { "Move…" }).size(13))
                    .style(theme::row)
                    .padding([4, 8])
                    .on_press(message(Message::Moving(
                        (!open).then(|| playlist.id.clone()),
                    ))),
            );
        }
        line = line.push(more);
        button(line)
            .width(Fill)
            .padding([8, 10])
            .style(theme::row)
            .on_press(App::Open(Route::Playlist {
                id: playlist.id.clone(),
            }))
            .into()
    }
}

static NO_FOLDERS: Folders = Folders { all: Vec::new() };

/// The places a playlist can move to: the top level, and every folder but
/// its own.
fn move_to<'a>(playlist: &'a PlaylistSummary, folders: &Folders, key: u64) -> Element<'a, App> {
    let message = move |m: Message| App::Page(key, PageMsg::Playlists(m));
    let mut places = Column::new().spacing(2).push(
        widgets::secondary("Move to")
            .size(12)
            .font(widgets::SEMIBOLD),
    );
    if playlist.folder_id.is_some() {
        places = places.push(
            button(text("Top level").size(13))
                .width(Fill)
                .style(theme::row)
                .padding([4, 8])
                .on_press(message(Message::MoveTo {
                    playlist: playlist.id.clone(),
                    folder: None,
                })),
        );
    }
    for (id, label) in folders.labelled() {
        if playlist.folder_id.as_deref() == Some(id.as_str()) {
            continue;
        }
        places = places.push(
            button(text(label).size(13))
                .width(Fill)
                .style(theme::row)
                .padding([4, 8])
                .on_press(message(Message::MoveTo {
                    playlist: playlist.id.clone(),
                    folder: Some(id),
                })),
        );
    }
    container(places)
        .padding(10)
        .width(Fill)
        .style(theme::menu)
        .into()
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
