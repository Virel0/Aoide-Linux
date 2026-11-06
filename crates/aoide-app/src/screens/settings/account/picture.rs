//! The profile picture: a file from this computer (made smaller first when
//! it is over the server's 5 MB), one from the server's gallery, or none.
//! A new picture is shown at once: the app keeps the bytes sent under the
//! picture's new address (`app/settings.rs`).

use aoide_melete::{AVATAR_MAX_BYTES, Error};
use iced::widget::{column, row};
use iced::{Center, Element, Task};

use super::{Ctx, Msg as Account, action, ask, plainly, problem, wrap};
use crate::app::Message as App;
use crate::artwork::Artwork;
use crate::screens::settings::section;
use crate::{theme, widgets};

mod gallery;
mod shrink;

pub use gallery::Gallery;

/// The picture's side on the pane.
const SHOWN_AT: f32 = 72.0;

#[derive(Debug, Default)]
pub struct State {
    pub busy: bool,
    pub note: Option<Result<String, String>>,
    /// The server's gallery, while it is open.
    pub gallery: Option<Gallery>,
}

/// A picture the server took: its address now, and the bytes sent when the
/// app has them, to show before it is fetched.
#[derive(Debug, Clone)]
pub struct Set {
    pub url: Option<String>,
    pub bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    ChooseFile,
    Read(Option<Result<Vec<u8>, String>>),
    Remove,
    Set(Result<Set, String>),
    Gallery(gallery::Msg),
}

fn send(key: u64) -> impl Fn(Msg) -> App + Copy {
    move |m| wrap(key, Account::Picture(m))
}

impl State {
    pub fn update(&mut self, msg: Msg, ctx: &Ctx<'_>) -> Task<App> {
        let key = ctx.key;
        match msg {
            Msg::ChooseFile => Task::perform(
                crate::covers::pick_picture("Choose a profile picture"),
                move |read| send(key)(Msg::Read(read)),
            ),
            Msg::Read(None) => Task::none(),
            Msg::Read(Some(Err(error))) => {
                self.note = Some(Err(format!("That file couldn't be read: {error}")));
                Task::none()
            }
            Msg::Read(Some(Ok(bytes))) => {
                self.start();
                upload(bytes, ctx)
            }
            Msg::Remove => {
                self.start();
                let client = ctx.client.clone();
                ask(key, async move { client.remove_avatar().await }, |r| {
                    Account::Picture(Msg::Set(
                        r.map(|url| Set { url, bytes: None })
                            .map_err(|e| refused(&e)),
                    ))
                })
            }
            Msg::Set(result) => {
                self.busy = false;
                self.note = Some(result.map(|set| {
                    if set.url.is_some() {
                        "Your profile picture is changed.".to_owned()
                    } else {
                        "Your profile picture is removed.".to_owned()
                    }
                }));
                if self.note.as_ref().is_some_and(Result::is_ok) {
                    self.gallery = None;
                }
                Task::none()
            }
            Msg::Gallery(msg) => self.gallery_message(msg, ctx),
        }
    }

    fn gallery_message(&mut self, msg: gallery::Msg, ctx: &Ctx<'_>) -> Task<App> {
        match msg {
            gallery::Msg::Open => {
                let (gallery, task) = Gallery::open(ctx);
                self.gallery = Some(gallery);
                self.note = None;
                task
            }
            gallery::Msg::Close => {
                self.close_gallery();
                Task::none()
            }
            gallery::Msg::Pick(id) => {
                self.start();
                let bytes = self.gallery.as_ref().and_then(|g| g.bytes_of(&id));
                let client = ctx.client.clone();
                ask(
                    ctx.key,
                    async move { client.set_gallery_avatar(&id).await },
                    move |r| {
                        Account::Picture(Msg::Set(
                            r.map(|url| Set {
                                url,
                                bytes: bytes.clone(),
                            })
                            .map_err(|e| refused(&e)),
                        ))
                    },
                )
            }
            other => match &mut self.gallery {
                Some(gallery) => gallery.update(other, ctx),
                None => Task::none(),
            },
        }
    }

    fn start(&mut self) {
        self.busy = true;
        self.note = None;
    }

    pub fn close_gallery(&mut self) {
        self.gallery = None;
    }

    pub fn view<'a>(
        &'a self,
        name: &str,
        picture: Option<&str>,
        artwork: &'a Artwork,
        key: u64,
    ) -> Element<'a, App> {
        let send = send(key);
        let idle = !self.busy;
        let gallery_closed = self.gallery.is_none();
        let buttons = row![
            action(
                "Choose a file…",
                theme::tonal,
                idle.then(|| send(Msg::ChooseFile))
            ),
            action(
                "Choose from the gallery…",
                theme::tonal,
                (idle && gallery_closed).then(|| send(Msg::Gallery(gallery::Msg::Open)))
            ),
            action(
                "Remove",
                theme::tonal,
                (idle && picture.is_some()).then(|| send(Msg::Remove))
            ),
        ]
        .spacing(8)
        .wrap();
        let mut rows = column![
            row![widgets::avatar(name, picture, artwork, SHOWN_AT), buttons]
                .spacing(16)
                .align_y(Center)
        ];
        if self.busy {
            rows = rows.push(widgets::secondary("Changing your picture…").size(13));
        }
        match &self.note {
            Some(Ok(words)) => rows = rows.push(widgets::secondary(words.as_str()).size(13)),
            Some(Err(words)) => rows = rows.push(problem(words)),
            None => {}
        }
        if let Some(gallery) = &self.gallery {
            rows = rows.push(gallery.view(idle, key));
        }
        section(
            "Profile picture",
            rows,
            "Everyone on your server sees it beside your name. A JPEG, PNG or WebP picture; one \
             over 5 MB is made smaller first.",
        )
    }
}

/// Makes the file fit the server's rules, then sends it.
fn upload(bytes: Vec<u8>, ctx: &Ctx<'_>) -> Task<App> {
    let client = ctx.client.clone();
    let key = ctx.key;
    Task::perform(
        async move {
            let prepared = tokio::task::spawn_blocking(move || shrink::to_fit(bytes))
                .await
                .map_err(|e| Unsent::Unreadable(e.to_string()))
                .and_then(|r| r.map_err(Unsent::Unreadable))?;
            let (bytes, kind) = prepared;
            match client.set_avatar(&bytes, kind).await {
                Ok(url) => Ok(Set {
                    url,
                    bytes: Some(bytes),
                }),
                Err(error) => Err(Unsent::Server(error)),
            }
        },
        move |result| match result {
            Err(Unsent::Server(Error::Unauthorized)) => App::TokenRejected,
            result => send(key)(Msg::Set(result.map_err(|e| match e {
                Unsent::Unreadable(words) => words,
                Unsent::Server(error) => refused(&error),
            }))),
        },
    )
}

/// Why a file wasn't set.
enum Unsent {
    /// It couldn't be made into a picture the server takes: in words.
    Unreadable(String),
    Server(Error),
}

/// The server's refusal in words.
pub fn refused(error: &Error) -> String {
    match error {
        Error::Http { status: 400, .. } => "Your server didn't take that picture: it isn't a \
             JPEG, PNG or WebP picture, or the file is damaged. Choose another."
            .to_owned(),
        Error::Http { status: 413, .. } => format!(
            "That picture is over {} MB, which your server doesn't take. Choose a smaller one.",
            AVATAR_MAX_BYTES / (1024 * 1024)
        ),
        Error::Http { status: 422, .. } => {
            "Your server can't use that gallery picture. Choose another.".to_owned()
        }
        Error::NotFound { .. } => {
            "That picture isn't in your server's gallery any more. Choose another.".to_owned()
        }
        other => plainly(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http(status: u16) -> Error {
        Error::Http {
            status,
            path: "api/v1/me/avatar".into(),
            detail: None,
            reason: None,
        }
    }

    #[test]
    fn each_refusal_says_what_to_do() {
        assert!(refused(&http(400)).contains("JPEG, PNG or WebP"));
        assert!(refused(&http(413)).contains("over 5 MB"));
        assert!(refused(&http(422)).contains("gallery picture"));
        let gone = Error::NotFound {
            path: "api/v1/me/avatar/gallery".into(),
        };
        assert!(refused(&gone).contains("any more"));
    }
}
