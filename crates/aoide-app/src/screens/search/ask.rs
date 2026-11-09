//! Asking for what the library doesn't have, under Search's results: the
//! words looked up on MusicBrainz through the server (albums, artists and
//! songs), each marked in the library, wanted or asked for already, else with
//! Ask. Looked up only when asked: MusicBrainz is slow and paced, and a lookup
//! at every pause in typing would be both. A song can also be put in a playlist
//! before it arrives (`playlists`). Each has its cover or portrait beside it
//! where the server gives one, a portrait with its credit (`poster`).

use aoide_melete::{Candidate, MusicRequest, RequestKind};
use iced::widget::{button, column, row, text};
use iced::{Center, Element, Fill, Task};

use crate::app::Message as App;
use crate::artwork::ArtKey;
use crate::failure::Failure;
use crate::screens::{Ctx, Load, PageMsg, View, fetch};
use crate::{theme, widgets};

mod playlists;
mod poster;

/// One lookup: its words, what it found, and what was asked for since.
#[derive(Debug)]
pub struct Asking {
    pub term: String,
    found: Load<Vec<Candidate>>,
    /// Asked for from here, by place in `found`: the answer, or a sentence why not.
    asked: Vec<(usize, Result<MusicRequest, String>)>,
    /// An ask on its way, by place in `found`.
    asking: Option<usize>,
    /// The song whose playlists are open to add it to, by place in `found`.
    choosing: Option<usize>,
    /// What is typed to find a playlist among many.
    filter: String,
    /// A song on its way into a playlist, by place in `found`.
    adding: Option<usize>,
    /// Put in a playlist from here, by place: the playlist's name, or why not.
    added: Vec<(usize, Result<String, String>)>,
}

#[derive(Debug, Clone)]
pub enum Message {
    LookUp,
    Found(String, Result<Vec<Candidate>, Failure>),
    Ask(usize),
    Asked(usize, Result<Box<MusicRequest>, Failure>),
    /// Show or hide the playlists to add the song at this place to.
    Choose(usize),
    Filter(String),
    /// Put the song at `place` in the playlist `id`, named `name`.
    AddTo {
        place: usize,
        id: String,
        name: String,
    },
    Added(usize, Result<Box<playlists::Added>, String>),
    /// A portrait's credit was pressed: its page, or its licence's.
    OpenLink(String),
}

fn wrap(message: Message) -> PageMsg {
    PageMsg::Search(super::Message::Ask(message))
}

/// Looks `term` up, albums first, then artists, then songs.
pub fn look_up(term: String, ctx: &Ctx<'_>) -> (Asking, Task<App>) {
    let client = ctx.client.clone();
    let words = term.clone();
    let words_shown = term.clone();
    let task = fetch(
        ctx.key,
        async move {
            let (albums, artists, songs) = tokio::try_join!(
                client.request_search(RequestKind::Album, &words),
                client.request_search(RequestKind::Artist, &words),
                client.request_search(RequestKind::Song, &words),
            )?;
            Ok([albums, artists, songs].concat())
        },
        move |r| wrap(Message::Found(term.clone(), r)),
    );
    let asking = Asking {
        term: words_shown,
        found: Load::Loading,
        asked: Vec::new(),
        asking: None,
        choosing: None,
        filter: String::new(),
        adding: None,
        added: Vec::new(),
    };
    (asking, task)
}

impl Asking {
    pub fn update(&mut self, message: Message, ctx: &Ctx<'_>) -> Task<App> {
        match message {
            // Search starts a lookup itself (`look_up`).
            Message::LookUp => Task::none(),
            Message::Found(term, result) => {
                if term == self.term {
                    self.found = Load::from(result);
                }
                Task::none()
            }
            Message::Ask(place) => {
                let Some(candidate) = self.found.ready().and_then(|f| f.get(place)).cloned() else {
                    return Task::none();
                };
                if self.asking.is_some() {
                    return Task::none();
                }
                self.asking = Some(place);
                let client = ctx.client.clone();
                fetch(
                    ctx.key,
                    async move { client.ask_for(&candidate).await.map(Box::new) },
                    move |r| wrap(Message::Asked(place, r)),
                )
            }
            Message::Asked(place, result) => {
                self.asking = None;
                self.answer(place, result.map(|r| *r).map_err(|e| e.to_string()));
                Task::none()
            }
            Message::Choose(place) => {
                self.choosing = (self.choosing != Some(place)).then_some(place);
                self.filter.clear();
                Task::none()
            }
            Message::Filter(filter) => {
                self.filter = filter;
                Task::none()
            }
            Message::AddTo { place, id, name } => self.add(place, id, name, ctx),
            Message::Added(place, result) => self.added(place, result),
            Message::OpenLink(url) => crate::posters::open(&url),
        }
    }

    /// The results' pictures, to fetch.
    pub fn artwork(&self) -> Vec<ArtKey> {
        self.found
            .ready()
            .map(|found| {
                found
                    .iter()
                    .filter_map(|c| c.poster.as_deref().map(crate::posters::key))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// What asking for the candidate at `place` came to.
    fn answer(&mut self, place: usize, answer: Result<MusicRequest, String>) {
        self.asked.retain(|(p, _)| *p != place);
        self.asked.push((place, answer));
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let heading = text("Ask for it").size(20).font(widgets::BOLD);
        let body: Element<'a, App> = match &self.found {
            Load::Loading => widgets::secondary("Looking it up on MusicBrainz…").into(),
            Load::Failed(error) => view.failure(error),
            Load::Ready(found) if found.is_empty() => {
                widgets::empty("MusicBrainz has nothing by that name.")
            }
            Load::Ready(found) => column(found.iter().enumerate().map(|(place, candidate)| {
                let row = self.row(place, candidate, view);
                if self.choosing == Some(place) {
                    column![row, self.choices(place, view)].spacing(6).into()
                } else {
                    row
                }
            }))
            .spacing(8)
            .into(),
        };
        column![heading, body].spacing(12).into()
    }

    fn row<'a>(&self, place: usize, candidate: &'a Candidate, view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let mut about = vec![kind_word(candidate.kind).to_owned()];
        about.extend(candidate.artist.clone());
        about.extend(candidate.year.map(|y| y.to_string()));
        // A detail that only says its kind again (an album whose type is Album).
        about.extend(
            candidate
                .detail
                .clone()
                .filter(|d| !d.eq_ignore_ascii_case(kind_word(candidate.kind))),
        );
        let mut words = column![
            text(candidate.title.as_str())
                .size(14)
                .font(widgets::SEMIBOLD),
            widgets::secondary(about.join(" · ")).size(12),
        ]
        .spacing(2)
        .width(Fill);
        if let Some(credit) = poster::credit(candidate, key) {
            words = words.push(credit);
        }
        let answered = self.asked.iter().find(|(p, _)| *p == place).map(|(_, r)| r);
        let side: Element<'a, App> = match answered {
            Some(Ok(request)) => note(super::super::requests::standing(request)),
            Some(Err(why)) => note(why.clone()),
            None if candidate.in_library => note("In your library".to_owned()),
            None if candidate.wanted => note("Wanted".to_owned()),
            None if candidate.requested.is_some() => note("Asked for".to_owned()),
            None => {
                let busy = self.asking == Some(place);
                button(text(if busy { "Asking…" } else { "Ask" }).size(12))
                    .style(theme::tonal)
                    .padding([4, 12])
                    .on_press_maybe(
                        (self.asking.is_none())
                            .then_some(App::Page(key, wrap(Message::Ask(place)))),
                    )
                    .into()
            }
        };
        let side = match self.added_note(place) {
            Some(added) => column![side, note(added)].spacing(2).into(),
            None => side,
        };
        let mut line = row![].spacing(12).align_y(Center);
        if let Some(picture) = poster::picture(candidate, view.artwork) {
            line = line.push(picture);
        }
        line = line.push(words);
        if let Some(add) = self.add_button(place, candidate, key) {
            line = line.push(add);
        }
        line.push(side).into()
    }
}

fn note<'a>(words: String) -> Element<'a, App> {
    widgets::secondary(words).size(12).into()
}

fn kind_word(kind: RequestKind) -> &'static str {
    match kind {
        RequestKind::Artist => "Artist",
        RequestKind::Album => "Album",
        RequestKind::Song => "Song",
        RequestKind::Other => "Title",
    }
}
