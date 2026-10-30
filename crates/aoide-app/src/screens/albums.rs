//! Albums: every album as a grid, a page at a time as it scrolls, sortable.

use std::fmt;

use aoide_melete::{Item, ListQuery, Page as ItemPage, Sort};
use iced::widget::{column, pick_list, row, sensor, space};
use iced::{Center, Element, Fill, Task};

use super::{Ctx, PageMsg, View, fetch};
use crate::app::Message as App;
use crate::failure::Failure;
use crate::library::Tile;
use crate::widgets;

/// Albums per request.
pub const PAGE: u32 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlbumSort {
    #[default]
    Title,
    Artist,
    RecentlyAdded,
    ReleaseDate,
}

impl AlbumSort {
    pub const ALL: [AlbumSort; 4] = [
        AlbumSort::Title,
        AlbumSort::Artist,
        AlbumSort::RecentlyAdded,
        AlbumSort::ReleaseDate,
    ];

    pub fn query(self) -> Sort {
        match self {
            AlbumSort::Title => Sort::Name,
            AlbumSort::Artist => Sort::Artist,
            AlbumSort::RecentlyAdded => Sort::Recent,
            AlbumSort::ReleaseDate => Sort::Year,
        }
    }
}

impl fmt::Display for AlbumSort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AlbumSort::Title => "Title",
            AlbumSort::Artist => "Artist",
            AlbumSort::RecentlyAdded => "Recently added",
            AlbumSort::ReleaseDate => "Release date",
        })
    }
}

#[derive(Debug)]
pub struct State {
    pub tiles: Vec<Tile>,
    pub total: Option<u32>,
    pub sort: AlbumSort,
    loading: bool,
    error: Option<Failure>,
    /// Bumped on every re-sort, so a page for the old order is dropped.
    generation: u32,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(u32, Result<ItemPage<Item>, Failure>),
    More,
    Sort(AlbumSort),
}

pub fn open(ctx: &Ctx<'_>) -> (State, Task<App>) {
    let mut state = State {
        tiles: Vec::new(),
        total: None,
        sort: AlbumSort::default(),
        loading: false,
        error: None,
        generation: 0,
    };
    let task = state.load(ctx);
    (state, task)
}

impl State {
    fn load(&mut self, ctx: &Ctx<'_>) -> Task<App> {
        if self.loading || self.total.is_some_and(|t| self.tiles.len() as u32 >= t) {
            return Task::none();
        }
        self.loading = true;
        let query = ListQuery {
            sort: self.sort.query(),
            start_index: self.tiles.len() as u32,
            limit: Some(PAGE),
            ..ListQuery::default()
        };
        let client = ctx.client.clone();
        let generation = self.generation;
        fetch(
            ctx.key,
            async move { client.albums(&query).await },
            move |r| PageMsg::Albums(Message::Loaded(generation, r)),
        )
    }

    pub fn update(&mut self, message: Message, ctx: &Ctx<'_>) -> Task<App> {
        match message {
            Message::Loaded(generation, _) if generation != self.generation => Task::none(),
            Message::Loaded(_, result) => {
                self.loading = false;
                match result {
                    Ok(page) => {
                        self.total = Some(page.total);
                        self.tiles.extend(page.items.iter().map(Tile::album));
                        // A server that says there are more but sends none would
                        // otherwise be asked forever.
                        if page.items.is_empty() {
                            self.total = Some(self.tiles.len() as u32);
                        }
                    }
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            Message::More => self.load(ctx),
            Message::Sort(sort) => {
                if sort == self.sort {
                    return Task::none();
                }
                self.sort = sort;
                self.generation += 1;
                self.tiles.clear();
                self.total = None;
                self.loading = false;
                self.error = None;
                self.load(ctx)
            }
        }
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let header = row![
            widgets::title_bar("Albums", view.can_go_back),
            space().width(Fill),
            pick_list(AlbumSort::ALL, Some(self.sort), move |s| {
                App::Page(key, PageMsg::Albums(Message::Sort(s)))
            })
            .text_size(13),
        ]
        .align_y(Center);
        let mut body = column![
            header,
            widgets::tile_grid(&self.tiles, view.artwork, super::home::open_album)
        ]
        .spacing(20);
        if let Some(error) = &self.error {
            body = body.push(view.failure(error));
        } else if self.total.is_none_or(|t| (self.tiles.len() as u32) < t) {
            body = body.push(
                sensor(widgets::loading())
                    .key(self.tiles.len())
                    .anticipate(600)
                    .on_show(move |_| App::Page(key, PageMsg::Albums(Message::More))),
            );
        } else if self.tiles.is_empty() {
            body = body.push(widgets::empty("No albums in this library."));
        }
        widgets::page(body)
    }
}
