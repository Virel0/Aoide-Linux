//! Favourites: the songs this listener likes (Melete's likes, the sync log's
//! own), and the albums and artists of those songs, one tab each. Melete keeps
//! likes on songs only.

use aoide_melete::{Item, ItemKind};
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
    Liked(Result<Vec<Item>, Failure>),
}

/// The artists credited on `songs`, each once, in the order first met.
fn artists_of(songs: &[Item]) -> Vec<Tile> {
    let mut seen = std::collections::HashSet::new();
    songs
        .iter()
        .flat_map(|song| song.artist_items.iter())
        .filter(|artist| seen.insert(artist.id.clone()))
        .map(|artist| {
            Tile::artist(&Item {
                id: artist.id.clone(),
                name: artist.name.clone(),
                kind: ItemKind::Artist,
                ..Item::default()
            })
        })
        .collect()
}

pub fn open(ctx: &Ctx<'_>) -> (State, Task<App>) {
    let client = ctx.client.clone();
    let task = fetch(ctx.key, async move { client.liked_songs().await }, |r| {
        PageMsg::Favourites(Message::Liked(r))
    });
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
            Message::Liked(Ok(songs)) => {
                self.albums = Load::Ready(crate::library::albums_of_tracks(&songs, usize::MAX));
                self.artists = Load::Ready(artists_of(&songs));
                self.songs = Load::Ready(songs.iter().map(TrackMeta::from_item).collect());
            }
            Message::Liked(Err(error)) => {
                self.albums = Load::Failed(error.clone());
                self.artists = Load::Failed(error.clone());
                self.songs = Load::Failed(error);
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
                    widgets::empty("No liked songs yet. Like one from any song's … menu.")
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
                    widgets::empty("No albums yet: they're the ones with a song you like.")
                }
                Load::Ready(tiles) => {
                    widgets::tile_grid(tiles, view.artwork, super::home::open_album)
                }
            },
            Tab::Artists => match &self.artists {
                Load::Loading => widgets::loading(),
                Load::Failed(error) => view.failure(error),
                Load::Ready(tiles) if tiles.is_empty() => {
                    widgets::empty("No artists yet: they're the ones with a song you like.")
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
    use aoide_melete::NameId;

    #[test]
    fn the_artists_tab_is_the_liked_songs_artists_once_each() {
        let credit = |id: &str, name: &str| NameId {
            id: id.into(),
            name: name.into(),
        };
        let song = |id: &str, artists: Vec<NameId>| Item {
            id: id.into(),
            artist_items: artists,
            ..Item::default()
        };
        let songs = [
            song("1", vec![credit("7", "Mara Vey")]),
            song("2", vec![credit("8", "Odd Hours"), credit("7", "Mara Vey")]),
        ];
        let names: Vec<String> = artists_of(&songs).into_iter().map(|t| t.title).collect();
        assert_eq!(names, ["Mara Vey", "Odd Hours"]);
    }
}
