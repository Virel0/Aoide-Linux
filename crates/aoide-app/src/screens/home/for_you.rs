//! Home's For You shelf: Melete's songs for this listener from their own
//! play history (`/api/v1/music/for-you`, with ListenBrainz on), each with
//! "Because you listened to …". One in the library plays, with the shelf's
//! other playable songs after it, and has a song's menu; one that isn't can
//! be asked for, as Search's "Ask for it" asks. Fetched each time Home opens;
//! shown only for a list that is ready and has songs. An older server (404),
//! a list that is off, waiting or without history, and an error all hide it,
//! the error logged.

use aoide_melete::{ForYou, ForYouItem, ForYouSong, ForYouState, MusicRequest, Suggestion};
use aoide_store::PlaySource;
use iced::widget::{Column, button, column, container, mouse_area, row, scrollable, text};
use iced::{Center, Element, Fill, Length, Task};

use crate::actions::{PlayRequest, RowAction};
use crate::app::Message as App;
use crate::artwork::ArtKey;
use crate::ellipsis::ellipsis;
use crate::failure::Failure;
use crate::icons::{Icon, Tone, icon};
use crate::playback::tracks::TrackMeta;
use crate::screens::{Ctx, PageMsg, View, fetch};
use crate::{theme, widgets};

/// Songs asked of the server: the whole list it makes.
const LIMIT: u32 = 40;
/// How wide a tile's picture is, as on Home's other shelves.
const SIDE: f32 = 160.0;
/// What the shelf's menus are anchored under. A tile has no row below it to
/// open under, so its menu is drawn over the page, as an album tile's is.
const MENU: &str = "tile:for-you:";

/// The shelf, once the server has given a list worth showing.
#[derive(Debug)]
pub struct Shelf {
    entries: Vec<Entry>,
    /// The playable songs, in the list's order: what a tile plays from.
    tracks: Vec<TrackMeta>,
    /// An ask on its way, by place in `entries`.
    asking: Option<usize>,
    /// Asked for from here, by place: the request, or a sentence why not.
    asked: Vec<(usize, Result<MusicRequest, String>)>,
}

/// One song on the shelf.
#[derive(Debug)]
struct Entry {
    kind: Kind,
    /// The listener's song or artist it came from, to name.
    because: String,
    art: Option<ArtKey>,
}

#[derive(Debug, PartialEq)]
enum Kind {
    /// In the library: its place in [`Shelf::tracks`].
    Play(usize),
    /// Not in the library, to ask for.
    Ask(ForYouSong),
    /// Not in the library, and asked for already.
    Asked(ForYouSong),
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Option<ForYou>, Failure>),
    Ask(usize),
    Asked(usize, Result<Box<MusicRequest>, Failure>),
}

fn wrap(message: Message) -> PageMsg {
    PageMsg::Home(super::Message::ForYou(message))
}

/// Asks the server for this listener's list.
pub fn load(ctx: &Ctx<'_>) -> Task<App> {
    let client = ctx.client.clone();
    fetch(ctx.key, async move { client.for_you(LIMIT).await }, |r| {
        wrap(Message::Loaded(r))
    })
}

/// Whether the open menu is one of the shelf's, drawn over the page.
pub fn floats(anchor: &str) -> bool {
    anchor.starts_with(MENU)
}

/// The shelf for the server's answer; `None` hides it.
pub fn shelf(answer: Result<Option<ForYou>, Failure>) -> Option<Shelf> {
    let list = match answer {
        Ok(list) => list?,
        Err(error) => {
            log::warn!("The server didn't give the For You list: {error}");
            return None;
        }
    };
    if list.state != ForYouState::Ready || list.items.is_empty() {
        return None;
    }
    let mut tracks = Vec::new();
    let entries = list
        .items
        .into_iter()
        .map(|item| entry(item, &mut tracks))
        .collect();
    Some(Shelf {
        entries,
        tracks,
        asking: None,
        asked: Vec::new(),
    })
}

/// A song on the list as a tile, a playable one added to `tracks`.
fn entry(item: ForYouItem, tracks: &mut Vec<TrackMeta>) -> Entry {
    let poster = |song: &ForYouSong| {
        song.poster
            .as_deref()
            .map(|path| crate::posters::key_at(path, crate::artwork::TILE))
    };
    let (kind, art) = match item.suggestion {
        Suggestion::Playable(track) => {
            tracks.push(TrackMeta::from_item(&track));
            let art = track
                .artwork_source()
                .map(|(id, tag)| ArtKey::sized(id, tag, crate::artwork::TILE));
            (Kind::Play(tracks.len() - 1), art)
        }
        Suggestion::Requestable(song) => {
            let art = poster(&song);
            (Kind::Ask(song), art)
        }
        Suggestion::Requested { song, .. } => {
            let art = poster(&song);
            (Kind::Asked(song), art)
        }
    };
    Entry {
        kind,
        because: item.because.seed().to_owned(),
        art,
    }
}

impl Shelf {
    pub fn update(&mut self, message: Message, ctx: &Ctx<'_>) -> Task<App> {
        match message {
            // Home makes the shelf itself (`shelf`).
            Message::Loaded(_) => Task::none(),
            Message::Ask(place) => self.ask(place, ctx),
            Message::Asked(place, result) => {
                self.asking = None;
                self.asked.retain(|(p, _)| *p != place);
                self.asked
                    .push((place, result.map(|r| *r).map_err(|e| e.to_string())));
                Task::none()
            }
        }
    }

    /// Asks for the song at `place`, one at a time, as Search does.
    fn ask(&mut self, place: usize, ctx: &Ctx<'_>) -> Task<App> {
        let Some(Kind::Ask(song)) = self.entries.get(place).map(|e| &e.kind) else {
            return Task::none();
        };
        if self.asking.is_some() {
            return Task::none();
        }
        self.asking = Some(place);
        let candidate = song.candidate();
        let client = ctx.client.clone();
        fetch(
            ctx.key,
            async move { client.ask_for(&candidate).await.map(Box::new) },
            move |r| wrap(Message::Asked(place, r)),
        )
    }

    /// The tiles' pictures, to fetch.
    pub fn artwork(&self) -> impl Iterator<Item = ArtKey> + '_ {
        self.entries.iter().filter_map(|e| e.art.clone())
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let tiles = row(self
            .entries
            .iter()
            .enumerate()
            .map(|(place, entry)| self.tile(place, entry, view)))
        .spacing(12)
        .padding(iced::Padding::ZERO.bottom(14));
        column![
            text("For you").size(20).font(widgets::BOLD),
            scrollable(tiles).horizontal().width(Fill),
        ]
        .spacing(10)
        .into()
    }

    fn tile<'a>(&'a self, place: usize, entry: &'a Entry, view: &View<'a>) -> Element<'a, App> {
        let song = match &entry.kind {
            Kind::Play(index) => return self.playable(place, *index, entry, view),
            Kind::Ask(song) | Kind::Asked(song) => song,
        };
        let body = column![
            widgets::art(entry.art.as_ref(), view.artwork, Length::Fixed(SIDE), 8.0),
            lines(&song.title, &song.artist),
            because(entry),
            self.side(place, entry, view.key),
        ];
        container(body.spacing(4).width(SIDE).clip(true))
            .padding(4)
            .into()
    }

    /// A song in the library: a click plays it and the shelf's other
    /// playable songs after it, as a list plays from a row; its `…`, or a
    /// right click, opens a song's menu.
    fn playable<'a>(
        &'a self,
        place: usize,
        index: usize,
        entry: &'a Entry,
        view: &View<'a>,
    ) -> Element<'a, App> {
        let track = &self.tracks[index];
        let menu = App::Row(RowAction::Open {
            anchor: format!("{MENU}{}:{place}", view.key),
            tracks: vec![track.clone()],
        });
        let caption = row![
            lines(&track.title, &track.artist),
            button(icon(Icon::More, 16.0, Tone::Secondary))
                .style(theme::row)
                .padding(4)
                .on_press(menu.clone()),
        ]
        .align_y(Center);
        let body = column![
            widgets::art(entry.art.as_ref(), view.artwork, Length::Fixed(SIDE), 8.0),
            caption,
            because(entry),
        ]
        .spacing(4)
        .width(SIDE)
        .clip(true);
        let play = App::Play(PlayRequest::from(&self.tracks, index, PlaySource::Smart));
        mouse_area(button(body).padding(4).style(theme::row).on_press(play))
            .on_right_press(menu)
            .into()
    }

    /// Under a song to ask for: Ask for it, what asking came to, or that it
    /// is asked for already, in Search's words.
    fn side<'a>(&self, place: usize, entry: &Entry, key: u64) -> Element<'a, App> {
        match self.asked.iter().find(|(p, _)| *p == place).map(|(_, r)| r) {
            Some(Ok(request)) => return note(crate::screens::requests::standing(request)),
            Some(Err(why)) => return note(why.clone()),
            None => {}
        }
        if matches!(entry.kind, Kind::Asked(_)) {
            return note("Asked for".to_owned());
        }
        let busy = self.asking == Some(place);
        button(text(if busy { "Asking…" } else { "Ask for it" }).size(12))
            .style(theme::tonal)
            .padding([4, 12])
            .on_press_maybe(
                self.asking
                    .is_none()
                    .then(|| App::Page(key, wrap(Message::Ask(place)))),
            )
            .into()
    }
}

/// A tile's title and artist, each on one line.
fn lines<'a>(title: &'a str, artist: &'a str) -> Column<'a, App> {
    column![
        ellipsis(title).size(14).font(widgets::SEMIBOLD),
        ellipsis(artist)
            .size(13)
            .color(|t| Some(theme::secondary(t))),
    ]
    .spacing(2)
    .width(Fill)
    .clip(true)
}

/// "Because you listened to …" under a tile.
fn because<'a>(entry: &Entry) -> Element<'a, App> {
    note(format!("Because you listened to {}", entry.because))
}

/// A small line under a tile, in the secondary colour.
fn note<'a>(words: String) -> Element<'a, App> {
    widgets::secondary(words)
        .size(12)
        .wrapping(text::Wrapping::WordOrGlyph)
        .into()
}

#[cfg(test)]
mod tests;
