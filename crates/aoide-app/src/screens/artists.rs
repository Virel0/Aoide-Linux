//! Artists: everyone credited with an album, a page at a time.

use aoide_melete::{Item, ListQuery, Page as ItemPage};
use iced::widget::{column, sensor};
use iced::{Element, Task};

use super::{Ctx, PageMsg, Route, View, fetch};
use crate::app::Message as App;
use crate::failure::Failure;
use crate::library::Tile;
use crate::widgets;

const PAGE: u32 = 120;

#[derive(Debug)]
pub struct State {
    pub tiles: Vec<Tile>,
    total: Option<u32>,
    loading: bool,
    error: Option<Failure>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<ItemPage<Item>, Failure>),
    More,
}

pub fn open(ctx: &Ctx<'_>) -> (State, Task<App>) {
    let mut state = State {
        tiles: Vec::new(),
        total: None,
        loading: false,
        error: None,
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
            start_index: self.tiles.len() as u32,
            limit: Some(PAGE),
            ..ListQuery::default()
        };
        let client = ctx.client.clone();
        fetch(ctx.key, async move { client.artists(&query).await }, |r| {
            PageMsg::Artists(Message::Loaded(r))
        })
    }

    pub fn update(&mut self, message: Message, ctx: &Ctx<'_>) -> Task<App> {
        match message {
            Message::Loaded(result) => {
                self.loading = false;
                match result {
                    Ok(page) => {
                        self.total = Some(page.total);
                        self.tiles.extend(page.items.iter().map(Tile::artist));
                        if page.items.is_empty() {
                            self.total = Some(self.tiles.len() as u32);
                        }
                    }
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            Message::More => self.load(ctx),
        }
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let mut body = column![
            widgets::title_bar("Artists", view.can_go_back),
            widgets::tile_grid(&self.tiles, view.artwork, |tile| Route::Artist {
                id: tile.id.clone(),
                name: tile.title.clone(),
            })
        ]
        .spacing(20);
        if let Some(error) = &self.error {
            body = body.push(view.failure(error));
        } else if self.total.is_none_or(|t| (self.tiles.len() as u32) < t) {
            body = body.push(
                sensor(widgets::loading())
                    .key(self.tiles.len())
                    .anticipate(600)
                    .on_show(move |_| App::Page(key, PageMsg::Artists(Message::More))),
            );
        } else if self.tiles.is_empty() {
            body = body.push(widgets::empty("No artists in this library."));
        }
        widgets::page(body)
    }
}
