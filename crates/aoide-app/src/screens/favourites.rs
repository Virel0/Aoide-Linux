//! Favourites: the songs, albums and artists marked as favourites on the
//! server (Jellyfin's `isFavorite`), one tab each.

use aoide_jellyfin::{Item, ListQuery, Sort, SortOrder};
use aoide_store::PlaySource;
use iced::widget::{button, column, row, text};
use iced::{Element, Task};

use super::{Ctx, Load, PageMsg, Route, View, fetch};
use crate::app::Message as App;
use crate::artwork::ArtKey;
use crate::failure::Failure;
use crate::library::Tile;
use crate::playback::tracks::TrackMeta;
use crate::{theme, widgets};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Songs,
    Albums,
    Artists,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Songs, Tab::Albums, Tab::Artists];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Songs => "Songs",
            Tab::Albums => "Albums",
            Tab::Artists => "Artists",
        }
    }
}

#[derive(Debug)]
pub struct State {
    tab: Tab,
    pub songs: Load<Vec<TrackMeta>>,
    pub albums: Load<Vec<Tile>>,
    pub artists: Load<Vec<Tile>>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Tab(Tab),
    Songs(Result<Vec<Item>, Failure>),
    Albums(Result<Vec<Item>, Failure>),
    Artists(Result<Vec<Item>, Failure>),
}

/// Only favourites, by name, everything at once: a favourites list is short.
pub fn favourites(parent_id: Option<String>) -> ListQuery {
    ListQuery {
        parent_id,
        favorites_only: true,
        sort: Sort::Name,
        order: SortOrder::Ascending,
        ..ListQuery::default()
    }
}

pub fn open(ctx: &Ctx<'_>) -> (State, Task<App>) {
    let library = ctx.library.map(str::to_owned);
    let (a, b, c) = (ctx.client.clone(), ctx.client.clone(), ctx.client.clone());
    let (songs, albums) = (favourites(library.clone()), favourites(library));
    // Artists have no library parent: asked for without one.
    let artists = favourites(None);
    let task = Task::batch([
        fetch(
            ctx.key,
            async move { a.songs(&songs).await.map(|p| p.items) },
            |r| PageMsg::Favourites(Message::Songs(r)),
        ),
        fetch(
            ctx.key,
            async move { b.albums(&albums).await.map(|p| p.items) },
            |r| PageMsg::Favourites(Message::Albums(r)),
        ),
        fetch(
            ctx.key,
            async move { c.album_artists(&artists).await.map(|p| p.items) },
            |r| PageMsg::Favourites(Message::Artists(r)),
        ),
    ]);
    (
        State {
            tab: Tab::Songs,
            songs: Load::Loading,
            albums: Load::Loading,
            artists: Load::Loading,
        },
        task,
    )
}

impl State {
    pub fn update(&mut self, message: Message) -> Task<App> {
        match message {
            Message::Tab(tab) => self.tab = tab,
            Message::Songs(result) => {
                self.songs = Load::from(
                    result.map(|items| items.iter().map(TrackMeta::from_item).collect()),
                );
            }
            Message::Albums(result) => {
                self.albums =
                    Load::from(result.map(|items| items.iter().map(Tile::album).collect()));
            }
            Message::Artists(result) => {
                self.artists =
                    Load::from(result.map(|items| items.iter().map(Tile::artist).collect()));
            }
        }
        Task::none()
    }

    pub fn artwork(&self) -> Vec<ArtKey> {
        let tiles = |load: &Load<Vec<Tile>>| -> Vec<ArtKey> {
            load.ready()
                .map(|t| t.iter().filter_map(|t| t.art.clone()).collect())
                .unwrap_or_default()
        };
        match self.tab {
            Tab::Songs => self
                .songs
                .ready()
                .map(|t| super::track_art(t))
                .unwrap_or_default(),
            Tab::Albums => tiles(&self.albums),
            Tab::Artists => tiles(&self.artists),
        }
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let tabs = row(Tab::ALL.into_iter().map(|tab| {
            let count = match tab {
                Tab::Songs => self.songs.ready().map(Vec::len),
                Tab::Albums => self.albums.ready().map(Vec::len),
                Tab::Artists => self.artists.ready().map(Vec::len),
            };
            let label = match count {
                Some(n) => format!("{} {n}", tab.label()),
                None => tab.label().to_owned(),
            };
            button(text(label).size(14))
                .style(if tab == self.tab {
                    theme::filled
                } else {
                    theme::tonal
                })
                .padding([6, 14])
                .on_press(App::Page(key, PageMsg::Favourites(Message::Tab(tab))))
                .into()
        }))
        .spacing(8);
        let content = match self.tab {
            Tab::Songs => match &self.songs {
                Load::Loading => widgets::loading(),
                Load::Failed(error) => view.failure(error),
                Load::Ready(songs) if songs.is_empty() => {
                    widgets::empty("No favourite songs yet. Mark one from any song's … menu.")
                }
                Load::Ready(songs) => column![
                    widgets::play_buttons(songs, format!("{key}:favourites"), PlaySource::Unknown),
                    widgets::track_list(
                        songs,
                        view,
                        widgets::ListStyle {
                            numbered: false,
                            show_album: true,
                        },
                        PlaySource::Unknown,
                    ),
                ]
                .spacing(14)
                .into(),
            },
            Tab::Albums => match &self.albums {
                Load::Loading => widgets::loading(),
                Load::Failed(error) => view.failure(error),
                Load::Ready(tiles) if tiles.is_empty() => {
                    widgets::empty("No favourite albums yet.")
                }
                Load::Ready(tiles) => {
                    widgets::tile_grid(tiles, view.artwork, super::home::open_album)
                }
            },
            Tab::Artists => match &self.artists {
                Load::Loading => widgets::loading(),
                Load::Failed(error) => view.failure(error),
                Load::Ready(tiles) if tiles.is_empty() => {
                    widgets::empty("No favourite artists yet.")
                }
                Load::Ready(tiles) => {
                    widgets::tile_grid(tiles, view.artwork, |tile| Route::Artist {
                        id: tile.id.clone(),
                        name: tile.title.clone(),
                    })
                }
            },
        };
        let mut body =
            column![widgets::title_bar("Favourites", view.can_go_back), tabs].spacing(18);
        if let Some(menu) = view
            .menu
            .filter(|m| m.anchor == format!("{key}:favourites"))
        {
            body = body.push(widgets::menu_strip(menu, view));
        }
        widgets::page(body.push(content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lists_ask_for_favourites_only() {
        let query = favourites(Some("lib1".into()));
        assert!(query.favorites_only);
        assert_eq!(query.parent_id.as_deref(), Some("lib1"));
        assert_eq!(query.limit, None, "the whole list");
    }
}
