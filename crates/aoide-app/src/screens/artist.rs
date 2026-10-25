//! An artist: how much of them is finished, their most played songs, and
//! their albums, newest first.

use std::collections::HashMap;

use aoide_jellyfin::Item;
use aoide_store::PlaySource;
use iced::widget::{column, text};
use iced::{Element, Task};

use super::{Ctx, Load, PageMsg, View, fetch};
use crate::app::Message as App;
use crate::artwork::ArtKey;
use crate::finish;
use crate::library::Tile;
use crate::playback::tracks::TrackMeta;
use crate::widgets;

#[derive(Debug)]
pub struct State {
    id: String,
    name: String,
    pub albums: Load<Vec<Tile>>,
    /// Their most played songs here; empty until there are plays.
    top: Vec<TrackMeta>,
    /// "You finish 78% of this artist's songs", once there is enough history.
    finish: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Albums(Result<Vec<Item>, String>),
    /// Every song on their albums.
    Tracks(Result<Vec<Item>, String>),
    Listening {
        finish: Option<String>,
        top: Vec<TrackMeta>,
    },
}

pub fn open(ctx: &Ctx<'_>, id: &str, name: &str) -> (State, Task<App>) {
    let client = ctx.client.clone();
    let id = id.to_owned();
    let asked = id.clone();
    let task = fetch(
        ctx.key,
        async move { client.artist_albums(&asked).await },
        |r| PageMsg::Artist(Message::Albums(r)),
    );
    (
        State {
            id: id.clone(),
            name: name.to_owned(),
            albums: Load::Loading,
            top: Vec::new(),
            finish: None,
        },
        task,
    )
}

impl State {
    pub fn update(&mut self, message: Message, ctx: &Ctx<'_>) -> Task<App> {
        match message {
            Message::Albums(result) => {
                let ids: Vec<String> = result
                    .as_ref()
                    .map(|items| items.iter().map(|a| a.id.clone()).collect())
                    .unwrap_or_default();
                self.albums =
                    Load::from(result.map(|items| items.iter().map(Tile::album).collect()));
                if ids.is_empty() {
                    return Task::none();
                }
                let client = ctx.client.clone();
                fetch(
                    ctx.key,
                    async move {
                        let mut tracks = Vec::new();
                        for id in ids {
                            tracks.extend(client.album_tracks(&id).await?);
                        }
                        Ok(tracks)
                    },
                    |r| PageMsg::Artist(Message::Tracks(r)),
                )
            }
            // The songs are only for the figures below; without them the page
            // still has its albums.
            Message::Tracks(Err(_)) => Task::none(),
            Message::Tracks(Ok(items)) => {
                let store = ctx.store.clone();
                let key = ctx.key;
                Task::perform(
                    async move {
                        let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
                        let finish =
                            finish::finish_of(store.clone(), ids.clone(), "this artist's songs")
                                .await;
                        let plays: HashMap<String, i64> = store
                            .call(move |w| w.store.stats_for(&ids).unwrap_or_default())
                            .await
                            .unwrap_or_default()
                            .into_iter()
                            .map(|(id, stats)| (id, stats.play_count))
                            .collect();
                        let top = finish::top_songs(&items, &plays, finish::TOP_SONGS)
                            .iter()
                            .map(TrackMeta::from_item)
                            .collect();
                        (finish, top)
                    },
                    move |(finish, top)| {
                        App::Page(key, PageMsg::Artist(Message::Listening { finish, top }))
                    },
                )
            }
            Message::Listening { finish, top } => {
                self.finish = finish;
                self.top = top;
                Task::none()
            }
        }
    }

    pub fn artwork(&self) -> Vec<ArtKey> {
        let mut keys: Vec<ArtKey> = self
            .albums
            .ready()
            .map(|tiles| tiles.iter().filter_map(|t| t.art.clone()).collect())
            .unwrap_or_default();
        keys.extend(super::track_art(&self.top));
        keys
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let mut body = column![
            widgets::title_bar(&self.name, view.can_go_back),
            crate::radio::start_button(crate::radio::Station::new(
                aoide_jellyfin::MixSeed::Artist(self.id.clone()),
                self.name.clone(),
            )),
        ]
        .spacing(20);
        if let Some(finish) = &self.finish {
            body = body.push(widgets::secondary(finish.as_str()).size(13));
        }
        if !self.top.is_empty() {
            body = body.push(
                column![
                    text("Top songs").size(20).font(widgets::BOLD),
                    widgets::track_list(
                        &self.top,
                        view,
                        widgets::ListStyle {
                            numbered: false,
                            show_album: true,
                        },
                        PlaySource::Unknown,
                    ),
                ]
                .spacing(10),
            );
        }
        let body = body.push(match &self.albums {
            Load::Loading => widgets::loading(),
            Load::Failed(error) => widgets::failure(error),
            Load::Ready(tiles) if tiles.is_empty() => {
                widgets::empty("No albums credited to this artist.")
            }
            Load::Ready(tiles) => column![
                text("Albums").size(20).font(widgets::BOLD),
                widgets::tile_grid(tiles, view.artwork, super::home::open_album),
            ]
            .spacing(10)
            .into(),
        });
        widgets::page(body)
    }
}
