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
    pub id: String,
    pub title: String,
    album: Option<Item>,
    pub art: Option<ArtKey>,
    pub tracks: Load<Vec<TrackMeta>>,
    /// "You finish 78% of this album's songs", once there is enough history.
    finish: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Album(Result<Vec<Item>, String>),
    Tracks(Result<Vec<Item>, String>),
    Finish(Option<String>),
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
            id: id.to_owned(),
            title: title.to_owned(),
            album: None,
            art: None,
            tracks: Load::Loading,
            finish: None,
        },
        tasks,
    )
}

impl State {
    pub fn update(&mut self, message: Message, ctx: &Ctx<'_>) -> Task<App> {
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
                self.tracks = Load::from(result.map(|items| {
                    let mut tracks: Vec<TrackMeta> =
                        items.iter().map(TrackMeta::from_item).collect();
                    album_order(&mut tracks);
                    tracks
                }));
                if let Load::Ready(tracks) = &self.tracks {
                    let ids = tracks.iter().map(|t| t.id.clone()).collect();
                    let key = ctx.key;
                    return Task::perform(
                        crate::listening::finish_of(ctx.store.clone(), ids, "this album's songs"),
                        move |line| App::Page(key, PageMsg::Album(Message::Finish(line))),
                    );
                }
            }
            Message::Finish(line) => self.finish = line,
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
        info = info.push(widgets::secondary(facts.join(" · ")).size(13));
        if let Some(finish) = &self.finish {
            info = info.push(widgets::secondary(finish.as_str()).size(13));
        }
        info = info.push(widgets::play_buttons(
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
            Load::Ready(tracks) => widgets::track_list_with_headings(
                tracks,
                view,
                ListStyle {
                    numbered: true,
                    show_album: false,
                },
                PlaySource::Album,
                &disc_headings(tracks),
            ),
        });
        crate::handoff::played_from(widgets::page(body), Some(self.context()))
    }

    /// What playing from this page is remembered as.
    fn context(&self) -> aoide_store::RecentContext {
        let mut context = aoide_store::RecentContext::new(
            aoide_store::RecentContextKind::Album,
            self.id.clone(),
            self.title.clone(),
            aoide_store::now_millis(),
        );
        context.subtitle = self.album.as_ref().and_then(|a| {
            a.album_artist
                .clone()
                .or_else(|| a.album_artists.first().map(|n| n.name.clone()))
        });
        context
    }
}

/// Disc, then track: the order an album plays in. The server is asked for it
/// already; this keeps it whatever comes back. Ties keep the server's order.
pub fn album_order(tracks: &mut [TrackMeta]) {
    tracks.sort_by_key(|t| {
        (
            t.disc_number.unwrap_or(1),
            t.track_number.unwrap_or(i32::MAX),
        )
    });
}

/// "Disc 1", "Disc 2"… before the first track of each disc, on an album with
/// more than one; nothing on a single disc.
pub fn disc_headings(tracks: &[TrackMeta]) -> Vec<(usize, String)> {
    let mut headings = Vec::new();
    let mut last = None;
    for (index, track) in tracks.iter().enumerate() {
        let disc = track.disc_number.unwrap_or(1);
        if last != Some(disc) {
            headings.push((index, format!("Disc {disc}")));
            last = Some(disc);
        }
    }
    if headings.len() < 2 {
        headings.clear();
    }
    headings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(disc: Option<i32>, number: Option<i32>) -> TrackMeta {
        TrackMeta::from_item(
            &serde_json::from_value(serde_json::json!({
                "Id": format!("{disc:?}-{number:?}"), "Name": "t", "Type": "Audio",
                "ParentIndexNumber": disc, "IndexNumber": number,
            }))
            .unwrap(),
        )
    }

    #[test]
    fn tracks_play_disc_by_disc_in_track_order() {
        let mut tracks = vec![
            track(Some(2), Some(1)),
            track(Some(1), Some(2)),
            track(None, None),
            track(Some(1), Some(1)),
            track(Some(2), Some(2)),
        ];
        album_order(&mut tracks);
        let order: Vec<_> = tracks
            .iter()
            .map(|t| (t.disc_number, t.track_number))
            .collect();
        assert_eq!(
            order,
            [
                (Some(1), Some(1)),
                (Some(1), Some(2)),
                (None, None),
                (Some(2), Some(1)),
                (Some(2), Some(2)),
            ]
        );
    }

    #[test]
    fn only_a_multi_disc_album_gets_disc_headings() {
        let mut tracks = vec![
            track(Some(1), Some(1)),
            track(Some(1), Some(2)),
            track(Some(2), Some(1)),
        ];
        assert_eq!(
            disc_headings(&tracks),
            [(0, "Disc 1".to_owned()), (2, "Disc 2".to_owned())]
        );
        tracks.truncate(2);
        assert!(disc_headings(&tracks).is_empty());
        assert!(disc_headings(&[track(None, Some(1))]).is_empty());
    }
}
