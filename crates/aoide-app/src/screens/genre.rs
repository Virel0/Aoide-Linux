//! A genre: its albums.

use aoide_jellyfin::Item;
use iced::widget::column;
use iced::{Element, Task};

use super::{Ctx, Load, PageMsg, View, fetch};
use crate::app::Message as App;
use crate::library::{self, Tile};
use crate::widgets;

#[derive(Debug)]
pub struct State {
    name: String,
    pub albums: Load<Vec<Tile>>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Albums(Result<Vec<Item>, String>),
}

pub fn open(ctx: &Ctx<'_>, id: &str, name: &str) -> (State, Task<App>) {
    let client = ctx.client.clone();
    let id = id.to_owned();
    let library = ctx.library.map(str::to_owned);
    let task = fetch(
        ctx.key,
        async move { library::genre_albums(&client, &id, library.as_deref()).await },
        |r| PageMsg::Genre(Message::Albums(r)),
    );
    (
        State {
            name: name.to_owned(),
            albums: Load::Loading,
        },
        task,
    )
}

impl State {
    pub fn update(&mut self, message: Message) -> Task<App> {
        match message {
            Message::Albums(result) => {
                self.albums =
                    Load::from(result.map(|items| items.iter().map(Tile::album).collect()));
            }
        }
        Task::none()
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let body = column![widgets::title_bar(&self.name, view.can_go_back)].spacing(20);
        let body = body.push(match &self.albums {
            Load::Loading => widgets::loading(),
            Load::Failed(error) => widgets::failure(error),
            Load::Ready(tiles) if tiles.is_empty() => widgets::empty("No albums in this genre."),
            Load::Ready(tiles) => widgets::tile_grid(tiles, view.artwork, super::home::open_album),
        });
        widgets::page(body)
    }
}
