//! Aoide playlists' covers: which picture to draw (`Store::playlist_covers`),
//! the bytes of a chosen one fetched only when it is about to be drawn
//! (`aoide_sidecar::cover_bytes`, from this computer's blob directory first),
//! choosing one from a file, taking it off, and "Repair covers".
//!
//! A chosen picture is kept in the blob directory in the data directory, not
//! the cache: until the next sync uploads it, it is the only copy. The sync
//! engine reads the same directory to upload it before the change naming it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use aoide_jellyfin::{Item, JellyfinClient};
use aoide_sidecar::{SidecarClient, cover_bytes};
use aoide_store::{BlobDirectory, NamedJellyfinPlaylist, PlaylistCover, Store};
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

/// A cover's bytes have no Jellyfin item: they are drawn under this key,
/// which nothing ever asks the server for.
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

/// The Jellyfin item each item-drawn cover names, by playlist.
pub fn items_to_fetch(covers: &HashMap<String, PlaylistCover>) -> Vec<(String, String)> {
    let mut wanted: Vec<(String, String)> = covers
        .iter()
        .filter_map(|(playlist, cover)| match cover {
            PlaylistCover::JellyfinItem(item) | PlaylistCover::FirstTrack(item) => {
                Some((playlist.clone(), item.clone()))
            }
            _ => None,
        })
        .collect();
    wanted.sort();
    wanted
}

/// The covers of these playlists, the way they are drawn: chosen pictures as
/// bytes (fetched from the sidecar only if this computer lacks them), items
/// and first tracks by their image tags.
pub async fn resolve(
    client: JellyfinClient,
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
    let wanted = items_to_fetch(&covers);
    if !wanted.is_empty() {
        let ids: Vec<&str> = wanted.iter().map(|(_, item)| item.as_str()).collect();
        let items = client.items_by_ids(&ids).await.unwrap_or_default();
        let art: HashMap<&str, ArtKey> = items
            .iter()
            .filter_map(|item: &Item| {
                let (id, tag) = item.artwork_source()?;
                Some((item.id.as_str(), ArtKey::new(id, tag)))
            })
            .collect();
        for (playlist, item) in wanted {
            if let Some(key) = art.get(item.as_str()) {
                resolved.art.entry(playlist).or_insert_with(|| key.clone());
            }
        }
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
    /// Sync panel: borrow Jellyfin playlists' pictures by name.
    Repair {
        key: u64,
    },
    /// Every page drawing covers looks again.
    RefreshAll,
}

impl App {
    pub(crate) fn covers(&mut self, message: Msg) -> Task<Message> {
        let store = self.store.clone();
        let blobs = open_blobs(self.paths.as_ref());
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        match message {
            Msg::Resolve { key, ids } => {
                // No ids: every playlist, for the list page.
                let ids = if ids.is_empty() {
                    signed.playlists.iter().map(|p| p.id.clone()).collect()
                } else {
                    ids
                };
                let client = signed.connection.client.clone();
                let sidecar = signed.sidecar.clone();
                let shared = store.shared();
                Task::perform(
                    resolve(client, store, shared, sidecar, blobs, ids),
                    move |found| {
                        Message::Covers(Msg::Resolved {
                            key,
                            found: Box::new(found),
                        })
                    },
                )
            }
            Msg::Resolved { key, found } => {
                let Resolved { art, bytes } = *found;
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
            Msg::Choose { key, playlist } => Task::perform(
                crate::screens::jellyfin_playlist::pick_picture("Choose a cover"),
                move |file| {
                    Message::Covers(Msg::Picked {
                        key,
                        playlist: playlist.clone(),
                        file,
                    })
                },
            ),
            Msg::Picked { file: None, .. } => Task::none(),
            Msg::Picked {
                key,
                playlist,
                file: Some(read),
            } => {
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
                                    .map(|_| "The cover will go to your other devices with the next sync.".to_owned())
                                    .map_err(|e| format!("That picture can't be a cover: {e}."))
                            })
                            .await
                            .unwrap_or_else(|| Err("the library is closing".into()))
                    },
                    move |result| Message::Covers(Msg::Changed { key, result }),
                )
            }
            Msg::Remove { key, playlist } => {
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
            Msg::Changed { key, result } => Task::batch([
                Task::done(Message::Page(
                    key,
                    PageMsg::Playlist(crate::screens::playlist::Message::CoverNote(result)),
                )),
                Task::done(Message::PlaylistsChanged),
                self.refresh_covers(),
            ]),
            Msg::Repair { key } => {
                let client = signed.connection.client.clone();
                let now = aoide_store::now_millis();
                Task::perform(
                    async move {
                        let lists = client
                            .playlists(&crate::screens::jellyfin_playlists::query())
                            .await
                            .map_err(|e| crate::session::describe(&e))?;
                        let named: Vec<NamedJellyfinPlaylist> = lists
                            .items
                            .into_iter()
                            .map(|i| NamedJellyfinPlaylist {
                                id: i.id,
                                name: i.name,
                            })
                            .collect();
                        store
                            .call(move |w| {
                                w.store
                                    .repair_covers(&named, now)
                                    .map(|written| written.len())
                                    .map_err(|e| e.to_string())
                            })
                            .await
                            .unwrap_or_else(|| Err("the library is closing".into()))
                    },
                    move |result| {
                        Message::Page(
                            key,
                            PageMsg::SyncStatus(crate::screens::sync_status::Message::Note(
                                repair_note(result),
                            )),
                        )
                    },
                )
                .chain(Task::done(Message::PlaylistsChanged))
                .chain(Task::done(Message::Covers(Msg::RefreshAll)))
            }
            Msg::RefreshAll => self.refresh_covers(),
        }
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

/// What Repair Covers did, in words.
pub fn repair_note(result: Result<usize, String>) -> String {
    match result {
        Ok(0) => "No playlist needed a cover from Jellyfin: none without one shares its name \
                  with exactly one Jellyfin playlist."
            .to_owned(),
        Ok(1) => "1 playlist now has the cover of the Jellyfin playlist with its name.".to_owned(),
        Ok(n) => {
            format!("{n} playlists now have the covers of the Jellyfin playlists with their names.")
        }
        Err(error) => format!("Covers couldn't be repaired: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_item_and_first_track_covers_ask_the_server() {
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
            items_to_fetch(&covers),
            [
                ("p1".to_owned(), "jp1".to_owned()),
                ("p2".to_owned(), "al1".to_owned())
            ]
        );
    }

    #[test]
    fn repairs_are_counted_in_words() {
        assert!(repair_note(Ok(0)).starts_with("No playlist needed"));
        assert!(repair_note(Ok(1)).starts_with("1 playlist now has"));
        assert!(repair_note(Ok(3)).starts_with("3 playlists now have"));
        assert!(repair_note(Err("offline".into())).ends_with("offline"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_chosen_picture_is_drawn_from_this_computer_without_asking_the_server() {
        use aoide_jellyfin::{Credentials, Device, ServerAddress};
        let server = wiremock::MockServer::start().await;
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
        let found = resolve(client, store, shared, None, Some(blobs), vec![id.clone()]).await;
        assert_eq!(found.art.get(&id), Some(&blob_key(&hash)));
        assert_eq!(found.bytes, [(blob_key(&hash), png)]);
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
