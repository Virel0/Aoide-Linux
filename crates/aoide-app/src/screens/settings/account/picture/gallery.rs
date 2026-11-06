//! The server's gallery of profile pictures, while it is open: its
//! categories, a page of the chosen one's pictures at a time, each read with
//! the token and kept only while the gallery is open.

use aoide_melete::{AvatarCategory, GalleryPage};
use iced::widget::{button, column, image, row, text};
use iced::{ContentFit, Element, Task};

use super::{Msg as Picture, send};
use crate::app::Message as App;
use crate::screens::settings::account::{Ctx, Msg as Account, action, ask, plainly, problem};
use crate::{theme, widgets};

/// Pictures asked for at a time.
const PAGE: u64 = 24;
/// A picture's side in the grid.
const SIDE: f32 = 64.0;

#[derive(Debug, Default)]
pub struct Gallery {
    pub categories: Option<Result<Vec<AvatarCategory>, String>>,
    /// The category shown.
    pub category: Option<String>,
    /// Its pictures so far: the id, and the picture once read.
    pub pictures: Vec<(String, Option<Shown>)>,
    pub total: u64,
    pub loading: bool,
    pub problem: Option<String>,
}

/// A picture read from the gallery: its bytes, kept to show as the new
/// picture at once if it is chosen, and the handle drawn.
#[derive(Debug, Clone)]
pub struct Shown {
    bytes: Vec<u8>,
    handle: image::Handle,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Open,
    Close,
    Categories(Result<Vec<AvatarCategory>, String>),
    Category(String),
    Page(String, Result<GalleryPage, String>),
    More,
    Read(String, Option<Vec<u8>>),
    Pick(String),
}

fn wrap(msg: Msg) -> Account {
    Account::Picture(Picture::Gallery(msg))
}

impl Gallery {
    pub fn open(ctx: &Ctx<'_>) -> (Self, Task<App>) {
        let client = ctx.client.clone();
        let task = ask(
            ctx.key,
            async move { client.avatar_categories().await },
            |r| wrap(Msg::Categories(r.map_err(|e| plainly(&e)))),
        );
        (Self::default(), task)
    }

    pub fn update(&mut self, msg: Msg, ctx: &Ctx<'_>) -> Task<App> {
        match msg {
            Msg::Categories(Ok(categories)) => {
                let first = categories.first().map(|c| c.id.clone());
                self.categories = Some(Ok(categories));
                match first {
                    Some(id) => self.show(id, ctx),
                    None => Task::none(),
                }
            }
            Msg::Categories(Err(words)) => {
                self.categories = Some(Err(words));
                Task::none()
            }
            Msg::Category(id) => self.show(id, ctx),
            Msg::Page(category, result) => self.page_arrived(category, result, ctx),
            Msg::More => self.ask_page(ctx),
            Msg::Read(id, bytes) => {
                if let Some((_, shown)) = self.pictures.iter_mut().find(|(p, _)| *p == id) {
                    *shown = bytes.map(|bytes| Shown {
                        handle: image::Handle::from_bytes(bytes.clone()),
                        bytes,
                    });
                }
                Task::none()
            }
            Msg::Open | Msg::Close | Msg::Pick(_) => Task::none(),
        }
    }

    /// The bytes of picture `id`, once read.
    pub fn bytes_of(&self, id: &str) -> Option<Vec<u8>> {
        self.pictures
            .iter()
            .find(|(p, _)| p == id)
            .and_then(|(_, shown)| shown.as_ref().map(|s| s.bytes.clone()))
    }

    fn show(&mut self, id: String, ctx: &Ctx<'_>) -> Task<App> {
        self.category = Some(id);
        self.pictures.clear();
        self.total = 0;
        self.ask_page(ctx)
    }

    fn ask_page(&mut self, ctx: &Ctx<'_>) -> Task<App> {
        let Some(category) = self.category.clone() else {
            return Task::none();
        };
        self.loading = true;
        self.problem = None;
        let client = ctx.client.clone();
        let offset = self.pictures.len() as u64;
        let asked = category.clone();
        ask(
            ctx.key,
            async move { client.gallery_pictures(&asked, offset, PAGE).await },
            move |r| wrap(Msg::Page(category.clone(), r.map_err(|e| plainly(&e)))),
        )
    }

    fn page_arrived(
        &mut self,
        category: String,
        result: Result<GalleryPage, String>,
        ctx: &Ctx<'_>,
    ) -> Task<App> {
        if self.category.as_deref() != Some(category.as_str()) {
            return Task::none();
        }
        self.loading = false;
        let page = match result {
            Ok(page) => page,
            Err(words) => {
                self.problem = Some(words);
                return Task::none();
            }
        };
        self.total = page.total;
        let mut reads = Vec::new();
        for picture in page.pictures {
            let client = ctx.client.clone();
            let id = picture.id.clone();
            reads.push(Task::perform(
                async move { client.gallery_picture(&id).await.ok() },
                {
                    let id = picture.id.clone();
                    let key = ctx.key;
                    move |bytes| send(key)(Picture::Gallery(Msg::Read(id.clone(), bytes)))
                },
            ));
            self.pictures.push((picture.id, None));
        }
        Task::batch(reads)
    }

    pub fn view(&self, idle: bool, key: u64) -> Element<'_, App> {
        let send = move |m: Msg| send(key)(Picture::Gallery(m));
        let mut rows = column![].spacing(10);
        match &self.categories {
            None => rows = rows.push(widgets::secondary("Loading the gallery…").size(13)),
            Some(Err(words)) => rows = rows.push(problem(words)),
            Some(Ok(categories)) if categories.is_empty() => {
                rows = rows
                    .push(widgets::secondary("Your server's gallery has no pictures.").size(13));
            }
            Some(Ok(categories)) => {
                let mut tabs = row![].spacing(4);
                for category in categories {
                    let chosen = self.category.as_deref() == Some(category.id.as_str());
                    tabs = tabs.push(
                        button(text(category.name.as_str()).size(13))
                            .padding([4, 10])
                            .style(if chosen {
                                theme::row_selected
                            } else {
                                theme::row
                            })
                            .on_press(send(Msg::Category(category.id.clone()))),
                    );
                }
                rows = rows.push(tabs.wrap().vertical_spacing(4));
                rows = rows.push(self.grid(idle, key));
            }
        }
        if let Some(words) = &self.problem {
            rows = rows.push(problem(words));
        }
        let more = (self.pictures.len() as u64) < self.total;
        let mut actions = row![].spacing(8);
        if more {
            actions = actions.push(action(
                "Show more",
                theme::tonal,
                (!self.loading).then(|| send(Msg::More)),
            ));
        }
        actions = actions.push(action(
            "Close the gallery",
            theme::tonal,
            Some(send(Msg::Close)),
        ));
        rows.push(actions).into()
    }

    fn grid(&self, idle: bool, key: u64) -> Element<'_, App> {
        let pick = move |id: &str| send(key)(Picture::Gallery(Msg::Pick(id.to_owned())));
        let mut grid = row![].spacing(8);
        for (id, shown) in &self.pictures {
            let face: Element<'_, App> = match shown {
                Some(shown) => image(shown.handle.clone())
                    .width(SIDE)
                    .height(SIDE)
                    .content_fit(ContentFit::Cover)
                    .border_radius(SIDE / 2.0)
                    .into(),
                None => iced::widget::container(iced::widget::space())
                    .width(SIDE)
                    .height(SIDE)
                    .style(theme::placeholder)
                    .into(),
            };
            grid = grid.push(
                button(face)
                    .padding(2)
                    .style(theme::row)
                    .on_press_maybe((idle && shown.is_some()).then(|| pick(id))),
            );
        }
        if self.loading && self.pictures.is_empty() {
            return widgets::secondary("Loading…").size(13).into();
        }
        grid.wrap().vertical_spacing(8).into()
    }
}
