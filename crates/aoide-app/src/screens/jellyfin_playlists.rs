//! The server's own playlists (Jellyfin's, not Aoide's): the list, making a
//! new one, and the switch between the two kinds at the top of Playlists.
//! Each one opens in `jellyfin_playlist`.

use aoide_jellyfin::{Item, ListQuery, Sort, SortOrder};
use iced::widget::{button, column, row, text, text_input};
use iced::{Center, Element, Fill, Task};

use super::{Ctx, Load, Page, PageMsg, Route, Section, View, fetch};
use crate::actions::{Collection, RowAction};
use crate::app::{App, Message as AppMessage};
use crate::failure::Failure;
use crate::icons::{Icon, Tone, icon};
use crate::{theme, widgets};

/// A Jellyfin playlist as a row: id, name, how many songs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub id: String,
    pub name: String,
    pub count: Option<i32>,
}

impl Summary {
    pub fn from_item(item: &Item) -> Self {
        Self {
            id: item.id.clone(),
            name: item.name.clone(),
            count: item.child_count,
        }
    }
}

/// Every playlist this user can see, by name. Jellyfin keeps playlists in
/// their own folder, so no library parent.
pub fn query() -> ListQuery {
    ListQuery {
        sort: Sort::Name,
        order: SortOrder::Ascending,
        ..ListQuery::default()
    }
}

/// The two kinds of playlist, side by side at the top of the page.
/// The title and the two tabs, laid out alike on both pages, so the tabs stay
/// where they are when switching between them.
pub fn header<'a>(jellyfin: bool, can_go_back: bool) -> Element<'a, AppMessage> {
    column![
        widgets::title_bar("Playlists", can_go_back),
        switcher(jellyfin)
    ]
    .spacing(20)
    .into()
}

fn switcher<'a>(jellyfin: bool) -> Element<'a, AppMessage> {
    let tab = |label: &'a str, selected: bool, section: Section| {
        button(text(label).size(14))
            .style(if selected {
                theme::filled
            } else {
                theme::tonal
            })
            .padding([6, 14])
            .on_press(AppMessage::Section(section))
    };
    row![
        tab("Aoide playlists", !jellyfin, Section::Playlists),
        tab("Jellyfin playlists", jellyfin, Section::JellyfinPlaylists),
    ]
    .spacing(8)
    .into()
}

#[derive(Debug)]
pub struct State {
    pub playlists: Load<Vec<Summary>>,
    new_name: String,
    creating: bool,
    error: Option<Failure>,
}

#[derive(Debug, Clone)]
pub enum Message {
    Loaded(Result<Vec<Item>, Failure>),
    Reload,
    NewName(String),
    Create,
    Created(Result<String, Failure>),
}

pub fn open(ctx: &Ctx<'_>) -> (State, Task<AppMessage>) {
    (
        State {
            playlists: Load::Loading,
            new_name: String::new(),
            creating: false,
            error: None,
        },
        load(ctx),
    )
}

fn load(ctx: &Ctx<'_>) -> Task<AppMessage> {
    let client = ctx.client.clone();
    fetch(
        ctx.key,
        async move { client.playlists(&query()).await.map(|page| page.items) },
        |r| PageMsg::JellyfinPlaylists(Message::Loaded(r)),
    )
}

impl State {
    pub fn update(&mut self, message: Message, ctx: &Ctx<'_>) -> Task<AppMessage> {
        match message {
            Message::Loaded(result) => {
                self.playlists =
                    Load::from(result.map(|items| items.iter().map(Summary::from_item).collect()));
                Task::none()
            }
            Message::Reload => load(ctx),
            Message::NewName(name) => {
                self.new_name = name;
                Task::none()
            }
            Message::Create => {
                let name = self.new_name.trim().to_owned();
                if name.is_empty() || self.creating {
                    return Task::none();
                }
                self.creating = true;
                let client = ctx.client.clone();
                fetch(
                    ctx.key,
                    async move { client.create_playlist::<&str>(&name, &[]).await },
                    |r| PageMsg::JellyfinPlaylists(Message::Created(r)),
                )
            }
            Message::Created(result) => {
                self.creating = false;
                match result {
                    Ok(_) => {
                        self.new_name.clear();
                        self.error = None;
                        load(ctx)
                    }
                    Err(error) => {
                        self.error = Some(error.after("The playlist couldn't be made."));
                        Task::none()
                    }
                }
            }
        }
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, AppMessage> {
        let key = view.key;
        let message = move |m: Message| AppMessage::Page(key, PageMsg::JellyfinPlaylists(m));
        let ready = !self.new_name.trim().is_empty() && !self.creating;
        let create = row![
            text_input("New Jellyfin playlist name", &self.new_name)
                .on_input(move |s| message(Message::NewName(s)))
                .on_submit_maybe(ready.then(|| message(Message::Create)))
                .padding(8)
                .style(theme::field),
            button(text(if self.creating {
                "Making…"
            } else {
                "New playlist"
            }))
            .style(theme::filled)
            .padding([8, 14])
            .on_press_maybe(ready.then(|| message(Message::Create))),
        ]
        .spacing(10)
        .align_y(Center);
        let mut body = column![
            header(true, view.can_go_back),
            widgets::secondary(
                "Playlists kept by your Jellyfin server, which every Jellyfin app sees. \
                 Aoide playlists sync between Aoide apps and can be smart."
            )
            .size(13),
            create,
        ]
        .spacing(16);
        if let Some(error) = &self.error {
            body = body.push(view.problem(error));
        }
        let list: Element<'a, AppMessage> = match &self.playlists {
            Load::Loading => widgets::loading(),
            Load::Failed(error) => view.failure(error),
            Load::Ready(lists) if lists.is_empty() => {
                widgets::empty("This server has no playlists you can see yet.")
            }
            Load::Ready(lists) => {
                let mut rows = column![].spacing(2);
                for playlist in lists {
                    let count = match playlist.count {
                        Some(1) => "1 song".to_owned(),
                        Some(n) => format!("{n} songs"),
                        None => String::new(),
                    };
                    let more = button(icon(Icon::More, 18.0, Tone::Secondary))
                        .style(theme::row)
                        .padding(6)
                        .on_press(AppMessage::Row(RowAction::OpenCollection {
                            anchor: format!("jellyfin-playlist:{}", playlist.id),
                            collection: Collection::JellyfinPlaylist(playlist.id.clone()),
                            title: playlist.name.clone(),
                        }));
                    rows = rows.push(
                        button(
                            row![
                                icon(Icon::Playlist, 22.0, Tone::Accent),
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
                        .on_press(AppMessage::Open(
                            Route::JellyfinPlaylist {
                                id: playlist.id.clone(),
                                name: playlist.name.clone(),
                            },
                        )),
                    );
                }
                rows.into()
            }
        };
        widgets::page(body.push(list))
    }
}

impl App {
    /// Something changed a Jellyfin playlist: every open list and that
    /// playlist's open pages read again.
    pub(crate) fn jellyfin_playlists_changed(&mut self, id: Option<String>) -> Task<AppMessage> {
        let Some(nav) = self.session.as_mut().and_then(|s| s.nav.as_mut()) else {
            return Task::none();
        };
        let mut tasks = Vec::new();
        for entry in nav.iter_mut() {
            match &entry.page {
                Page::JellyfinPlaylists(_) => tasks.push(Task::done(AppMessage::Page(
                    entry.key,
                    PageMsg::JellyfinPlaylists(Message::Reload),
                ))),
                Page::JellyfinPlaylist(page) if id.as_deref() == Some(page.id.as_str()) => {
                    tasks.push(Task::done(AppMessage::Page(
                        entry.key,
                        PageMsg::JellyfinPlaylist(super::jellyfin_playlist::Message::Reload),
                    )));
                }
                _ => {}
            }
        }
        Task::batch(tasks)
    }

    /// Add to Playlist opened: fetch the Jellyfin playlists to offer beside
    /// the Aoide ones, once per menu.
    pub(crate) fn offer_jellyfin_playlists(&mut self) -> Task<AppMessage> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let Some(menu) = signed
            .menu
            .as_mut()
            .filter(|m| m.adding && m.jellyfin.is_none())
        else {
            return Task::none();
        };
        menu.jellyfin = Some(Ok(Vec::new()));
        let client = signed.connection.client.clone();
        Task::perform(
            async move { client.playlists(&query()).await },
            |result| match result {
                Err(aoide_jellyfin::Error::Unauthorized) => AppMessage::TokenRejected,
                result => AppMessage::Row(RowAction::JellyfinPlaylists(
                    result
                        .map(|page| page.items.into_iter().map(|i| (i.id, i.name)).collect())
                        .map_err(|e| crate::failure::describe(&e).to_string()),
                )),
            },
        )
    }

    /// The menu's Jellyfin playlist actions.
    pub(crate) fn jellyfin_menu_action(&mut self, action: RowAction) -> Task<AppMessage> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        match action {
            RowAction::JellyfinPlaylists(result) => {
                if let Some(menu) = signed.menu.as_mut() {
                    menu.jellyfin = Some(result);
                }
                Task::none()
            }
            RowAction::AddToJellyfin(playlist) => {
                let Some(menu) = signed.menu.take() else {
                    return Task::none();
                };
                let ids: Vec<String> = menu.tracks.iter().map(|t| t.id.clone()).collect();
                let client = signed.connection.client.clone();
                let changed = playlist.clone();
                Task::perform(
                    async move { client.add_to_playlist(&playlist, &ids, None).await },
                    move |result| match result {
                        Ok(()) => AppMessage::JellyfinPlaylistChanged(Some(changed.clone())),
                        Err(aoide_jellyfin::Error::Unauthorized) => AppMessage::TokenRejected,
                        Err(error) => AppMessage::Notice(format!(
                            "The songs couldn't be added. {}",
                            crate::failure::describe(&error)
                        )),
                    },
                )
            }
            _ => Task::none(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_says_how_many_songs_it_has() {
        let item: Item = serde_json::from_value(serde_json::json!({
            "Id": "jp1", "Name": "Road Trip", "Type": "Playlist", "ChildCount": 3
        }))
        .unwrap();
        assert_eq!(
            Summary::from_item(&item),
            Summary {
                id: "jp1".into(),
                name: "Road Trip".into(),
                count: Some(3)
            }
        );
        assert_eq!(
            query().parent_id,
            None,
            "playlists live outside the library"
        );
    }
}
