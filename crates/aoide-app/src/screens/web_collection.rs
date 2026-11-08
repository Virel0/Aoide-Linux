//! Collections made in Melete's web (Melete 0.8.0): each a saved song filter
//! kept on the server, listed in the sidebar under this computer's own
//! collections. Its page lists the songs the filter finds now, with Play and
//! Shuffle, and can delete it from the server. They are made and changed in
//! the web; a server before collections has none, and lists nothing.

use aoide_melete::{COLLECTION_PAGE_MAX, Collection, MeleteClient, Page};
use iced::Task;

use super::{Ctx, Load, PageMsg, Section, fetch};
use crate::app::{App, Message as AppMessage};
use crate::artwork::ArtKey;
use crate::failure::Failure;
use crate::playback::tracks::TrackMeta;

mod view;
mod words;

/// The app's side: the signed-in user's collections, as last listed.
#[derive(Debug, Clone)]
pub enum Msg {
    Listed(Result<Vec<Collection>, String>),
    /// The page with this key shows the collection with this id.
    Show {
        key: u64,
        id: i64,
    },
    /// The collection with this id was deleted on the server.
    Deleted(i64),
}

/// Lists the signed-in user's collections: at sign-in and after each sync,
/// so one made in the web shows up here.
pub fn ask(client: MeleteClient) -> Task<AppMessage> {
    Task::perform(async move { client.collections().await }, |answer| {
        AppMessage::WebCollections(Msg::Listed(answer.map_err(|e| e.to_string())))
    })
}

#[derive(Debug)]
pub struct State {
    id: i64,
    collection: Option<Collection>,
    songs: Load<Vec<TrackMeta>>,
    /// How many songs the filter finds in all; more than are listed past
    /// one page.
    total: u32,
    confirm_delete: bool,
    deleting: bool,
    /// Why the last delete didn't happen.
    delete_failed: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// The collection, or `None` when it's gone.
    Found(Option<Collection>),
    Songs(Result<Page<aoide_melete::Item>, Failure>),
    Delete,
    ConfirmDelete(bool),
    Deleted(Result<(), Failure>),
}

pub fn open(ctx: &Ctx<'_>, id: i64) -> (State, Task<AppMessage>) {
    let state = State {
        id,
        collection: None,
        songs: Load::Loading,
        total: 0,
        confirm_delete: false,
        deleting: false,
        delete_failed: None,
    };
    let task = Task::done(AppMessage::WebCollections(Msg::Show { key: ctx.key, id }));
    (state, task)
}

fn wrap(message: Message) -> PageMsg {
    PageMsg::WebCollection(message)
}

impl State {
    pub fn update(&mut self, message: Message, ctx: &Ctx<'_>) -> Task<AppMessage> {
        match message {
            Message::Found(None) => {
                self.songs = Load::Failed("That collection has been deleted.".into());
                Task::none()
            }
            Message::Found(Some(collection)) => {
                let client = ctx.client.clone();
                let asked = collection.clone();
                self.collection = Some(collection);
                self.songs = Load::Loading;
                fetch(
                    ctx.key,
                    async move {
                        client
                            .collection_songs(&asked, 0, COLLECTION_PAGE_MAX)
                            .await
                    },
                    |r| wrap(Message::Songs(r)),
                )
            }
            Message::Songs(result) => {
                self.total = result.as_ref().map_or(0, |page| page.total);
                self.songs = Load::from(
                    result.map(|page| page.items.iter().map(TrackMeta::from_item).collect()),
                );
                Task::none()
            }
            Message::Delete => {
                self.confirm_delete = true;
                self.delete_failed = None;
                Task::none()
            }
            Message::ConfirmDelete(false) => {
                self.confirm_delete = false;
                Task::none()
            }
            Message::ConfirmDelete(true) => self.delete(ctx),
            Message::Deleted(result) => self.deleted(result),
        }
    }

    fn delete(&mut self, ctx: &Ctx<'_>) -> Task<AppMessage> {
        if self.deleting {
            return Task::none();
        }
        self.deleting = true;
        let client = ctx.client.clone();
        let id = self.id;
        fetch(
            ctx.key,
            async move {
                match client.delete_collection(id).await {
                    // Gone already, from another device: as good as deleted.
                    Err(aoide_melete::Error::NotFound { .. }) => Ok(()),
                    other => other,
                }
            },
            |r| wrap(Message::Deleted(r)),
        )
    }

    fn deleted(&mut self, result: Result<(), Failure>) -> Task<AppMessage> {
        self.deleting = false;
        self.confirm_delete = false;
        match result {
            Ok(()) => Task::done(AppMessage::WebCollections(Msg::Deleted(self.id))),
            Err(failure) => {
                self.delete_failed = Some(failure.summary);
                Task::none()
            }
        }
    }

    pub fn artwork(&self) -> Vec<ArtKey> {
        self.songs
            .ready()
            .map(|t| super::track_art(t))
            .unwrap_or_default()
    }
}

impl App {
    pub(crate) fn web_collections(&mut self, message: Msg) -> Task<AppMessage> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        match message {
            Msg::Listed(Ok(list)) => {
                signed.web_collections = list;
                Task::none()
            }
            // Kept as they were; asked again after the next sync.
            Msg::Listed(Err(error)) => {
                log::warn!("The server didn't list this account's collections: {error}");
                Task::none()
            }
            Msg::Show { key, id } => {
                let found = signed.web_collections.iter().find(|c| c.id == id).cloned();
                Task::done(AppMessage::Page(key, wrap(Message::Found(found))))
            }
            Msg::Deleted(id) => {
                signed.web_collections.retain(|c| c.id != id);
                Task::done(AppMessage::Section(Section::Home))
            }
        }
    }
}
