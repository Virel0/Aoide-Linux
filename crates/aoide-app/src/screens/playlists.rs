//! Aoide playlists: the ones kept on this computer and synced through the
//! sidecar.

use iced::widget::{button, column, row, text, text_input};
use iced::{Center, Element, Fill, Task};

use super::{Ctx, PageMsg, Route, View};
use crate::actions::{Collection, RowAction};
use crate::app::Message as App;
use crate::icons::{Icon, Tone, icon};
use crate::theme;
use crate::widgets;

#[derive(Debug, Default)]
pub struct State {
    new_name: String,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    NewName(String),
    Create,
    Created(Result<String, String>),
}

impl State {
    pub fn update(&mut self, message: Message, ctx: &Ctx<'_>) -> Task<App> {
        match message {
            Message::NewName(name) => {
                self.new_name = name;
                Task::none()
            }
            Message::Create => {
                let name = self.new_name.trim().to_owned();
                if name.is_empty() {
                    return Task::none();
                }
                self.new_name.clear();
                let store = ctx.store.clone();
                let key = ctx.key;
                Task::perform(
                    async move {
                        let now = aoide_store::now_millis();
                        store
                            .call(move |w| {
                                w.store
                                    .create_playlist(&name, None, None, now)
                                    .map(|p| p.id)
                                    .map_err(|e| e.to_string())
                            })
                            .await
                            .unwrap_or_else(|| Err("the library is closing".into()))
                    },
                    move |r| App::Page(key, PageMsg::Playlists(Message::Created(r))),
                )
            }
            Message::Created(result) => match result {
                Ok(_) => {
                    self.error = None;
                    Task::done(App::PlaylistsChanged)
                }
                Err(error) => {
                    self.error = Some(format!("The playlist couldn't be made: {error}"));
                    Task::none()
                }
            },
        }
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let create = row![
            text_input("New playlist name", &self.new_name)
                .on_input(move |s| App::Page(key, PageMsg::Playlists(Message::NewName(s))))
                .on_submit(App::Page(key, PageMsg::Playlists(Message::Create)))
                .padding(8)
                .style(theme::field),
            button(text("New Playlist"))
                .style(theme::filled)
                .padding([8, 14])
                .on_press_maybe(
                    (!self.new_name.trim().is_empty())
                        .then_some(App::Page(key, PageMsg::Playlists(Message::Create)))
                ),
        ]
        .spacing(10)
        .align_y(Center);
        let mut body = column![
            widgets::title_bar("Playlists", view.can_go_back),
            super::jellyfin_playlists::switcher(false),
            create
        ]
        .spacing(20);
        if let Some(error) = &self.error {
            body = body.push(widgets::failure(error));
        }
        if view.playlists.is_empty() {
            body = body.push(widgets::empty(
                "No playlists yet. Make one here, or add songs to a new one from any song's … menu.",
            ));
        }
        let mut list = column![].spacing(2);
        for playlist in view.playlists {
            let count = match (playlist.smart, playlist.count) {
                (true, 1) => "Smart · 1 song".to_owned(),
                (true, n) => format!("Smart · {n} songs"),
                (false, 1) => "1 song".to_owned(),
                (false, n) => format!("{n} songs"),
            };
            let glyph = if playlist.smart {
                Icon::Sparkle
            } else {
                Icon::Playlist
            };
            let more = button(icon(Icon::More, 18.0, Tone::Secondary))
                .style(theme::row)
                .padding(6)
                .on_press(App::Row(RowAction::OpenCollection {
                    anchor: format!("playlist:{}", playlist.id),
                    collection: Collection::Playlist(playlist.id.clone()),
                    title: playlist.name.clone(),
                }));
            list = list.push(
                button(
                    row![
                        icon(glyph, 22.0, Tone::Accent),
                        text(playlist.name.as_str()).size(15).width(Fill),
                        widgets::secondary(count).size(13),
                        more,
                    ]
                    .spacing(12)
                    .align_y(Center),
                )
                .width(Fill)
                .padding([8, 10])
                .style(theme::row)
                .on_press(App::Open(Route::Playlist {
                    id: playlist.id.clone(),
                })),
            );
        }
        widgets::page(body.push(list))
    }
}
