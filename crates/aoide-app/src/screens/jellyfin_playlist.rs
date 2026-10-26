//! One Jellyfin playlist: its songs, Play and Shuffle, rename, delete (after
//! asking), and while editing each entry moved up, down or out. Its cover can
//! be chosen from a file or removed; Jellyfin lets only administrators change
//! images, and anyone else is told so in those words.

use aoide_jellyfin::{Item, PlaylistEdit, PlaylistEntry};
use iced::Task;

use super::{Ctx, Load, PageMsg, Section, fetch};
use crate::app::Message as App;
use crate::artwork::ArtKey;
use crate::failure::Failure;
use crate::playback::tracks::TrackMeta;
use crate::widgets;

mod view;

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
        other => crate::failure::describe(other).to_string(),
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
    error: Option<Failure>,
    /// What the last cover change said.
    cover_note: Option<String>,
    busy: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    Item(Result<Vec<Item>, Failure>),
    Entries(Result<Vec<PlaylistEntry>, Failure>),
    Reload,
    Rename,
    RenameText(String),
    SaveName,
    Renamed(Result<String, Failure>),
    Delete,
    ConfirmDelete(bool),
    Deleted(Result<(), Failure>),
    Edit(bool),
    Move {
        entry: String,
        up: bool,
    },
    Remove(String),
    Edited(Result<(), Failure>),
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
                self.entries_found(result);
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
            Message::SaveName => self.save_name(ctx),
            Message::Renamed(result) => self.renamed(result),
            Message::Delete => {
                self.confirm_delete = true;
                Task::none()
            }
            Message::ConfirmDelete(false) => {
                self.confirm_delete = false;
                Task::none()
            }
            Message::ConfirmDelete(true) => self.delete(ctx),
            Message::Deleted(Ok(())) => Task::done(App::JellyfinPlaylistChanged(None))
                .chain(Task::done(App::Section(Section::JellyfinPlaylists))),
            Message::Deleted(Err(error)) => {
                self.error = Some(error.after("The playlist couldn't be deleted."));
                Task::none()
            }
            Message::Edit(on) => {
                self.editing = on;
                Task::none()
            }
            Message::Move { entry, up } => self.move_entry(entry, up, ctx),
            Message::Remove(entry) => self.remove_entry(entry, ctx),
            Message::Edited(Ok(())) => Task::done(App::JellyfinPlaylistChanged(None)),
            Message::Edited(Err(error)) => {
                self.error = Some(error);
                self.load_entries(ctx)
            }
            Message::ChooseCover => {
                self.cover_note = None;
                let key = ctx.key;
                Task::perform(pick_picture("Choose a cover"), move |file| {
                    App::Page(key, PageMsg::JellyfinPlaylist(Message::CoverFile(file)))
                })
            }
            Message::CoverFile(None) => Task::none(),
            Message::CoverFile(Some(read)) => self.cover_file(read, ctx),
            Message::RemoveCover => self.remove_cover(ctx),
            Message::CoverDone(result) => self.cover_done(result, ctx),
        }
    }

    fn entries_found(&mut self, result: Result<Vec<PlaylistEntry>, Failure>) {
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
    }

    fn save_name(&mut self, ctx: &Ctx<'_>) -> Task<App> {
        let Some(name) = self.renaming.take().map(|n| n.trim().to_owned()) else {
            return Task::none();
        };
        if name.is_empty() || name == self.name {
            return Task::none();
        }
        let client = ctx.client.clone();
        let id = self.id.clone();
        fetch(
            ctx.key,
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

    fn renamed(&mut self, result: Result<String, Failure>) -> Task<App> {
        match result {
            Ok(name) => {
                self.name = name;
                self.error = None;
                Task::done(App::JellyfinPlaylistChanged(None))
            }
            Err(error) => {
                self.error = Some(error.after("The playlist couldn't be renamed."));
                Task::none()
            }
        }
    }

    fn delete(&mut self, ctx: &Ctx<'_>) -> Task<App> {
        self.confirm_delete = false;
        let client = ctx.client.clone();
        let id = self.id.clone();
        fetch(
            ctx.key,
            async move { client.delete_playlist(&id).await },
            |r| PageMsg::JellyfinPlaylist(Message::Deleted(r)),
        )
    }

    fn move_entry(&mut self, entry: String, up: bool, ctx: &Ctx<'_>) -> Task<App> {
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
        let client = ctx.client.clone();
        let id = self.id.clone();
        fetch(
            ctx.key,
            async move { client.move_playlist_entry(&id, &entry, to as u32).await },
            |r| PageMsg::JellyfinPlaylist(Message::Edited(r)),
        )
    }

    fn remove_entry(&mut self, entry: String, ctx: &Ctx<'_>) -> Task<App> {
        let Some(at) = self.entries.iter().position(|e| *e == entry) else {
            return Task::none();
        };
        self.entries.remove(at);
        if let Load::Ready(tracks) = &mut self.tracks {
            tracks.remove(at);
        }
        let client = ctx.client.clone();
        let id = self.id.clone();
        fetch(
            ctx.key,
            async move { client.remove_from_playlist(&id, &[entry]).await },
            |r| PageMsg::JellyfinPlaylist(Message::Edited(r)),
        )
    }

    /// The picked file, read: uploaded as the cover if it is a picture.
    fn cover_file(&mut self, read: Result<Vec<u8>, String>, ctx: &Ctx<'_>) -> Task<App> {
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
        let client = ctx.client.clone();
        let id = self.id.clone();
        let key = ctx.key;
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

    fn remove_cover(&mut self, ctx: &Ctx<'_>) -> Task<App> {
        self.busy = true;
        self.cover_note = None;
        let client = ctx.client.clone();
        let id = self.id.clone();
        let key = ctx.key;
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

    fn cover_done(&mut self, result: Result<&'static str, String>, ctx: &Ctx<'_>) -> Task<App> {
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

    pub fn artwork(&self) -> Vec<ArtKey> {
        let mut keys: Vec<ArtKey> = self.art.clone().into_iter().collect();
        if let Load::Ready(tracks) = &self.tracks {
            keys.extend(super::track_art(tracks));
        }
        keys
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
        let said = admin_error(&other, "x");
        assert!(said.starts_with("Can't reach your server"), "{said}");
        assert!(
            !said.contains("refused"),
            "the transport's text is for Details: {said}"
        );
    }
}
