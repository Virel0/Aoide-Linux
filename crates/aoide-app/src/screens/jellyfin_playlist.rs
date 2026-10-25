//! One Jellyfin playlist: its songs, Play and Shuffle, rename, delete (after
//! asking), and while editing each entry moved up, down or out. Its cover can
//! be chosen from a file or removed; Jellyfin lets only administrators change
//! images, and anyone else is told so in those words.

use aoide_jellyfin::{Item, PlaylistEdit, PlaylistEntry};
use aoide_store::PlaySource;
use iced::widget::{Column, button, column, row, text, text_input};
use iced::{Center, Element, Length, Task};

use super::{Ctx, Load, PageMsg, Section, View, fetch};
use crate::actions::PlayRequest;
use crate::app::Message as App;
use crate::artwork::ArtKey;
use crate::icons::{Icon, Tone, icon};
use crate::playback::tracks::TrackMeta;
use crate::{theme, widgets};

/// The largest file offered as a cover.
pub const MAX_COVER_BYTES: usize = 20 * 1024 * 1024;

const LIST: widgets::ListStyle = widgets::ListStyle {
    numbered: false,
    show_album: true,
};

/// What the server says when an image or a scan needs an administrator, as a
/// person should read it; anything else as it is.
pub fn admin_error(error: &aoide_jellyfin::Error, what: &str) -> String {
    match error {
        aoide_jellyfin::Error::Http { status: 403, .. } => format!(
            "Only an administrator of this Jellyfin server can {what}. The server refused \
             (HTTP 403 Forbidden)."
        ),
        other => crate::session::describe(other),
    }
}

#[derive(Debug)]
pub struct State {
    pub id: String,
    name: String,
    item: Option<Item>,
    art: Option<ArtKey>,
    /// The songs and, beside them, the entry each one is.
    pub tracks: Load<Vec<TrackMeta>>,
    entries: Vec<String>,
    renaming: Option<String>,
    confirm_delete: bool,
    editing: bool,
    error: Option<String>,
    /// What the last cover change said.
    cover_note: Option<String>,
    busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Item(Result<Vec<Item>, String>),
    Entries(Result<Vec<PlaylistEntry>, String>),
    Reload,
    Rename,
    RenameText(String),
    SaveName,
    Renamed(Result<String, String>),
    Delete,
    ConfirmDelete(bool),
    Deleted(Result<(), String>),
    Edit(bool),
    Move {
        entry: String,
        up: bool,
    },
    Remove(String),
    Edited(Result<(), String>),
    ChooseCover,
    /// The file picked, read: its bytes, or why not.
    CoverFile(Option<Result<Vec<u8>, String>>),
    RemoveCover,
    CoverDone(Result<&'static str, String>),
}

pub fn open(ctx: &Ctx<'_>, id: &str, name: &str) -> (State, Task<App>) {
    let state = State {
        id: id.to_owned(),
        name: name.to_owned(),
        item: None,
        art: None,
        tracks: Load::Loading,
        entries: Vec::new(),
        renaming: None,
        confirm_delete: false,
        editing: false,
        error: None,
        cover_note: None,
        busy: false,
    };
    let task = Task::batch([state.load_item(ctx), state.load_entries(ctx)]);
    (state, task)
}

/// Where the entry at `at` lands one step up or down, if it can move.
pub fn step(at: usize, up: bool, len: usize) -> Option<usize> {
    let to = if up { at.checked_sub(1)? } else { at + 1 };
    (to < len).then_some(to)
}

/// Reads a picked file as a cover: a picture, not too large.
pub fn cover_bytes(bytes: Vec<u8>) -> Result<(Vec<u8>, &'static str), String> {
    if bytes.len() > MAX_COVER_BYTES {
        return Err(format!(
            "That picture is {} MB; a cover can be at most {} MB.",
            bytes.len() / (1024 * 1024),
            MAX_COVER_BYTES / (1024 * 1024)
        ));
    }
    let mime = aoide_store::sniff_image_mime(&bytes)
        .ok_or_else(|| "That file isn't a JPEG, PNG or WebP picture.".to_owned())?;
    Ok((bytes, mime))
}

/// Asks for a picture with the system's file dialog and reads it. `None`
/// when the dialog was cancelled.
pub async fn pick_picture(title: &str) -> Option<Result<Vec<u8>, String>> {
    let file = rfd::AsyncFileDialog::new()
        .set_title(title)
        .add_filter("Pictures", &["jpg", "jpeg", "png", "webp"])
        .pick_file()
        .await?;
    let path = file.path().to_owned();
    Some(
        tokio::task::spawn_blocking(move || std::fs::read(&path))
            .await
            .map_err(|e| e.to_string())
            .and_then(|read| read.map_err(|e| e.to_string())),
    )
}

impl State {
    fn load_item(&self, ctx: &Ctx<'_>) -> Task<App> {
        let client = ctx.client.clone();
        let id = self.id.clone();
        fetch(
            ctx.key,
            async move { client.items_by_ids(&[id]).await },
            |r| PageMsg::JellyfinPlaylist(Message::Item(r)),
        )
    }

    fn load_entries(&self, ctx: &Ctx<'_>) -> Task<App> {
        let client = ctx.client.clone();
        let id = self.id.clone();
        fetch(
            ctx.key,
            async move { client.playlist_entries(&id).await },
            |r| PageMsg::JellyfinPlaylist(Message::Entries(r)),
        )
    }

    pub fn update(&mut self, message: Message, ctx: &Ctx<'_>) -> Task<App> {
        let client = ctx.client.clone();
        let id = self.id.clone();
        let key = ctx.key;
        match message {
            Message::Item(result) => {
                if let Some(item) = result.ok().and_then(|items| items.into_iter().next()) {
                    self.name = item.name.clone();
                    self.art = item.artwork_source().map(|(i, t)| ArtKey::new(i, t));
                    self.item = Some(item);
                }
                Task::none()
            }
            Message::Entries(result) => {
                match result {
                    Ok(entries) => {
                        let (ids, tracks) = entries
                            .iter()
                            .map(|e| (e.entry_id.clone(), TrackMeta::from_item(&e.item)))
                            .unzip();
                        self.entries = ids;
                        self.tracks = Load::Ready(tracks);
                    }
                    Err(error) => self.tracks = Load::Failed(error),
                }
                Task::none()
            }
            Message::Reload => Task::batch([self.load_item(ctx), self.load_entries(ctx)]),
            Message::Rename => {
                self.renaming = Some(self.name.clone());
                Task::none()
            }
            Message::RenameText(name) => {
                self.renaming = Some(name);
                Task::none()
            }
            Message::SaveName => {
                let Some(name) = self.renaming.take().map(|n| n.trim().to_owned()) else {
                    return Task::none();
                };
                if name.is_empty() || name == self.name {
                    return Task::none();
                }
                fetch(
                    key,
                    async move {
                        let edit = PlaylistEdit {
                            name: Some(name.clone()),
                            ..PlaylistEdit::default()
                        };
                        client.edit_playlist(&id, &edit).await.map(|()| name)
                    },
                    |r| PageMsg::JellyfinPlaylist(Message::Renamed(r)),
                )
            }
            Message::Renamed(result) => match result {
                Ok(name) => {
                    self.name = name;
                    self.error = None;
                    Task::done(App::JellyfinPlaylistChanged(None))
                }
                Err(error) => {
                    self.error = Some(format!("The playlist couldn't be renamed: {error}"));
                    Task::none()
                }
            },
            Message::Delete => {
                self.confirm_delete = true;
                Task::none()
            }
            Message::ConfirmDelete(false) => {
                self.confirm_delete = false;
                Task::none()
            }
            Message::ConfirmDelete(true) => {
                self.confirm_delete = false;
                fetch(key, async move { client.delete_playlist(&id).await }, |r| {
                    PageMsg::JellyfinPlaylist(Message::Deleted(r))
                })
            }
            Message::Deleted(Ok(())) => Task::done(App::JellyfinPlaylistChanged(None))
                .chain(Task::done(App::Section(Section::JellyfinPlaylists))),
            Message::Deleted(Err(error)) => {
                self.error = Some(format!("The playlist couldn't be deleted: {error}"));
                Task::none()
            }
            Message::Edit(on) => {
                self.editing = on;
                Task::none()
            }
            Message::Move { entry, up } => {
                let Some(at) = self.entries.iter().position(|e| *e == entry) else {
                    return Task::none();
                };
                let Some(to) = step(at, up, self.entries.len()) else {
                    return Task::none();
                };
                // Shown at once; the server is told, and read again if it says no.
                self.entries.swap(at, to);
                if let Load::Ready(tracks) = &mut self.tracks {
                    tracks.swap(at, to);
                }
                fetch(
                    key,
                    async move { client.move_playlist_entry(&id, &entry, to as u32).await },
                    |r| PageMsg::JellyfinPlaylist(Message::Edited(r)),
                )
            }
            Message::Remove(entry) => {
                let Some(at) = self.entries.iter().position(|e| *e == entry) else {
                    return Task::none();
                };
                self.entries.remove(at);
                if let Load::Ready(tracks) = &mut self.tracks {
                    tracks.remove(at);
                }
                fetch(
                    key,
                    async move { client.remove_from_playlist(&id, &[entry]).await },
                    |r| PageMsg::JellyfinPlaylist(Message::Edited(r)),
                )
            }
            Message::Edited(Ok(())) => Task::done(App::JellyfinPlaylistChanged(None)),
            Message::Edited(Err(error)) => {
                self.error = Some(error);
                self.load_entries(ctx)
            }
            Message::ChooseCover => {
                self.cover_note = None;
                Task::perform(pick_picture("Choose a cover"), move |file| {
                    App::Page(key, PageMsg::JellyfinPlaylist(Message::CoverFile(file)))
                })
            }
            Message::CoverFile(None) => Task::none(),
            Message::CoverFile(Some(read)) => {
                let picture = read
                    .map_err(|e| format!("That file couldn't be read: {e}"))
                    .and_then(cover_bytes);
                let (bytes, mime) = match picture {
                    Ok(picture) => picture,
                    Err(error) => {
                        self.cover_note = Some(error);
                        return Task::none();
                    }
                };
                self.busy = true;
                Task::perform(
                    async move { client.set_playlist_cover(&id, &bytes, mime).await },
                    move |result| match result {
                        Err(aoide_jellyfin::Error::Unauthorized) => App::TokenRejected,
                        result => App::Page(
                            key,
                            PageMsg::JellyfinPlaylist(Message::CoverDone(
                                result
                                    .map(|()| "The cover has been changed.")
                                    .map_err(|e| admin_error(&e, "change a playlist's cover")),
                            )),
                        ),
                    },
                )
            }
            Message::RemoveCover => {
                self.busy = true;
                self.cover_note = None;
                Task::perform(
                    async move { client.delete_playlist_cover(&id).await },
                    move |result| match result {
                        Err(aoide_jellyfin::Error::Unauthorized) => App::TokenRejected,
                        result => App::Page(
                            key,
                            PageMsg::JellyfinPlaylist(Message::CoverDone(
                                result
                                    .map(|()| "The cover has been removed.")
                                    .map_err(|e| admin_error(&e, "change a playlist's cover")),
                            )),
                        ),
                    },
                )
            }
            Message::CoverDone(result) => {
                self.busy = false;
                match result {
                    Ok(note) => {
                        self.cover_note = Some(note.to_owned());
                        // The new picture has a new tag.
                        self.load_item(ctx)
                    }
                    Err(error) => {
                        self.cover_note = Some(error);
                        Task::none()
                    }
                }
            }
        }
    }

    pub fn artwork(&self) -> Vec<ArtKey> {
        let mut keys: Vec<ArtKey> = self.art.clone().into_iter().collect();
        if let Load::Ready(tracks) = &self.tracks {
            keys.extend(super::track_art(tracks));
        }
        keys
    }

    /// What playing from this page is remembered as.
    fn context(&self) -> aoide_store::RecentContext {
        aoide_store::RecentContext::new(
            aoide_store::RecentContextKind::JellyfinPlaylist,
            self.id.clone(),
            self.name.clone(),
            aoide_store::now_millis(),
        )
    }

    pub fn view<'a>(&'a self, view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let message = move |m: Message| App::Page(key, PageMsg::JellyfinPlaylist(m));
        let tracks: &[TrackMeta] = self.tracks.ready().map(Vec::as_slice).unwrap_or(&[]);
        let title: Element<'a, App> = match &self.renaming {
            Some(draft) => row![
                text_input("Playlist name", draft)
                    .on_input(move |s| message(Message::RenameText(s)))
                    .on_submit(message(Message::SaveName))
                    .padding(8)
                    .size(20)
                    .style(theme::field),
                button("Save")
                    .style(theme::filled)
                    .on_press(message(Message::SaveName)),
            ]
            .spacing(10)
            .align_y(Center)
            .into(),
            None => text(self.name.as_str()).size(30).font(widgets::BOLD).into(),
        };
        let count = match tracks.len() {
            1 => "1 song".to_owned(),
            n => format!("{n} songs"),
        };
        let small = |label: &'a str, m: Option<Message>| {
            button(text(label).size(13))
                .style(theme::tonal)
                .padding([6, 12])
                .on_press_maybe(m.map(message))
        };
        let mut tools = row![].spacing(8).align_y(Center);
        if self.renaming.is_none() {
            tools = tools.push(small("Rename", Some(Message::Rename)));
        }
        tools = tools.push(if self.editing {
            small("Done", Some(Message::Edit(false)))
        } else {
            small("Edit", Some(Message::Edit(true)))
        });
        tools = tools
            .push(small(
                "Choose Cover…",
                (!self.busy).then_some(Message::ChooseCover),
            ))
            .push(small(
                "Remove Cover",
                (!self.busy && self.art.is_some()).then_some(Message::RemoveCover),
            ));
        tools = tools.push(
            button(text("Delete").size(13))
                .style(theme::destructive)
                .padding([6, 12])
                .on_press_maybe((!self.confirm_delete).then(|| message(Message::Delete))),
        );
        // On a line of its own: a question beside five buttons runs off the page.
        let confirm = self.confirm_delete.then(|| {
            row![
                text("Delete this playlist from the server, for every app?").size(13),
                button(text("Delete").size(13))
                    .style(theme::destructive)
                    .padding([6, 12])
                    .on_press(message(Message::ConfirmDelete(true))),
                small("Cancel", Some(Message::ConfirmDelete(false))),
            ]
            .spacing(8)
            .align_y(Center)
        });
        let mut info = column![
            title,
            widgets::secondary(format!("Jellyfin playlist · {count}")).size(13),
            row![
                widgets::play_buttons(
                    tracks,
                    format!("{key}:jellyfin-playlist"),
                    PlaySource::Playlist
                ),
                crate::radio::start_button(crate::radio::Station::new(
                    aoide_jellyfin::MixSeed::Playlist(self.id.clone()),
                    self.name.clone(),
                )),
            ]
            .spacing(10),
            tools,
        ]
        .spacing(10)
        .width(Length::Fill);
        if let Some(confirm) = confirm {
            info = info.push(confirm);
        }
        let mut body = column![
            widgets::title_bar("", view.can_go_back),
            row![
                widgets::art(self.art.as_ref(), view.artwork, Length::Fixed(180.0), 10.0),
                info
            ]
            .spacing(24),
        ]
        .spacing(16);
        if let Some(note) = &self.cover_note {
            body = body.push(widgets::secondary(note.as_str()).size(13));
        }
        if let Some(menu) = view
            .menu
            .filter(|m| m.anchor == format!("{key}:jellyfin-playlist"))
        {
            body = body.push(widgets::menu_strip(menu, view));
        }
        if let Some(error) = &self.error {
            body = body.push(widgets::failure(error));
        }
        body = body.push(match &self.tracks {
            Load::Loading => widgets::loading(),
            Load::Failed(error) => widgets::failure(error),
            Load::Ready(tracks) if tracks.is_empty() => {
                widgets::empty("No songs yet. Add some from any song's … menu.")
            }
            Load::Ready(tracks) if self.editing => self.editable(tracks, view),
            Load::Ready(tracks) => widgets::track_list(tracks, view, LIST, PlaySource::Playlist),
        });
        crate::handoff::played_from(widgets::page(body), Some(self.context()))
    }

    fn editable<'a>(&'a self, tracks: &'a [TrackMeta], view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let message = move |m: Message| App::Page(key, PageMsg::JellyfinPlaylist(m));
        let small = |glyph: Icon, m: Option<App>| {
            button(icon(glyph, 16.0, Tone::Secondary))
                .padding(6)
                .style(theme::row)
                .on_press_maybe(m)
        };
        let mut list = Column::with_capacity(tracks.len()).spacing(2);
        for (index, (track, entry)) in tracks.iter().zip(&self.entries).enumerate() {
            let anchor = format!("{key}:{index}");
            let play = move || App::Play(PlayRequest::from(tracks, index, PlaySource::Playlist));
            let up = (index > 0).then(|| {
                message(Message::Move {
                    entry: entry.clone(),
                    up: true,
                })
            });
            let down = (index + 1 < tracks.len()).then(|| {
                message(Message::Move {
                    entry: entry.clone(),
                    up: false,
                })
            });
            list = list.push(
                row![
                    widgets::track_row(track, &anchor, view, LIST, play),
                    small(Icon::Up, up),
                    small(Icon::Down, down),
                    small(Icon::Remove, Some(message(Message::Remove(entry.clone())))),
                ]
                .spacing(2)
                .align_y(Center),
            );
            if let Some(menu) = view.menu.filter(|m| m.anchor == anchor) {
                list = list.push(widgets::menu_strip(menu, view));
            }
        }
        list.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoide_jellyfin::{Credentials, Device, JellyfinClient, ServerAddress};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn a_step_stays_inside_the_list() {
        assert_eq!(step(0, true, 3), None);
        assert_eq!(step(0, false, 3), Some(1));
        assert_eq!(step(2, false, 3), None);
        assert_eq!(step(2, true, 3), Some(1));
    }

    #[test]
    fn only_pictures_of_a_sane_size_are_covers() {
        let png = b"\x89PNG\r\n\x1a\n rest".to_vec();
        assert_eq!(cover_bytes(png.clone()).unwrap().1, "image/png");
        assert!(
            cover_bytes(b"GIF89a".to_vec())
                .unwrap_err()
                .contains("isn't a JPEG")
        );
        let huge = [png, vec![0; MAX_COVER_BYTES]].concat();
        assert!(cover_bytes(huge).unwrap_err().contains("at most 20 MB"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_refused_cover_says_who_may_change_it() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/Items/jp1/Images/Primary"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        let client = JellyfinClient::new(
            ServerAddress::parse(&server.uri()).unwrap(),
            Device::new("Test", "device-1"),
        )
        .unwrap()
        .with_credentials(Credentials {
            user_id: "u1".into(),
            access_token: "tok".into(),
            username: "alice".into(),
        });
        let error = client
            .set_playlist_cover("jp1", b"\x89PNG\r\n\x1a\n", "image/png")
            .await
            .unwrap_err();
        let said = admin_error(&error, "change a playlist's cover");
        assert!(
            said.starts_with("Only an administrator of this Jellyfin server can change"),
            "{said}"
        );
        assert!(said.contains("403"));
        let other = aoide_jellyfin::Error::Transport("refused".into());
        assert!(admin_error(&other, "x").starts_with("Couldn't reach the server"));
    }
}
