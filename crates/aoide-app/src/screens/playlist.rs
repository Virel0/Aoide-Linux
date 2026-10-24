//! One Aoide playlist: its songs in order, Play and Shuffle, rename, delete.

use aoide_jellyfin::Item;
use aoide_store::{PlaySource, Playlist, PlaylistItem};
use iced::widget::{button, column, row, text, text_input};
use iced::{Center, Element, Task};

use super::{Ctx, Load, PageMsg, Section, View, fetch};
use crate::app::Message as App;
use crate::playback::tracks::TrackMeta;
use crate::theme;
use crate::widgets::{self, ListStyle};

#[derive(Debug)]
pub struct State {
    id: String,
    playlist: Option<Playlist>,
    pub tracks: Load<Vec<TrackMeta>>,
    /// Entries whose track the server no longer has.
    missing: usize,
    renaming: Option<String>,
    confirm_delete: bool,
    error: Option<String>,
}

/// The playlist row and its entries, as the store has them.
pub type Rows = (Option<Playlist>, Vec<PlaylistItem>);

#[derive(Debug, Clone)]
pub enum Message {
    Rows(Result<Box<Rows>, String>),
    Tracks(usize, Result<Vec<Item>, String>),
    Rename,
    RenameText(String),
    SaveName,
    Delete,
    ConfirmDelete(bool),
    Done(Result<bool, String>),
}

pub fn open(ctx: &Ctx<'_>, id: &str) -> (State, Task<App>) {
    let state = State {
        id: id.to_owned(),
        playlist: None,
        tracks: Load::Loading,
        missing: 0,
        renaming: None,
        confirm_delete: false,
        error: None,
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
                let asked = ids.len();
                self.missing = items.len() - asked;
                let client = ctx.client.clone();
                fetch(
                    ctx.key,
                    async move { client.items_by_ids(&ids).await },
                    move |r| PageMsg::Playlist(Message::Tracks(asked, r)),
                )
            }
            Message::Tracks(asked, result) => {
                if let Ok(items) = &result {
                    self.missing += asked.saturating_sub(items.len());
                }
                self.tracks = Load::from(
                    result.map(|items| items.iter().map(TrackMeta::from_item).collect()),
                );
                Task::none()
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
                self.error = Some(error);
                Task::none()
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
        if let Some(notes) = self.playlist.as_ref().and_then(|p| p.notes.as_deref()) {
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
            Load::Ready(tracks) => widgets::track_list(
                tracks,
                view,
                ListStyle {
                    numbered: false,
                    show_album: true,
                },
                PlaySource::Playlist,
            ),
        });
        widgets::page(body)
    }
}
