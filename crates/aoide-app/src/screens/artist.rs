//! An artist: their portrait and its credit where the server has one
//! (Melete 0.9.0), how much of them is finished, their most played songs,
//! and their albums, newest first, under Albums, Singles & EPs, Live and
//! Compilations (`kinds`).

use std::collections::HashMap;

use aoide_melete::{Item, PosterCredit};
use aoide_store::PlaySource;
use iced::widget::{column, container, text};
use iced::{Element, Task};

use super::{Ctx, Load, PageMsg, View, fetch};
use crate::app::Message as App;
use crate::artwork::ArtKey;
use crate::cover_colour::Origin;
use crate::failure::Failure;
use crate::finish;
use crate::library::Tile;
use crate::playback::tracks::TrackMeta;
use crate::widgets;

mod kinds;

/// How wide the portrait is drawn under the title.
const PORTRAIT: f32 = 160.0;

#[derive(Debug)]
pub struct State {
    id: String,
    name: String,
    /// Their portrait, once the page has loaded, if they have one.
    portrait: Option<ArtKey>,
    /// Who made the portrait, for a picture from Wikimedia Commons.
    credit: Option<PosterCredit>,
    /// Their albums by kind, each kind that has any.
    albums: Load<Vec<(kinds::Kind, Vec<Tile>)>>,
    /// Each album's colours from the server, by id.
    palettes: Vec<(String, Vec<[u8; 3]>)>,
    /// Their most played songs here; empty until there are plays.
    top: Vec<TrackMeta>,
    /// "You finish 78% of this artist's songs", once there is enough history.
    finish: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// The artist, and their albums.
    Loaded(Box<Result<(Item, Vec<Item>), Failure>>),
    /// A link in the portrait's credit.
    OpenLink(String),
    /// Every song on their albums.
    Tracks(Result<Vec<Item>, Failure>),
    Listening {
        finish: Option<String>,
        top: Vec<TrackMeta>,
    },
}

pub fn open(ctx: &Ctx<'_>, id: &str, name: &str) -> (State, Task<App>) {
    let client = ctx.client.clone();
    let id = id.to_owned();
    let asked = id.clone();
    let task = fetch(ctx.key, async move { client.artist(&asked).await }, |r| {
        PageMsg::Artist(Message::Loaded(Box::new(r)))
    });
    (
        State {
            id: id.clone(),
            name: name.to_owned(),
            portrait: None,
            credit: None,
            albums: Load::Loading,
            palettes: Vec::new(),
            top: Vec::new(),
            finish: None,
        },
        task,
    )
}

impl State {
    pub fn update(&mut self, message: Message, ctx: &Ctx<'_>) -> Task<App> {
        match message {
            Message::Loaded(result) => {
                let result = (*result).map(|(artist, albums)| {
                    self.portrait = artist
                        .portrait
                        .as_deref()
                        .map(|path| crate::posters::key_at(path, crate::artwork::TILE));
                    self.credit = artist.portrait_credit;
                    albums
                });
                let ids: Vec<String> = result
                    .as_ref()
                    .map(|items| items.iter().map(|a| a.id.clone()).collect())
                    .unwrap_or_default();
                if let Ok(items) = &result {
                    self.palettes = items
                        .iter()
                        .map(|a| (a.id.clone(), a.palette.clone()))
                        .collect();
                }
                self.albums = Load::from(result.map(|items| kinds::group(&items)));
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
            Message::OpenLink(url) => crate::posters::open(&url),
        }
    }

    /// Where the page's colours come from, once it has loaded: their
    /// portrait, else their first album's cover (its colours from the
    /// server, else its picture's).
    pub fn colour_origins(&self) -> Vec<Origin> {
        let Some(groups) = self.albums.ready() else {
            return Vec::new();
        };
        let mut origins: Vec<Origin> = self
            .portrait
            .clone()
            .map(Origin::Picture)
            .into_iter()
            .collect();
        if let Some(first) = groups.first().and_then(|(_, tiles)| tiles.first()) {
            origins.push(Origin::Album(first.id.clone()));
            origins.extend(first.art.clone().map(Origin::Picture));
        }
        origins
    }

    /// Their albums' colours as the page brought them.
    pub fn known_palettes(&self) -> impl Iterator<Item = (&str, &[[u8; 3]])> {
        self.palettes
            .iter()
            .map(|(id, palette)| (id.as_str(), palette.as_slice()))
    }

    pub fn artwork(&self) -> Vec<ArtKey> {
        let mut keys: Vec<ArtKey> = self
            .albums
            .ready()
            .map(|groups| {
                groups
                    .iter()
                    .flat_map(|(_, tiles)| tiles.iter().filter_map(|t| t.art.clone()))
                    .collect()
            })
            .unwrap_or_default();
        keys.extend(super::track_art(&self.top));
        keys.extend(self.portrait.clone());
        keys
    }

    /// Their portrait, round, with its credit under it; `None` without one.
    fn portrait<'a>(&'a self, view: &View<'a>) -> Option<Element<'a, App>> {
        let portrait = self.portrait.as_ref()?;
        let picture = container(widgets::round_art(Some(portrait), view.artwork))
            .width(PORTRAIT)
            .height(PORTRAIT);
        let picture = widgets::lifted(picture, PORTRAIT / 2.0, view);
        let key = view.key;
        let credit = self.credit.as_ref().and_then(|credit| {
            widgets::credit_line(credit, move |url| {
                App::Page(key, PageMsg::Artist(Message::OpenLink(url)))
            })
        });
        Some(column![picture].push(credit).spacing(6).into())
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let header = column![
            widgets::title_bar(&self.name, view.can_go_back),
            self.portrait(view),
            crate::radio::start_button(crate::radio::Station::new(
                crate::radio::MixSeed::Artist(self.id.clone()),
                self.name.clone(),
            )),
        ]
        .spacing(20);
        let mut body = column![].spacing(20);
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
            Load::Failed(error) => view.failure(error),
            Load::Ready(groups) if groups.is_empty() => {
                widgets::empty("No albums credited to this artist.")
            }
            Load::Ready(groups) => column(groups.iter().map(|(kind, tiles)| {
                column![
                    text(kind.title()).size(20).font(widgets::BOLD),
                    widgets::tile_grid(tiles, view.artwork, super::home::open_album),
                ]
                .spacing(10)
                .into()
            }))
            .spacing(20)
            .into(),
        });
        widgets::page_with_header(header, body, view)
    }
}
