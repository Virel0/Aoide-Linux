//! Songs: every song by title, a page at a time.

use aoide_jellyfin::{Item, ListQuery, Page as ItemPage};
use aoide_store::PlaySource;
use iced::widget::{column, sensor};
use iced::{Element, Task};

use super::{Ctx, PageMsg, View, fetch};
use crate::app::Message as App;
use crate::failure::Failure;
use crate::playback::tracks::TrackMeta;
use crate::widgets::{self, ListStyle};

const PAGE: u32 = 200;

#[derive(Debug)]
pub struct State {
    pub tracks: Vec<TrackMeta>,
    total: Option<u32>,
    loading: bool,
    error: Option<Failure>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<ItemPage<Item>, Failure>),
    More,
}

pub fn open(ctx: &Ctx<'_>) -> (State, Task<App>) {
    let mut state = State {
        tracks: Vec::new(),
        total: None,
        loading: false,
        error: None,
    };
    let task = state.load(ctx);
    (state, task)
}

impl State {
    fn load(&mut self, ctx: &Ctx<'_>) -> Task<App> {
        if self.loading || self.total.is_some_and(|t| self.tracks.len() as u32 >= t) {
            return Task::none();
        }
        self.loading = true;
        let query = ListQuery {
            parent_id: ctx.library.map(str::to_owned),
            start_index: self.tracks.len() as u32,
            limit: Some(PAGE),
            ..ListQuery::default()
        };
        let client = ctx.client.clone();
        fetch(ctx.key, async move { client.songs(&query).await }, |r| {
            PageMsg::Songs(Message::Loaded(r))
        })
    }

    pub fn update(&mut self, message: Message, ctx: &Ctx<'_>) -> Task<App> {
        match message {
            Message::Loaded(result) => {
                self.loading = false;
                match result {
                    Ok(page) => {
                        self.total = Some(page.total_record_count);
                        self.tracks
                            .extend(page.items.iter().map(TrackMeta::from_item));
                        if page.items.is_empty() {
                            self.total = Some(self.tracks.len() as u32);
                        }
                    }
                    Err(error) => self.error = Some(error),
                }
                Task::none()
            }
            Message::More => self.load(ctx),
        }
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let mut body = column![
            widgets::title_bar("Songs", view.can_go_back),
            widgets::track_list(
                &self.tracks,
                view,
                ListStyle {
                    numbered: false,
                    show_album: true,
                },
                PlaySource::Unknown,
            ),
        ]
        .spacing(16);
        if let Some(error) = &self.error {
            body = body.push(view.failure(error));
        } else if self.total.is_none_or(|t| (self.tracks.len() as u32) < t) {
            body = body.push(
                sensor(widgets::loading())
                    .key(self.tracks.len())
                    .anticipate(600)
                    .on_show(move |_| App::Page(key, PageMsg::Songs(Message::More))),
            );
        } else if self.tracks.is_empty() {
            body = body.push(widgets::empty("No songs in this library."));
        }
        widgets::page(body)
    }
}
