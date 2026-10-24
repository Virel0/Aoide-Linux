//! An album: its cover, artist and year, Play and Shuffle, and its tracks.

use aoide_jellyfin::Item;
use aoide_store::PlaySource;
use iced::widget::{button, column, row, text};
use iced::{Element, Length, Task};

use super::{Ctx, Load, PageMsg, Route, View, fetch};
use crate::app::Message as App;
use crate::artwork::ArtKey;
use crate::playback::tracks::TrackMeta;
use crate::theme;
use crate::widgets::{self, ListStyle};

#[derive(Debug)]
pub struct State {
    pub title: String,
    album: Option<Item>,
    pub art: Option<ArtKey>,
    pub tracks: Load<Vec<TrackMeta>>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Album(Result<Vec<Item>, String>),
    Tracks(Result<Vec<Item>, String>),
}

pub fn open(ctx: &Ctx<'_>, id: &str, title: &str) -> (State, Task<App>) {
    let (a, b) = (ctx.client.clone(), ctx.client.clone());
    let (ia, ib) = (id.to_owned(), id.to_owned());
    let tasks = Task::batch([
        fetch(ctx.key, async move { a.items_by_ids(&[ia]).await }, |r| {
            PageMsg::Album(Message::Album(r))
        }),
        fetch(ctx.key, async move { b.album_tracks(&ib).await }, |r| {
            PageMsg::Album(Message::Tracks(r))
        }),
    ]);
    (
        State {
            title: title.to_owned(),
            album: None,
            art: None,
            tracks: Load::Loading,
        },
        tasks,
    )
}

impl State {
    pub fn update(&mut self, message: Message) -> Task<App> {
        match message {
            Message::Album(Ok(items)) => {
                if let Some(album) = items.into_iter().next() {
                    self.title = album.name.clone();
                    self.art = album.artwork_source().map(|(id, tag)| ArtKey::new(id, tag));
                    self.album = Some(album);
                }
            }
            Message::Album(Err(_)) => {}
            Message::Tracks(result) => {
                self.tracks = Load::from(
                    result.map(|items| items.iter().map(TrackMeta::from_item).collect()),
                );
            }
        }
        Task::none()
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let artist = self
            .album
            .as_ref()
            .and_then(|a| a.album_artists.first().cloned().map(|n| (n.id, n.name)))
            .or_else(|| {
                self.album
                    .as_ref()
                    .and_then(|a| a.album_artist.clone())
                    .map(|name| (String::new(), name))
            });
        let tracks: &[TrackMeta] = self.tracks.ready().map(Vec::as_slice).unwrap_or(&[]);
        let total_ms: i64 = tracks.iter().filter_map(|t| t.duration_ms).sum();
        let mut facts = Vec::new();
        if let Some(year) = self.album.as_ref().and_then(|a| a.production_year) {
            facts.push(year.to_string());
        }
        if !tracks.is_empty() {
            facts.push(match tracks.len() {
                1 => "1 song".to_owned(),
                n => format!("{n} songs"),
            });
            facts.push(format!("{} min", total_ms / 60_000));
        }

        let mut info = column![text(self.title.as_str()).size(30).font(widgets::BOLD),].spacing(8);
        if let Some((id, name)) = artist {
            let label = text(name.clone()).size(20).style(|t| text::Style {
                color: Some(theme::accent(t)),
            });
            info = info.push(if id.is_empty() {
                Element::from(label)
            } else {
                button(label)
                    .padding(0)
                    .style(theme::row)
                    .on_press(App::Open(Route::Artist { id, name }))
                    .into()
            });
        }
        info = info
            .push(widgets::secondary(facts.join(" · ")).size(13))
            .push(widgets::play_buttons(
                tracks,
                format!("{}:album", view.key),
                PlaySource::Album,
            ));

        let mut body = column![
            widgets::title_bar("", view.can_go_back),
            row![
                widgets::art(self.art.as_ref(), view.artwork, Length::Fixed(220.0), 10.0),
                info.width(Length::Fill)
            ]
            .spacing(24),
        ]
        .spacing(16);
        if let Some(menu) = view
            .menu
            .filter(|m| m.anchor == format!("{}:album", view.key))
        {
            body = body.push(widgets::menu_strip(menu, view));
        }
        body = body.push(match &self.tracks {
            Load::Loading => widgets::loading(),
            Load::Failed(error) => widgets::failure(error),
            Load::Ready(tracks) => widgets::track_list(
                tracks,
                view,
                ListStyle {
                    numbered: true,
                    show_album: false,
                },
                PlaySource::Album,
            ),
        });
        widgets::page(body)
    }
}
