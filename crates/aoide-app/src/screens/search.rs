//! Search: songs, albums and artists as you type, asked for once typing
//! pauses.

use std::time::Duration;

use aoide_jellyfin::SearchResults;
use aoide_store::PlaySource;
use iced::widget::{column, text, text_input};
use iced::{Element, Task};

use super::{Ctx, Load, PageMsg, Route, View, fetch};
use crate::app::Message as App;
use crate::artwork::ArtKey;
use crate::library::Tile;
use crate::playback::tracks::TrackMeta;
use crate::theme;
use crate::widgets::{self, ListStyle};

/// How long typing must pause before the server is asked.
pub const DEBOUNCE: Duration = Duration::from_millis(300);
/// Results per kind.
const LIMIT: u32 = 24;
pub const FIELD: &str = "search-field";

/// Counts keystrokes so that only the last one's timer searches.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Debouncer {
    generation: u64,
}

impl Debouncer {
    /// Something was typed: the timer to start.
    pub fn typed(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    /// A timer ran out: whether it is still the latest.
    pub fn is_current(&self, generation: u64) -> bool {
        generation == self.generation
    }
}

#[derive(Debug)]
pub struct State {
    term: String,
    debouncer: Debouncer,
    /// The term the results are for.
    searched: String,
    results: Option<Load<Results>>,
}

#[derive(Debug, Default)]
struct Results {
    songs: Vec<TrackMeta>,
    albums: Vec<Tile>,
    artists: Vec<Tile>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Typed(String),
    Due(u64),
    Found(String, Result<SearchResults, String>),
}

pub fn open() -> (State, Task<App>) {
    (
        State {
            term: String::new(),
            debouncer: Debouncer::default(),
            searched: String::new(),
            results: None,
        },
        iced::widget::operation::focus(FIELD),
    )
}

impl State {
    pub fn update(&mut self, message: Message, ctx: &Ctx<'_>) -> Task<App> {
        match message {
            Message::Typed(term) => {
                self.term = term;
                let generation = self.debouncer.typed();
                let key = ctx.key;
                Task::perform(tokio::time::sleep(DEBOUNCE), move |()| {
                    App::Page(key, PageMsg::Search(Message::Due(generation)))
                })
            }
            Message::Due(generation) => {
                if !self.debouncer.is_current(generation) {
                    return Task::none();
                }
                let term = self.term.trim().to_owned();
                if term.is_empty() {
                    self.results = None;
                    self.searched.clear();
                    return Task::none();
                }
                if term == self.searched {
                    return Task::none();
                }
                self.searched = term.clone();
                self.results = Some(Load::Loading);
                let client = ctx.client.clone();
                let library = ctx.library.map(str::to_owned);
                let asked = term.clone();
                fetch(
                    ctx.key,
                    async move { client.search(&asked, library.as_deref(), LIMIT).await },
                    move |r| PageMsg::Search(Message::Found(term.clone(), r)),
                )
            }
            Message::Found(term, result) => {
                // An answer for an older term, overtaken by typing.
                if term != self.searched {
                    return Task::none();
                }
                self.results = Some(Load::from(result.map(|found| Results {
                    songs: found.songs.iter().map(TrackMeta::from_item).collect(),
                    albums: found.albums.iter().map(Tile::album).collect(),
                    artists: found.artists.iter().map(Tile::artist).collect(),
                })));
                Task::none()
            }
        }
    }

    pub fn artwork(&self) -> Vec<ArtKey> {
        let Some(Load::Ready(results)) = &self.results else {
            return Vec::new();
        };
        super::track_art(&results.songs)
            .into_iter()
            .chain(results.albums.iter().filter_map(|t| t.art.clone()))
            .chain(results.artists.iter().filter_map(|t| t.art.clone()))
            .collect()
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let field = text_input("Songs, albums and artists", &self.term)
            .id(FIELD)
            .on_input(move |s| App::Page(key, PageMsg::Search(Message::Typed(s))))
            .padding(10)
            .size(16)
            .style(theme::field);
        let mut body = column![widgets::title_bar("Search", view.can_go_back), field].spacing(20);
        match &self.results {
            None => {}
            Some(Load::Loading) => body = body.push(widgets::loading()),
            Some(Load::Failed(error)) => body = body.push(widgets::failure(error)),
            Some(Load::Ready(results)) => {
                if results.songs.is_empty()
                    && results.albums.is_empty()
                    && results.artists.is_empty()
                {
                    body = body.push(widgets::empty("Nothing matches that."));
                }
                if !results.songs.is_empty() {
                    body = body.push(text("Songs").size(20).font(widgets::BOLD)).push(
                        widgets::track_list(
                            &results.songs,
                            view,
                            ListStyle {
                                numbered: false,
                                show_album: true,
                            },
                            PlaySource::Search,
                        ),
                    );
                }
                if !results.albums.is_empty() {
                    body = body.push(text("Albums").size(20).font(widgets::BOLD)).push(
                        widgets::tile_grid(&results.albums, view.artwork, super::home::open_album),
                    );
                }
                if !results.artists.is_empty() {
                    body = body
                        .push(text("Artists").size(20).font(widgets::BOLD))
                        .push(widgets::tile_grid(&results.artists, view.artwork, |tile| {
                            Route::Artist {
                                id: tile.id.clone(),
                                name: tile.title.clone(),
                            }
                        }));
                }
            }
        }
        widgets::page(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_last_keystrokes_timer_searches() {
        let mut debouncer = Debouncer::default();
        let first = debouncer.typed();
        let second = debouncer.typed();
        assert!(!debouncer.is_current(first));
        assert!(debouncer.is_current(second));
        let third = debouncer.typed();
        assert!(!debouncer.is_current(second));
        assert!(debouncer.is_current(third));
    }
}
