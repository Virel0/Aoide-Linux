//! Genres: every genre, by name.

use aoide_melete::Item;
use iced::widget::{button, column, row, text};
use iced::{Element, Task};

use super::{Ctx, Load, PageMsg, Route, View, fetch};
use crate::app::Message as App;
use crate::failure::Failure;
use crate::theme;
use crate::widgets;

#[derive(Debug)]
pub struct State {
    genres: Load<Vec<(String, String)>>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<Item>, Failure>),
}

pub fn open(ctx: &Ctx<'_>) -> (State, Task<App>) {
    let client = ctx.client.clone();
    let task = fetch(ctx.key, async move { client.genres().await }, |r| {
        PageMsg::Genres(Message::Loaded(r))
    });
    (
        State {
            genres: Load::Loading,
        },
        task,
    )
}

impl State {
    pub fn update(&mut self, message: Message) -> Task<App> {
        match message {
            Message::Loaded(result) => {
                self.genres = Load::from(
                    result.map(|genres| genres.into_iter().map(|g| (g.id, g.name)).collect()),
                );
            }
        }
        Task::none()
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let body = column![widgets::title_bar("Genres", view.can_go_back)].spacing(20);
        let body = body.push(match &self.genres {
            Load::Loading => widgets::loading(),
            Load::Failed(error) => view.failure(error),
            Load::Ready(genres) if genres.is_empty() => {
                widgets::empty("No genres in this library.")
            }
            Load::Ready(genres) => row(genres.iter().map(|(id, name)| {
                button(text(name.as_str()).size(15))
                    .style(theme::tonal)
                    .padding([10, 16])
                    .on_press(App::Open(Route::Genre {
                        id: id.clone(),
                        name: name.clone(),
                    }))
                    .into()
            }))
            .spacing(10)
            .wrap()
            .vertical_spacing(10)
            .into(),
        });
        widgets::page(body)
    }
}
