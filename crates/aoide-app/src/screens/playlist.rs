//! One Aoide playlist: its songs in order, Play and Shuffle, rename, delete,
//! and, while editing, its notes and each entry moved up, down or out.

use std::collections::HashMap;

use aoide_jellyfin::Item;
use aoide_store::{PlaySource, Playlist, PlaylistItem};
use iced::widget::{Column, button, column, row, text, text_editor, text_input};
use iced::{Center, Element, Length, Task};

use super::{Ctx, Load, PageMsg, Section, View, fetch};
use crate::actions::PlayRequest;
use crate::app::Message as App;
use crate::icons::{Icon, Tone, icon};
use crate::playback::tracks::TrackMeta;
use crate::theme;
use crate::widgets::{self, ListStyle};

#[derive(Debug)]
pub struct State {
    id: String,
    playlist: Option<Playlist>,
    /// Every live entry, in the playlist's order, those the server no longer
    /// has included: moves are measured against this.
    items: Vec<PlaylistItem>,
    pub tracks: Load<Vec<TrackMeta>>,
    /// The entry behind each listed track, in the same order.
    entries: Vec<String>,
    /// Entries whose track the server no longer has.
    missing: usize,
    renaming: Option<String>,
    confirm_delete: bool,
    error: Option<String>,
    /// Showing the notes editor and each entry's move and remove buttons.
    editing: bool,
    notes: text_editor::Content,
}

const LIST: ListStyle = ListStyle {
    numbered: false,
    show_album: true,
};

/// The playlist row and its entries, as the store has them.
pub type Rows = (Option<Playlist>, Vec<PlaylistItem>);

#[derive(Debug, Clone)]
pub enum Message {
    Rows(Result<Box<Rows>, String>),
    Tracks(Result<Vec<Item>, String>),
    Rename,
    RenameText(String),
    SaveName,
    Delete,
    ConfirmDelete(bool),
    Done(Result<bool, String>),
    Edit(bool),
    Notes(text_editor::Action),
    /// Move this entry one place up (or down) among the listed ones.
    Move {
        entry: String,
        up: bool,
    },
    Remove(String),
}

/// Where `moving` goes to land just before (`up`) or just after `neighbour`,
/// as `Store::move_item` measures it: in the whole list, entries the server
/// no longer has included, with `moving` taken out.
pub fn move_index(all: &[String], moving: &str, neighbour: &str, up: bool) -> Option<usize> {
    let at = all
        .iter()
        .filter(|id| *id != moving)
        .position(|id| id == neighbour)?;
    Some(if up { at } else { at + 1 })
}

pub fn open(ctx: &Ctx<'_>, id: &str) -> (State, Task<App>) {
    let state = State {
        id: id.to_owned(),
        playlist: None,
        items: Vec::new(),
        tracks: Load::Loading,
        entries: Vec::new(),
        missing: 0,
        renaming: None,
        confirm_delete: false,
        error: None,
        editing: false,
        notes: text_editor::Content::new(),
    };
    let task = state.load_rows(ctx);
    (state, task)
}

impl State {
    fn load_rows(&self, ctx: &Ctx<'_>) -> Task<App> {
        let store = ctx.store.clone();
        let id = self.id.clone();
        let key = ctx.key;
        Task::perform(
            async move {
                store
                    .call(move |w| {
                        let read = || -> aoide_store::Result<_> {
                            Ok(Box::new((w.store.playlist(&id)?, w.store.items(&id)?)))
                        };
                        read().map_err(|e| e.to_string())
                    })
                    .await
                    .unwrap_or_else(|| Err("the library is closing".into()))
            },
            move |r| App::Page(key, PageMsg::Playlist(Message::Rows(r))),
        )
    }

    fn write(
        &self,
        ctx: &Ctx<'_>,
        job: impl FnOnce(&aoide_store::Store, &str, i64) -> aoide_store::Result<bool> + Send + 'static,
    ) -> Task<App> {
        let store = ctx.store.clone();
        let id = self.id.clone();
        let key = ctx.key;
        Task::perform(
            async move {
                let now = aoide_store::now_millis();
                store
                    .call(move |w| job(&w.store, &id, now).map_err(|e| e.to_string()))
                    .await
                    .unwrap_or_else(|| Err("the library is closing".into()))
            },
            move |r| App::Page(key, PageMsg::Playlist(Message::Done(r))),
        )
    }

    pub fn update(&mut self, message: Message, ctx: &Ctx<'_>) -> Task<App> {
        match message {
            Message::Rows(Err(error)) => {
                self.tracks = Load::Failed(error);
                Task::none()
            }
            Message::Rows(Ok(rows)) => {
                let (playlist, items) = *rows;
                self.playlist = playlist;
                let ids: Vec<String> = items
                    .iter()
                    .filter(|item| !item.is_unresolved())
                    .map(|item| item.linked_jellyfin_id().to_owned())
                    .collect();
                self.items = items;
                let client = ctx.client.clone();
                fetch(
                    ctx.key,
                    async move { client.items_by_ids(&ids).await },
                    move |r| PageMsg::Playlist(Message::Tracks(r)),
                )
            }
            Message::Tracks(result) => {
                match result {
                    Ok(found) => {
                        let found: HashMap<String, TrackMeta> = found
                            .iter()
                            .map(|item| (item.id.clone(), TrackMeta::from_item(item)))
                            .collect();
                        let (entries, tracks): (Vec<String>, Vec<TrackMeta>) = self
                            .items
                            .iter()
                            .filter(|item| !item.is_unresolved())
                            .filter_map(|item| {
                                let track = found.get(item.linked_jellyfin_id())?;
                                Some((item.id.clone(), track.clone()))
                            })
                            .unzip();
                        self.missing = self.items.len() - entries.len();
                        self.entries = entries;
                        self.tracks = Load::Ready(tracks);
                    }
                    Err(error) => self.tracks = Load::Failed(error),
                }
                Task::none()
            }
            Message::Edit(on) => {
                if on {
                    let notes = self.playlist.as_ref().and_then(|p| p.notes.as_deref());
                    self.notes = text_editor::Content::with_text(notes.unwrap_or_default());
                    self.editing = true;
                    return Task::none();
                }
                self.editing = false;
                let text = self.notes.text();
                let notes = Some(text.trim().to_owned()).filter(|n| !n.is_empty());
                let Some(playlist) = self.playlist.as_mut() else {
                    return Task::none();
                };
                if playlist.notes == notes {
                    return Task::none();
                }
                playlist.notes.clone_from(&notes);
                self.write(ctx, move |store, id, now| {
                    store.set_notes(notes.as_deref(), id, now).map(|_| false)
                })
            }
            Message::Notes(action) => {
                self.notes.perform(action);
                Task::none()
            }
            Message::Move { entry, up } => {
                let Some(at) = self.entries.iter().position(|e| *e == entry) else {
                    return Task::none();
                };
                let Some(other) = (if up { at.checked_sub(1) } else { Some(at + 1) })
                    .filter(|&i| i < self.entries.len())
                else {
                    return Task::none();
                };
                let all: Vec<String> = self.items.iter().map(|i| i.id.clone()).collect();
                let Some(index) = move_index(&all, &entry, &self.entries[other], up) else {
                    return Task::none();
                };
                // Shown at once; the store writes the one entry that moved.
                self.entries.swap(at, other);
                if let Load::Ready(tracks) = &mut self.tracks {
                    tracks.swap(at, other);
                }
                if let Some(from) = self.items.iter().position(|i| i.id == entry) {
                    let item = self.items.remove(from);
                    self.items.insert(index.min(self.items.len()), item);
                }
                self.write(ctx, move |store, id, now| {
                    store.move_item(&entry, id, index, now).map(|_| false)
                })
            }
            Message::Remove(entry) => {
                let Some(at) = self.entries.iter().position(|e| *e == entry) else {
                    return Task::none();
                };
                self.entries.remove(at);
                if let Load::Ready(tracks) = &mut self.tracks {
                    tracks.remove(at);
                }
                self.items.retain(|i| i.id != entry);
                self.write(ctx, move |store, _, now| {
                    store.remove_item(&entry, now).map(|()| false)
                })
            }
            Message::Rename => {
                self.renaming = self.playlist.as_ref().map(|p| p.name.clone());
                Task::none()
            }
            Message::RenameText(name) => {
                self.renaming = Some(name);
                Task::none()
            }
            Message::SaveName => {
                let Some(name) = self.renaming.take().map(|n| n.trim().to_owned()) else {
                    return Task::none();
                };
                if name.is_empty() {
                    return Task::none();
                }
                if let Some(playlist) = self.playlist.as_mut() {
                    playlist.name = name.clone();
                }
                self.write(ctx, move |store, id, now| {
                    store.rename_playlist(id, &name, now).map(|_| false)
                })
            }
            Message::Delete => {
                self.confirm_delete = true;
                Task::none()
            }
            Message::ConfirmDelete(false) => {
                self.confirm_delete = false;
                Task::none()
            }
            Message::ConfirmDelete(true) => {
                self.confirm_delete = false;
                self.write(ctx, |store, id, now| {
                    store.delete_playlist(id, now).map(|()| true)
                })
            }
            Message::Done(Ok(deleted)) => {
                let changed = Task::done(App::PlaylistsChanged);
                if deleted {
                    changed.chain(Task::done(App::Section(Section::Playlists)))
                } else {
                    changed
                }
            }
            Message::Done(Err(error)) => {
                // What is shown may no longer be what is stored: read it again.
                self.error = Some(error);
                self.load_rows(ctx)
            }
        }
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let message = move |m: Message| App::Page(key, PageMsg::Playlist(m));
        let name = self
            .playlist
            .as_ref()
            .map_or("Playlist", |p| p.name.as_str());
        let tracks: &[TrackMeta] = self.tracks.ready().map(Vec::as_slice).unwrap_or(&[]);

        let title: Element<'a, App> = match &self.renaming {
            Some(draft) => row![
                text_input("Playlist name", draft)
                    .on_input(move |s| message(Message::RenameText(s)))
                    .on_submit(message(Message::SaveName))
                    .padding(8)
                    .size(20)
                    .style(theme::field),
                button("Save")
                    .style(theme::filled)
                    .on_press(message(Message::SaveName)),
            ]
            .spacing(10)
            .align_y(Center)
            .into(),
            None => widgets::title_bar(name, view.can_go_back),
        };
        let mut body = column![title].spacing(16);
        if self.editing {
            body = body.push(
                text_editor(&self.notes)
                    .placeholder("Notes")
                    .on_action(move |a| message(Message::Notes(a)))
                    .height(Length::Shrink)
                    .padding(8),
            );
        } else if let Some(notes) = self.playlist.as_ref().and_then(|p| p.notes.as_deref()) {
            body = body.push(widgets::secondary(notes));
        }
        let mut tools = row![widgets::play_buttons(
            tracks,
            format!("{key}:playlist"),
            PlaySource::Playlist
        )]
        .spacing(10)
        .align_y(Center);
        if self.renaming.is_none() {
            tools = tools.push(
                button(text("Rename"))
                    .style(theme::tonal)
                    .padding([8, 14])
                    .on_press(message(Message::Rename)),
            );
        }
        tools = tools.push(if self.editing {
            button(text("Done"))
                .style(theme::filled)
                .padding([8, 14])
                .on_press(message(Message::Edit(false)))
        } else {
            button(text("Edit"))
                .style(theme::tonal)
                .padding([8, 14])
                .on_press(message(Message::Edit(true)))
        });
        tools = tools.push(if self.confirm_delete {
            Element::from(
                row![
                    text("Delete this playlist?"),
                    button("Delete")
                        .style(theme::destructive)
                        .padding([8, 14])
                        .on_press(message(Message::ConfirmDelete(true))),
                    button("Cancel")
                        .style(theme::tonal)
                        .padding([8, 14])
                        .on_press(message(Message::ConfirmDelete(false))),
                ]
                .spacing(8)
                .align_y(Center),
            )
        } else {
            button(text("Delete"))
                .style(theme::destructive)
                .padding([8, 14])
                .on_press(message(Message::Delete))
                .into()
        });
        body = body.push(tools);
        if let Some(menu) = view.menu.filter(|m| m.anchor == format!("{key}:playlist")) {
            body = body.push(widgets::menu_strip(menu, view));
        }
        if let Some(error) = &self.error {
            body = body.push(widgets::failure(error));
        }
        if self.playlist.as_ref().is_some_and(|p| p.is_smart) {
            body = body.push(widgets::empty(
                "A smart playlist. Its rules are kept and synced; showing what they match comes with Round 2.",
            ));
        }
        if self.missing > 0 {
            body = body.push(widgets::secondary(format!(
                "{} {} no longer on the server.",
                self.missing,
                if self.missing == 1 {
                    "song is"
                } else {
                    "songs are"
                }
            )));
        }
        body = body.push(match &self.tracks {
            Load::Loading => widgets::loading(),
            Load::Failed(error) => widgets::failure(error),
            Load::Ready(tracks) if tracks.is_empty() => {
                widgets::empty("No songs yet. Add some from any song's … menu.")
            }
            Load::Ready(tracks) if self.editing => self.editable(tracks, view),
            Load::Ready(tracks) => widgets::track_list(tracks, view, LIST, PlaySource::Playlist),
        });
        crate::handoff::played_from(widgets::page(body), self.context())
    }

    /// What playing from this page is remembered as: the playlist, drawn by
    /// its first track's cover.
    fn context(&self) -> Option<aoide_store::RecentContext> {
        let playlist = self.playlist.as_ref()?;
        let mut context = aoide_store::RecentContext::new(
            aoide_store::RecentContextKind::LocalPlaylist,
            playlist.id.clone(),
            playlist.name.clone(),
            aoide_store::now_millis(),
        );
        context.smart = Some(playlist.is_smart);
        context.artwork_item_id = playlist.artwork_item_id.clone().or_else(|| {
            self.tracks
                .ready()
                .and_then(|t| t.first())
                .and_then(|t| t.album_id.clone().or_else(|| Some(t.id.clone())))
        });
        Some(context)
    }

    /// The songs with each entry's Up, Down and Remove.
    fn editable<'a>(&'a self, tracks: &'a [TrackMeta], view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let message = move |m: Message| App::Page(key, PageMsg::Playlist(m));
        let small = |glyph: Icon, label: Option<App>| {
            button(icon(glyph, 16.0, Tone::Secondary))
                .padding(6)
                .style(theme::row)
                .on_press_maybe(label)
        };
        let mut list = Column::with_capacity(tracks.len()).spacing(2);
        for (index, (track, entry)) in tracks.iter().zip(&self.entries).enumerate() {
            let anchor = format!("{key}:{index}");
            let play = move || App::Play(PlayRequest::from(tracks, index, PlaySource::Playlist));
            let up = (index > 0).then(|| {
                message(Message::Move {
                    entry: entry.clone(),
                    up: true,
                })
            });
            let down = (index + 1 < tracks.len()).then(|| {
                message(Message::Move {
                    entry: entry.clone(),
                    up: false,
                })
            });
            list = list.push(
                row![
                    widgets::track_row(track, &anchor, view, LIST, play),
                    small(Icon::Up, up),
                    small(Icon::Down, down),
                    small(Icon::Remove, Some(message(Message::Remove(entry.clone())))),
                ]
                .spacing(2)
                .align_y(Center),
            );
            if let Some(menu) = view.menu.filter(|m| m.anchor == anchor) {
                list = list.push(widgets::menu_strip(menu, view));
            }
        }
        list.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    #[test]
    fn a_step_lands_beside_its_listed_neighbour_past_missing_entries() {
        // "x" is an entry the server no longer has; it is not listed.
        let all = ids(&["a", "x", "b", "c"]);
        assert_eq!(move_index(&all, "b", "a", true), Some(0));
        assert_eq!(move_index(&all, "a", "b", false), Some(2));
        assert_eq!(move_index(&all, "c", "b", true), Some(2));
        assert_eq!(move_index(&all, "b", "c", false), Some(3));
        assert_eq!(move_index(&all, "b", "gone", true), None);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_move_writes_one_entry_and_lands_where_shown() {
        let store = crate::store_worker::StoreHandle::in_memory("device-1").unwrap();
        store
            .call(|w| {
                let list = w.store.create_playlist("Set", None, None, 1).unwrap();
                let link = |id: &str| aoide_store::TrackLink::new(id.to_owned(), format!("k-{id}"));
                let added = w
                    .store
                    .add_tracks(&["a", "x", "b", "c"].map(link), &list.id, 1)
                    .unwrap();
                let all: Vec<String> = added.iter().map(|i| i.id.clone()).collect();
                let before = w.store.pending_op_count().unwrap();
                // "c" steps up past "b"; "x" (not listed) stays between a and b.
                let index = move_index(&all, &all[3], &all[2], true).unwrap();
                w.store.move_item(&all[3], &list.id, index, 2).unwrap();
                assert_eq!(w.store.pending_op_count().unwrap(), before + 1);
                let order: Vec<String> = w
                    .store
                    .items(&list.id)
                    .unwrap()
                    .into_iter()
                    .map(|i| i.jellyfin_id)
                    .collect();
                assert_eq!(order, ["a", "x", "c", "b"]);
            })
            .await
            .unwrap();
    }
}
