//! One Aoide playlist: its songs in order, Play and Shuffle, rename, delete,
//! and, while editing, its notes and each entry moved up, down or out.
//!
//! A smart playlist is its rules, evaluated against the local history when
//! the page opens (`Store::resolve_smart`): its songs are listed in the
//! rules' order and cannot be moved or removed one by one.

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
    /// For a smart playlist: what its rules matched when the page opened.
    smart: Option<Vec<String>>,
    /// The cover as drawn (`covers`): a chosen picture, an item, or the
    /// first track's album.
    cover: Option<crate::artwork::ArtKey>,
    /// What the last cover change said: done, or why not.
    cover_note: Option<Result<String, String>>,
}

const LIST: ListStyle = ListStyle {
    numbered: false,
    show_album: true,
};

/// The playlist row and its entries, as the store has them, and for a smart
/// playlist the ids its rules match now (or why they could not be read).
pub type Rows = (
    Option<Playlist>,
    Vec<PlaylistItem>,
    Option<Result<Vec<String>, String>>,
);

/// Reads a playlist for its page, evaluating a smart one's rules as of `now`
/// on the local calendar.
pub fn read_rows(store: &aoide_store::Store, id: &str, now: i64) -> aoide_store::Result<Rows> {
    let playlist = store.playlist(id)?;
    let items = store.items(id)?;
    let smart = playlist.as_ref().filter(|p| p.is_smart).map(|p| {
        store
            .resolve_smart(p, now, aoide_store::Calendar::Local)
            .map_err(|e| e.to_string())
    });
    Ok((playlist, items, smart))
}

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
    /// Ask for the cover to draw.
    FindCover,
    Cover(Option<crate::artwork::ArtKey>),
    ChooseCover,
    RemoveCover,
    CoverNote(Result<String, String>),
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
        smart: None,
        cover: None,
        cover_note: None,
    };
    let task = state.load_rows(ctx);
    (state, task)
}

impl State {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The songs' covers and the playlist's own.
    pub fn artwork(&self) -> Vec<crate::artwork::ArtKey> {
        let mut keys: Vec<_> = self.cover.clone().into_iter().collect();
        if let Load::Ready(tracks) = &self.tracks {
            keys.extend(super::track_art(tracks));
        }
        keys
    }

    fn load_rows(&self, ctx: &Ctx<'_>) -> Task<App> {
        let store = ctx.store.clone();
        let id = self.id.clone();
        let key = ctx.key;
        Task::perform(
            async move {
                store
                    .call(move |w| {
                        read_rows(&w.store, &id, aoide_store::now_millis())
                            .map(Box::new)
                            .map_err(|e| e.to_string())
                    })
                    .await
                    .unwrap_or_else(|| Err("the library is closing".into()))
            },
            move |r| App::Page(key, PageMsg::Playlist(Message::Rows(r))),
        )
    }

    fn find_cover(&self, key: u64) -> Task<App> {
        Task::done(App::Covers(crate::covers::Msg::Resolve {
            key,
            ids: vec![self.id.clone()],
        }))
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
                let (playlist, items, smart) = *rows;
                self.playlist = playlist;
                let ids: Vec<String> = match smart {
                    Some(Ok(ids)) => {
                        self.smart = Some(ids.clone());
                        ids
                    }
                    Some(Err(error)) => {
                        self.tracks = Load::Failed(format!(
                            "This smart playlist's rules couldn't be read: {error}"
                        ));
                        return Task::none();
                    }
                    None => {
                        self.smart = None;
                        items
                            .iter()
                            .filter(|item| !item.is_unresolved())
                            .map(|item| item.linked_jellyfin_id().to_owned())
                            .collect()
                    }
                };
                self.items = items;
                let client = ctx.client.clone();
                Task::batch([
                    fetch(
                        ctx.key,
                        async move { client.items_by_ids(&ids).await },
                        move |r| PageMsg::Playlist(Message::Tracks(r)),
                    ),
                    self.find_cover(ctx.key),
                ])
            }
            Message::FindCover => self.find_cover(ctx.key),
            Message::Cover(cover) => {
                self.cover = cover;
                Task::none()
            }
            Message::ChooseCover => {
                self.cover_note = None;
                Task::done(App::Covers(crate::covers::Msg::Choose {
                    key: ctx.key,
                    playlist: self.id.clone(),
                }))
            }
            Message::RemoveCover => {
                self.cover_note = None;
                Task::done(App::Covers(crate::covers::Msg::Remove {
                    key: ctx.key,
                    playlist: self.id.clone(),
                }))
            }
            Message::CoverNote(note) => {
                self.cover_note = Some(note);
                // The row changed: read it again, for its hash.
                self.load_rows(ctx)
            }
            Message::Tracks(result) => {
                match (result, &self.smart) {
                    (Ok(found), Some(ids)) => {
                        // The rules' order; nothing to edit entry by entry.
                        let tracks: Vec<TrackMeta> = crate::handoff::ordered(found, ids)
                            .iter()
                            .map(TrackMeta::from_item)
                            .collect();
                        self.missing = ids.len() - tracks.len();
                        self.entries.clear();
                        self.tracks = Load::Ready(tracks);
                    }
                    (Ok(found), None) => {
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
                    (Err(error), _) => self.tracks = Load::Failed(error),
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
        let mut head = column![title].spacing(12);
        if self.editing {
            head = head.push(
                text_editor(&self.notes)
                    .placeholder("Notes")
                    .on_action(move |a| message(Message::Notes(a)))
                    .height(Length::Shrink)
                    .padding(8),
            );
        } else if let Some(notes) = self.playlist.as_ref().and_then(|p| p.notes.as_deref()) {
            head = head.push(widgets::secondary(notes));
        }
        // Plays from a smart playlist are filed as smart, as on the phone.
        let source = if self.smart.is_some() {
            PlaySource::Smart
        } else {
            PlaySource::Playlist
        };
        let mut tools = row![widgets::play_buttons(
            tracks,
            format!("{key}:playlist"),
            source
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
        tools = tools.push(
            button(text("Delete"))
                .style(theme::destructive)
                .padding([8, 14])
                .on_press_maybe((!self.confirm_delete).then(|| message(Message::Delete))),
        );
        // On a line of its own, as on a Jellyfin playlist: beside the other
        // buttons the question runs off a narrow window.
        let confirm = self.confirm_delete.then(|| {
            row![
                text(format!(
                    "Delete \u{201c}{name}\u{201d}? Its songs stay in your library."
                )),
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
            .align_y(Center)
            .wrap()
            .vertical_spacing(8)
        });
        let small = |label: &'a str, m: Option<Message>| {
            button(text(label).size(13))
                .style(theme::tonal)
                .padding([6, 12])
                .on_press_maybe(m.map(message))
        };
        let has_picture = self
            .playlist
            .as_ref()
            .is_some_and(|p| p.image_hash.is_some());
        let share = button(text("Share…").size(13))
            .style(theme::tonal)
            .padding([6, 12])
            .on_press_maybe(self.playlist.as_ref().map(|p| {
                App::Open(super::Route::Share {
                    id: p.id.clone(),
                    name: p.name.clone(),
                })
            }));
        let cover_tools = row![
            small("Choose cover…", Some(Message::ChooseCover)),
            small("Remove cover", has_picture.then_some(Message::RemoveCover)),
            share,
        ]
        .spacing(8);
        // Wrapped: at the narrowest window the buttons don't fit on one line.
        head = head.push(tools.wrap().vertical_spacing(10));
        if let Some(confirm) = confirm {
            head = head.push(confirm);
        }
        head = head.push(cover_tools);
        if let Some(note) = &self.cover_note {
            head = head.push(match note {
                Ok(note) => widgets::secondary(note.as_str()).size(13).into(),
                Err(error) => widgets::failure(error),
            });
        }
        let mut body = column![
            row![
                widgets::art(
                    self.cover.as_ref(),
                    view.artwork,
                    Length::Fixed(180.0),
                    10.0
                ),
                head.width(Length::Fill)
            ]
            .spacing(24)
        ]
        .spacing(16);
        if let Some(menu) = view.menu.filter(|m| m.anchor == format!("{key}:playlist")) {
            body = body.push(widgets::menu_strip(menu, view));
        }
        if let Some(error) = &self.error {
            body = body.push(widgets::failure(error));
        }
        if self.smart.is_some() {
            body = body.push(
                widgets::secondary(
                    "A smart playlist: its songs are whatever its rules match in your listening, \
                 worked out each time it opens, so they can't be moved or removed one by one.",
                )
                .size(13),
            );
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
            Load::Ready(tracks) if tracks.is_empty() && self.smart.is_some() => widgets::empty(
                "Nothing matches its rules yet. Songs appear as your listening fits them.",
            ),
            Load::Ready(tracks) if tracks.is_empty() => {
                widgets::empty("No songs yet. Add some from any song's … menu.")
            }
            Load::Ready(tracks) if self.editing && self.smart.is_none() => {
                self.editable(tracks, view)
            }
            Load::Ready(tracks) => widgets::track_list(tracks, view, LIST, source),
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

    #[test]
    fn a_smart_playlist_is_evaluated_from_the_history_on_open() {
        let store = aoide_store::Store::in_memory("desk").unwrap();
        let now = 1_792_540_800_000;
        let created = store.ensure_built_in_playlists(now).unwrap();
        assert!(!created.is_empty());
        let heavy = aoide_store::BuiltInPlaylist::HeavyRotation;
        // Played often, lately: Heavy Rotation's kind of song.
        let track = aoide_store::TrackRef::new("t1", "k1", "Lantern", "Mara Vey", "Saltwater", now);
        store.cache_tracks(&[track]).unwrap();
        let link = aoide_store::TrackLink::new("t1", "k1");
        for n in 0..6 {
            let at = now - (n + 1) * 3_600_000;
            store
                .record_play(
                    &link,
                    at,
                    180_000,
                    Some(180_000),
                    PlaySource::Album,
                    Some(true),
                )
                .unwrap();
        }
        let (playlist, items, smart) = read_rows(&store, heavy.id(), now).unwrap();
        assert!(playlist.unwrap().is_smart);
        assert!(items.is_empty(), "rules, not entries");
        assert_eq!(smart.unwrap().unwrap(), ["t1"]);
        let plain = store.create_playlist("Mine", None, None, now).unwrap();
        assert!(read_rows(&store, &plain.id, now).unwrap().2.is_none());
    }

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
