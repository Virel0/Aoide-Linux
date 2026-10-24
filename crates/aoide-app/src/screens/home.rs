//! Home: shelves of albums — recently added, recently played, most played.

use aoide_jellyfin::Item;
use iced::widget::column;
use iced::{Element, Task};

use super::{Ctx, Load, PageMsg, Route, View, fetch};
use crate::app::Message as App;
use crate::artwork::ArtKey;
use crate::library::{self, Tile};
use crate::widgets;

/// Albums per shelf.
const SHELF: usize = 18;

#[derive(Debug)]
pub struct State {
    added: Load<Vec<Tile>>,
    played: Load<Vec<Tile>>,
    most: Load<Vec<Tile>>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Added(Result<Vec<Item>, String>),
    Played(Result<Vec<Item>, String>),
    Most(Result<Vec<Item>, String>),
}

pub fn open(ctx: &Ctx<'_>) -> (State, Task<App>) {
    let key = ctx.key;
    let library = ctx.library.map(str::to_owned);
    let (a, b, c) = (ctx.client.clone(), ctx.client.clone(), ctx.client.clone());
    let (la, lb, lc) = (library.clone(), library.clone(), library);
    let wrap = |m: fn(Result<Vec<Item>, String>) -> self::Message| move |r| PageMsg::Home(m(r));
    let tasks = Task::batch([
        fetch(
            key,
            async move { a.recently_added(la.as_deref(), SHELF as u32).await },
            wrap(Message::Added),
        ),
        // Tracks, a few times a shelf's worth: several often share an album.
        fetch(
            key,
            async move { b.recently_played(lb.as_deref(), 120).await },
            wrap(Message::Played),
        ),
        fetch(
            key,
            async move { c.most_played(lc.as_deref(), 120).await },
            wrap(Message::Most),
        ),
    ]);
    (
        State {
            added: Load::Loading,
            played: Load::Loading,
            most: Load::Loading,
        },
        tasks,
    )
}

impl State {
    pub fn update(&mut self, message: Message) -> Task<App> {
        match message {
            Message::Added(result) => {
                self.added =
                    Load::from(result.map(|items| items.iter().map(Tile::album).collect()));
            }
            Message::Played(result) => {
                self.played =
                    Load::from(result.map(|items| library::albums_of_tracks(&items, SHELF)));
            }
            Message::Most(result) => {
                self.most =
                    Load::from(result.map(|items| library::albums_of_tracks(&items, SHELF)));
            }
        }
        Task::none()
    }

    pub fn artwork(&self) -> Vec<ArtKey> {
        [&self.added, &self.played, &self.most]
            .into_iter()
            .filter_map(Load::ready)
            .flatten()
            .filter_map(|t| t.art.clone())
            .collect()
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let mut body = column![widgets::title_bar("Home", view.can_go_back)].spacing(28);
        for (title, shelf) in [
            ("Recently Added", &self.added),
            ("Recently Played", &self.played),
            ("Most Played", &self.most),
        ] {
            match shelf {
                Load::Loading => body = body.push(widgets::loading()),
                Load::Failed(error) => body = body.push(widgets::failure(error)),
                Load::Ready(tiles) if tiles.is_empty() => {}
                Load::Ready(tiles) => {
                    body = body.push(widgets::shelf(title, tiles, view.artwork, open_album));
                }
            }
        }
        if [&self.added, &self.played, &self.most]
            .iter()
            .all(|l| l.ready().is_some_and(Vec::is_empty))
        {
            body = body.push(widgets::empty(
                "Nothing here yet. Albums appear as they're added and played.",
            ));
        }
        widgets::page(body)
    }
}

pub fn open_album(tile: &Tile) -> Route {
    Route::Album {
        id: tile.id.clone(),
        title: tile.title.clone(),
    }
}
