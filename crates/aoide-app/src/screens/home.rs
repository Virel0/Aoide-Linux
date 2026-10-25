//! Home: the resume grid (the last few things played from, led by another
//! device's queue when it is newer), then shelves of albums — recently added,
//! recently played, most played, recently released, a random handful — and
//! some genres to wander into. A shelf with nothing on it is left out.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;

use aoide_jellyfin::{Item, ListQuery, Page, Sort, SortOrder};
use iced::widget::{button, column, row, scrollable, text};
use iced::{Element, Fill, Task};

use super::{Ctx, Load, PageMsg, Route, View, fetch};
use crate::app::Message as App;
use crate::artwork::ArtKey;
use crate::failure::Failure;
use crate::library::{self, Tile};
use crate::{theme, widgets};

/// Albums per shelf.
const SHELF: usize = 18;
/// Genres on the genre shelf.
const GENRES: usize = 12;

#[derive(Debug)]
pub struct State {
    added: Load<Vec<Tile>>,
    played: Load<Vec<Tile>>,
    most: Load<Vec<Tile>>,
    released: Load<Vec<Tile>>,
    random: Load<Vec<Tile>>,
    /// Id and name, a random few.
    genres: Load<Vec<(String, String)>>,
    /// The resume grid; empty draws nothing.
    resume: Vec<crate::handoff::Card>,
    /// How many songs this week's Discover holds; `None` hides the tile.
    discover: Option<usize>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Added(Result<Vec<Item>, Failure>),
    Played(Result<Vec<Item>, Failure>),
    Most(Result<Vec<Item>, Failure>),
    Released(Result<Vec<Item>, Failure>),
    Random(Result<Vec<Item>, Failure>),
    Genres(Result<Vec<Item>, Failure>),
    Resume(Vec<crate::handoff::Card>),
    Discover(Option<usize>),
}

pub fn open(ctx: &Ctx<'_>) -> (State, Task<App>) {
    let key = ctx.key;
    let library = ctx.library.map(str::to_owned);
    let wrap = |m: fn(Result<Vec<Item>, Failure>) -> self::Message| move |r| PageMsg::Home(m(r));
    let albums = |sort: Sort, order: SortOrder| ListQuery {
        parent_id: library.clone(),
        sort,
        order,
        limit: Some(SHELF as u32),
        ..ListQuery::default()
    };
    let items = |page: Page<Item>| page.items;
    let released = albums(Sort::ReleaseDate, SortOrder::Descending);
    let random = albums(Sort::Random, SortOrder::Ascending);
    let genres = ListQuery {
        parent_id: library.clone(),
        ..ListQuery::default()
    };
    let (a, b, c, d, e, f) = (
        ctx.client.clone(),
        ctx.client.clone(),
        ctx.client.clone(),
        ctx.client.clone(),
        ctx.client.clone(),
        ctx.client.clone(),
    );
    let (la, lb, lc) = (library.clone(), library.clone(), library);
    let tasks = Task::batch([
        Task::done(App::Handoff(crate::handoff::Msg::LoadResume(key))),
        Task::done(App::Discover(super::discover::Msg::Home(key))),
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
        fetch(
            key,
            async move { d.albums(&released).await.map(items) },
            wrap(Message::Released),
        ),
        fetch(
            key,
            async move { e.albums(&random).await.map(items) },
            wrap(Message::Random),
        ),
        fetch(
            key,
            async move { f.genres(&genres).await.map(items) },
            wrap(Message::Genres),
        ),
    ]);
    (
        State {
            added: Load::Loading,
            played: Load::Loading,
            most: Load::Loading,
            released: Load::Loading,
            random: Load::Loading,
            genres: Load::Loading,
            resume: Vec::new(),
            discover: None,
        },
        tasks,
    )
}

fn album_tiles(result: Result<Vec<Item>, Failure>) -> Load<Vec<Tile>> {
    Load::from(result.map(|items| items.iter().map(Tile::album).collect()))
}

/// A few of `items`, in a different order each time Home opens.
fn pick<T>(mut items: Vec<T>, count: usize) -> Vec<T> {
    let random = RandomState::new();
    // Fisher-Yates, as far as is needed.
    for i in 0..items.len().min(count) {
        let j = i + (random.hash_one(i) % (items.len() - i) as u64) as usize;
        items.swap(i, j);
    }
    items.truncate(count);
    items
}

impl State {
    pub fn update(&mut self, message: Message) -> Task<App> {
        match message {
            Message::Added(result) => self.added = album_tiles(result),
            Message::Played(result) => {
                self.played =
                    Load::from(result.map(|items| library::albums_of_tracks(&items, SHELF)));
            }
            Message::Most(result) => {
                self.most =
                    Load::from(result.map(|items| library::albums_of_tracks(&items, SHELF)));
            }
            Message::Released(result) => self.released = album_tiles(result),
            Message::Random(result) => self.random = album_tiles(result),
            Message::Genres(result) => {
                self.genres = Load::from(result.map(|items| {
                    let genres = items.into_iter().map(|g| (g.id, g.name)).collect();
                    pick(genres, GENRES)
                }));
            }
            Message::Resume(cards) => self.resume = cards,
            Message::Discover(count) => self.discover = count,
        }
        Task::none()
    }

    fn shelves(&self) -> [(&'static str, &Load<Vec<Tile>>); 5] {
        [
            ("Recently added", &self.added),
            ("Recently played", &self.played),
            ("Most played", &self.most),
            ("Recently released", &self.released),
            ("Random albums", &self.random),
        ]
    }

    pub fn artwork(&self) -> Vec<ArtKey> {
        self.resume
            .iter()
            .filter_map(|card| card.art.clone())
            .chain(
                self.shelves()
                    .into_iter()
                    .filter_map(|(_, shelf)| shelf.ready())
                    .flatten()
                    .filter_map(|t| t.art.clone()),
            )
            .collect()
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let mut body = column![widgets::title_bar("Home", view.can_go_back)].spacing(28);
        if !self.resume.is_empty() {
            body = body.push(resume_grid(&self.resume, view));
        }
        if let Some(count) = self.discover {
            body = body.push(super::discover::tile(count));
        }
        for (title, shelf) in self.shelves() {
            match shelf {
                Load::Loading => body = body.push(widgets::loading()),
                Load::Failed(error) => body = body.push(view.failure(error)),
                Load::Ready(tiles) if tiles.is_empty() => {}
                Load::Ready(tiles) => {
                    body = body.push(widgets::shelf(title, tiles, view.artwork, open_album));
                }
            }
        }
        match &self.genres {
            Load::Ready(genres) if !genres.is_empty() => body = body.push(genre_shelf(genres)),
            Load::Failed(error) => body = body.push(view.failure(error)),
            _ => {}
        }
        let empty = self
            .shelves()
            .iter()
            .all(|(_, l)| l.ready().is_some_and(Vec::is_empty))
            && self.genres.ready().is_some_and(Vec::is_empty);
        if empty {
            body = body.push(widgets::empty(
                "Nothing here yet. Albums appear as they're added and played.",
            ));
        }
        widgets::page(body)
    }
}

/// Two rows of three: each tile plays at once. A context's `×` takes it out
/// of the recents; another device's queue has none, it goes when it is old.
fn resume_grid<'a>(cards: &'a [crate::handoff::Card], view: &View<'a>) -> Element<'a, App> {
    use crate::handoff::Msg;
    use aoide_store::ResumeTile;
    use iced::widget::container;
    let tile = |card: &'a crate::handoff::Card| -> Element<'a, App> {
        let words = column![
            text(card.tile.title())
                .size(14)
                .font(widgets::SEMIBOLD)
                .wrapping(text::Wrapping::WordOrGlyph)
                .style(|t: &iced::Theme| text::Style {
                    color: Some(t.palette().text),
                }),
            widgets::secondary(card.tile.detail())
                .size(12)
                .wrapping(text::Wrapping::None),
        ]
        .spacing(2)
        .width(Fill)
        .clip(true);
        let mut line = row![
            widgets::art(
                card.art.as_ref(),
                view.artwork,
                iced::Length::Fixed(52.0),
                6.0
            ),
            words,
        ]
        .spacing(10)
        .align_y(iced::Center);
        if let ResumeTile::Context(context) = &card.tile {
            line = line.push(
                button(crate::icons::icon(
                    crate::icons::Icon::Close,
                    12.0,
                    crate::icons::Tone::Secondary,
                ))
                .style(theme::row)
                .padding(4)
                .on_press(App::Handoff(Msg::Forget(context.key()))),
            );
        }
        button(container(line).padding(6))
            .width(Fill)
            .style(theme::tonal)
            .padding(0)
            .on_press(App::Handoff(Msg::Resume(card.tile.clone())))
            .into()
    };
    // Rows of three, the last one padded so its tiles keep their width.
    let mut rows = column![].spacing(10);
    for chunk in cards.chunks(3) {
        let mut line = row![].spacing(10);
        for card in chunk {
            line = line.push(container(tile(card)).width(Fill));
        }
        for _ in chunk.len()..3 {
            line = line.push(iced::widget::space().width(Fill));
        }
        rows = rows.push(line);
    }
    column![text("Jump back in").size(20).font(widgets::BOLD), rows]
        .spacing(10)
        .into()
}

/// The genres as a row of chips, scrolled sideways like the album shelves.
fn genre_shelf(genres: &[(String, String)]) -> Element<'_, App> {
    let chips = row(genres.iter().map(|(id, name)| {
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
    .padding(iced::Padding::ZERO.bottom(14));
    column![
        text("Genres").size(20).font(widgets::BOLD),
        scrollable(chips).horizontal().width(Fill),
    ]
    .spacing(10)
    .into()
}

pub fn open_album(tile: &Tile) -> Route {
    Route::Album {
        id: tile.id.clone(),
        title: tile.title.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pick_is_a_few_of_the_items_each_once() {
        let items: Vec<u32> = (0..40).collect();
        let mut picked = pick(items, GENRES);
        assert_eq!(picked.len(), GENRES);
        picked.sort_unstable();
        picked.dedup();
        assert_eq!(picked.len(), GENRES);
        assert!(picked.iter().all(|n| *n < 40));
        assert_eq!(pick(vec![1, 2], GENRES).len(), 2);
        assert!(pick(Vec::<u8>::new(), GENRES).is_empty());
    }
}
