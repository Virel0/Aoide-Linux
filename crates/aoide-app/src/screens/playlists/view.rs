//! The playlists page as drawn: New Playlist and New Folder, the path to
//! the folder shown, its folders and playlists, and a playlist's Move list.

use aoide_store::Folder;
use iced::widget::{Column, Row, button, column, container, row, text, text_input};
use iced::{Center, Element, Fill};

use super::{Folders, Message, State, playlists_in};
use crate::actions::{Collection, RowAction};
use crate::app::Message as App;
use crate::icons::{Icon, Tone, icon};
use crate::screens::{Load, PageMsg, PlaylistSummary, Route, View};
use crate::theme;
use crate::widgets;

impl State {
    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let folders: &Folders = match &self.folders {
            Load::Ready(folders) => folders,
            _ => &NO_FOLDERS,
        };
        let shown = self.folder.as_deref();
        let mut body = column![
            widgets::title_bar("Playlists", view.can_go_back),
            self.create_row(key)
        ]
        .spacing(20);
        if shown.is_some() {
            body = body.push(path_bar(folders, shown, key));
        }
        if let Some(draft) = &self.new_folder {
            body = body.push(new_folder_row(draft, key));
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

    /// The new playlist's name, Import, New Folder and New Playlist.
    fn create_row(&self, key: u64) -> Row<'_, App> {
        let message = move |m: Message| App::Page(key, PageMsg::Playlists(m));
        row![
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
        .align_y(Center)
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
            return rename_folder_row(id, draft, key);
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

/// Where we are: All Playlists › Folder › Folder, each a way back up.
fn path_bar<'a>(folders: &'a Folders, shown: Option<&str>, key: u64) -> Row<'a, App> {
    let message = move |m: Message| App::Page(key, PageMsg::Playlists(m));
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
    path
}

/// New Folder's name field, Make Folder and Cancel.
fn new_folder_row(draft: &str, key: u64) -> Row<'_, App> {
    let message = move |m: Message| App::Page(key, PageMsg::Playlists(m));
    row![
        text_input("Folder name", draft)
            .on_input(move |s| message(Message::NewFolder(Some(s))))
            .on_submit(message(Message::CreateFolder))
            .padding(8)
            .style(theme::field),
        button(text("Make folder"))
            .style(theme::filled)
            .padding([8, 14])
            .on_press_maybe((!draft.trim().is_empty()).then(|| message(Message::CreateFolder))),
        button(text("Cancel"))
            .style(theme::tonal)
            .padding([8, 14])
            .on_press(message(Message::NewFolder(None))),
    ]
    .spacing(10)
    .align_y(Center)
}

/// A folder's row while it is renamed: the name field, Save and Cancel.
fn rename_folder_row<'a>(id: &'a str, draft: &'a str, key: u64) -> Element<'a, App> {
    let message = move |m: Message| App::Page(key, PageMsg::Playlists(m));
    row![
        text_input("Folder name", draft)
            .on_input(move |s| message(Message::Rename(Some((id.to_owned(), s)))))
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
    .into()
}

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
