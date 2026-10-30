//! Aoide playlists' covers: which picture to draw (`Store::playlist_covers`),
//! the bytes of a chosen one fetched only when it is about to be drawn
//! (`aoide_sidecar::cover_bytes`, from this computer's blob directory first),
//! choosing one from a file, and taking it off.
//!
//! A chosen picture is kept in the blob directory in the data directory, not
//! the cache: until the next sync uploads it, it is the only copy. The sync
//! engine reads the same directory to upload it before the change naming it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use aoide_sidecar::{SidecarClient, cover_bytes};
use aoide_store::{BlobDirectory, PlaylistCover, Store};
use iced::Task;

use crate::app::{App, Message};
use crate::artwork::ArtKey;
use crate::config::Paths;
use crate::screens::{Page, PageMsg};
use crate::store_worker::StoreHandle;

/// Where chosen covers are kept: beside the store.
pub fn blobs_dir(paths: &Paths) -> PathBuf {
    paths.store_file.with_file_name("covers")
}

/// The blob directory, opened (and made) if there is somewhere to put it.
pub fn open_blobs(paths: Option<&Paths>) -> Option<BlobDirectory> {
    BlobDirectory::open(blobs_dir(paths?)).ok()
}

/// A cover's bytes are no album's: they are drawn under this key, which
/// nothing ever asks the server for.
pub fn blob_key(hash: &str) -> ArtKey {
    ArtKey::new("aoidecover", hash)
}

/// What one resolution found: each playlist's key, and the bytes of the
/// chosen pictures to put in the artwork cache.
#[derive(Debug, Clone, Default)]
pub struct Resolved {
    pub art: HashMap<String, ArtKey>,
    pub bytes: Vec<(ArtKey, Vec<u8>)>,
}

/// The album cover each first-track cover stands for, by playlist. A cover
/// naming a Jellyfin item (from before Melete) has nothing to draw: Melete's
/// import clears those, and an app uploads a picture again.
pub fn album_covers(covers: &HashMap<String, PlaylistCover>) -> Vec<(String, ArtKey)> {
    let mut wanted: Vec<(String, ArtKey)> = covers
        .iter()
        .filter_map(|(playlist, cover)| match cover {
            PlaylistCover::FirstTrack(album) => Some((
                playlist.clone(),
                ArtKey::new(album, aoide_melete::COVER_TAG),
            )),
            _ => None,
        })
        .collect();
    wanted.sort_by(|a, b| a.0.cmp(&b.0));
    wanted
}

/// The covers of these playlists, the way they are drawn: chosen pictures as
/// bytes (fetched from the server only if this computer lacks them), first
/// tracks by their album's cover.
pub async fn resolve(
    store: StoreHandle,
    shared: Arc<Store>,
    sidecar: Option<SidecarClient>,
    blobs: Option<BlobDirectory>,
    ids: Vec<String>,
) -> Resolved {
    let covers: HashMap<String, PlaylistCover> = store
        .call(move |w| {
            let lists: Vec<_> = ids
                .iter()
                .filter_map(|id| w.store.playlist(id).ok().flatten())
                .collect();
            w.store.playlist_covers(&lists).unwrap_or_default()
        })
        .await
        .unwrap_or_default();
    let mut resolved = Resolved::default();
    for (playlist, cover) in &covers {
        let PlaylistCover::Image { hash, mime } = cover else {
            continue;
        };
        let Some(blobs) = &blobs else {
            continue;
        };
        if let Ok(Some(bytes)) =
            cover_bytes(&shared, blobs, sidecar.as_ref(), hash, mime.as_deref()).await
        {
            let key = blob_key(hash);
            resolved.art.insert(playlist.clone(), key.clone());
            resolved.bytes.push((key, bytes));
        }
    }
    for (playlist, key) in album_covers(&covers) {
        resolved.art.entry(playlist).or_insert(key);
    }
    resolved
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// The page with this key shows these playlists: find their covers.
    Resolve {
        key: u64,
        ids: Vec<String>,
    },
    Resolved {
        key: u64,
        found: Box<Resolved>,
    },
    /// Choose a picture for this playlist from a file.
    Choose {
        key: u64,
        playlist: String,
    },
    Picked {
        key: u64,
        playlist: String,
        file: Option<Result<Vec<u8>, String>>,
    },
    Remove {
        key: u64,
        playlist: String,
    },
    /// A cover changed (or failed to): `Err` is what to tell the listener.
    Changed {
        key: u64,
        result: Result<String, String>,
    },
}

impl App {
    pub(crate) fn covers(&mut self, message: Msg) -> Task<Message> {
        if self.session.is_none() {
            return Task::none();
        }
        match message {
            Msg::Resolve { key, ids } => self.resolve_covers(key, ids),
            Msg::Resolved { key, found } => self.covers_resolved(key, *found),
            Msg::Choose { key, playlist } => {
                Task::perform(pick_picture("Choose a cover"), move |file| {
                    Message::Covers(Msg::Picked {
                        key,
                        playlist: playlist.clone(),
                        file,
                    })
                })
            }
            Msg::Picked { file: None, .. } => Task::none(),
            Msg::Picked {
                key,
                playlist,
                file: Some(read),
            } => self.cover_picked(key, playlist, read),
            Msg::Remove { key, playlist } => self.take_cover_off(key, playlist),
            Msg::Changed { key, result } => Task::batch([
                Task::done(Message::Page(
                    key,
                    PageMsg::Playlist(crate::screens::playlist::Message::CoverNote(result)),
                )),
                Task::done(Message::PlaylistsChanged),
                self.refresh_covers(),
            ]),
        }
    }

    /// Finds the covers of these playlists (none: every playlist, for the
    /// list page) for the page with `key`.
    fn resolve_covers(&mut self, key: u64, ids: Vec<String>) -> Task<Message> {
        let store = self.store.clone();
        let blobs = open_blobs(self.paths.as_ref());
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        // No ids: every playlist, for the list page.
        let ids = if ids.is_empty() {
            signed.playlists.iter().map(|p| p.id.clone()).collect()
        } else {
            ids
        };
        let sidecar = signed.sidecar.clone();
        let shared = store.shared();
        Task::perform(resolve(store, shared, sidecar, blobs, ids), move |found| {
            Message::Covers(Msg::Resolved {
                key,
                found: Box::new(found),
            })
        })
    }

    /// The covers found: the chosen pictures into the artwork cache, and
    /// the keys to the page that asked.
    fn covers_resolved(&mut self, key: u64, found: Resolved) -> Task<Message> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let Resolved { art, bytes } = found;
        for (art_key, bytes) in bytes {
            self.artwork.insert(art_key, bytes);
        }
        let page = signed
            .nav
            .as_mut()
            .and_then(|nav| nav.iter_mut().find(|e| e.key == key))
            .map(|e| &e.page);
        let message = match page {
            Some(Page::Playlist(state)) => PageMsg::Playlist(
                crate::screens::playlist::Message::Cover(art.get(state.id()).cloned()),
            ),
            Some(Page::Playlists(_)) => {
                PageMsg::Playlists(crate::screens::playlists::Message::Covers(art))
            }
            _ => return Task::none(),
        };
        Task::done(Message::Page(key, message))
    }

    /// A picture picked for a playlist: kept in the blob directory and set
    /// as its cover.
    fn cover_picked(
        &mut self,
        key: u64,
        playlist: String,
        read: Result<Vec<u8>, String>,
    ) -> Task<Message> {
        let store = self.store.clone();
        let blobs = open_blobs(self.paths.as_ref());
        let bytes = match read {
            Ok(bytes) => bytes,
            Err(error) => {
                return Task::done(Message::Covers(Msg::Changed {
                    key,
                    result: Err(format!("That file couldn't be read: {error}")),
                }));
            }
        };
        let Some(blobs) = blobs else {
            return Task::done(Message::Covers(Msg::Changed {
                key,
                result: Err("There is nowhere on this computer to keep a cover.".into()),
            }));
        };
        let now = aoide_store::now_millis();
        Task::perform(
            async move {
                store
                    .call(move |w| {
                        w.store
                            .set_cover_image(&playlist, &bytes, &blobs, now)
                            .map(|_| {
                                "The cover will go to your other devices with the next sync."
                                    .to_owned()
                            })
                            .map_err(|e| format!("That picture can't be a cover: {e}."))
                    })
                    .await
                    .unwrap_or_else(|| Err("the library is closing".into()))
            },
            move |result| Message::Covers(Msg::Changed { key, result }),
        )
    }

    fn take_cover_off(&mut self, key: u64, playlist: String) -> Task<Message> {
        let store = self.store.clone();
        let now = aoide_store::now_millis();
        Task::perform(
            async move {
                store
                    .call(move |w| {
                        w.store
                            .clear_cover_image(&playlist, now)
                            .map(|_| "The cover has been taken off.".to_owned())
                            .map_err(|e| e.to_string())
                    })
                    .await
                    .unwrap_or_else(|| Err("the library is closing".into()))
            },
            move |result| Message::Covers(Msg::Changed { key, result }),
        )
    }

    /// Every open page that draws playlist covers, asked to find them again.
    fn refresh_covers(&mut self) -> Task<Message> {
        let Some(nav) = self.session.as_mut().and_then(|s| s.nav.as_mut()) else {
            return Task::none();
        };
        let mut tasks = Vec::new();
        for entry in nav.iter_mut() {
            let message = match &entry.page {
                Page::Playlist(_) => {
                    PageMsg::Playlist(crate::screens::playlist::Message::FindCover)
                }
                Page::Playlists(_) => {
                    PageMsg::Playlists(crate::screens::playlists::Message::FindCovers)
                }
                _ => continue,
            };
            tasks.push(Task::done(Message::Page(entry.key, message)));
        }
        Task::batch(tasks)
    }
}

/// A picture chosen in the system's file dialog, read whole; `None` when the
/// dialog was closed.
async fn pick_picture(title: &str) -> Option<Result<Vec<u8>, String>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_track_covers_are_their_albums_and_jellyfin_items_draw_nothing() {
        let covers = HashMap::from([
            ("p1".to_owned(), PlaylistCover::JellyfinItem("jp1".into())),
            ("p2".to_owned(), PlaylistCover::FirstTrack("al1".into())),
            (
                "p3".to_owned(),
                PlaylistCover::Image {
                    hash: "a".repeat(64),
                    mime: None,
                },
            ),
            ("p4".to_owned(), PlaylistCover::None),
        ]);
        assert_eq!(
            album_covers(&covers),
            [("p2".to_owned(), ArtKey::new("al1", aoide_melete::COVER_TAG))]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_chosen_picture_is_drawn_from_this_computer() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobDirectory::open(dir.path().join("covers")).unwrap();
        let store = StoreHandle::in_memory("desk").unwrap();
        let png = b"\x89PNG\r\n\x1a\nnot really".to_vec();
        let (id, hash) = {
            let blobs = blobs.clone();
            let png = png.clone();
            store
                .call(move |w| {
                    let list = w.store.create_playlist("Mine", None, None, 1).unwrap();
                    let list = w.store.set_cover_image(&list.id, &png, &blobs, 2).unwrap();
                    (list.id, list.image_hash.unwrap())
                })
                .await
                .unwrap()
        };
        let shared = store.shared();
        let found = resolve(store, shared, None, Some(blobs), vec![id.clone()]).await;
        assert_eq!(found.art.get(&id), Some(&blob_key(&hash)));
        assert_eq!(found.bytes, [(blob_key(&hash), png)]);
    }
}
