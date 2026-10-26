//! A row's `…` menu: opening it, and what each of its items does.

use iced::Task;

use super::{App, Message};
use crate::actions::{Collection, Flags, Menu, PlayRequest, RowAction};
use crate::playback::Enqueue;
use crate::playback::tracks::TrackMeta;
use crate::screens::Route;

impl App {
    pub(super) fn row_action(&mut self, action: RowAction) -> Task<Message> {
        if self.session.is_none() {
            return Task::none();
        }
        match action {
            RowAction::Open { anchor, tracks } => self.open_menu(anchor, tracks),
            RowAction::OpenCollection {
                anchor,
                collection,
                title,
            } => self.open_collection_menu(anchor, collection, title),
            RowAction::CollectionTracks(anchor, result) => self.collection_tracks(&anchor, result),
            RowAction::Play | RowAction::Shuffle => {
                self.play_menu(matches!(action, RowAction::Shuffle))
            }
            RowAction::FlagsLoaded(id, flags) => self.flags_loaded(&id, flags),
            RowAction::Close => {
                self.close_menu();
                Task::none()
            }
            RowAction::PlayNext => self.enqueue_menu(Enqueue::Next),
            RowAction::PlayLast => self.enqueue_menu(Enqueue::Last),
            RowAction::ShowPlaylists => {
                if let Some(menu) = self.menu_mut() {
                    menu.adding = !menu.adding;
                }
                self.offer_jellyfin_playlists()
            }
            RowAction::JellyfinPlaylists(_) | RowAction::AddToJellyfin(_) => {
                self.jellyfin_menu_action(action)
            }
            RowAction::GetInfo => self.menu_info(),
            RowAction::StartRadio => self.menu_radio(),
            RowAction::Filter(filter) => {
                if let Some(menu) = self.menu_mut() {
                    menu.filter = filter;
                }
                Task::none()
            }
            RowAction::NewName(name) => {
                if let Some(menu) = self.menu_mut() {
                    menu.new_name = name;
                }
                Task::none()
            }
            RowAction::AddTo(id) => self.add_to_playlist(Some(id)),
            RowAction::CreatePlaylist => self.add_to_playlist(None),
            RowAction::NotInterested => self.toggle_flag(true),
            RowAction::DontCount => self.toggle_flag(false),
            RowAction::Favourite => self.favourite(),
            RowAction::GoToAlbum => self.go_to(true),
            RowAction::GoToArtist => self.go_to(false),
        }
    }

    /// The open menu, if any.
    fn menu(&self) -> Option<&Menu> {
        self.session.as_ref()?.menu.as_ref()
    }

    fn menu_mut(&mut self) -> Option<&mut Menu> {
        self.session.as_mut()?.menu.as_mut()
    }

    fn close_menu(&mut self) {
        if let Some(signed) = self.session.as_mut() {
            signed.menu = None;
        }
    }

    /// The menu taken out of the session: its item was chosen.
    fn take_menu(&mut self) -> Option<Menu> {
        self.session.as_mut()?.menu.take()
    }

    /// A song row's menu (or several songs'); a second click closes it.
    fn open_menu(&mut self, anchor: String, tracks: Vec<TrackMeta>) -> Task<Message> {
        let store = self.store.clone();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        if signed.menu.as_ref().is_some_and(|m| m.anchor == anchor) {
            signed.menu = None;
            return Task::none();
        }
        let single = (tracks.len() == 1).then(|| tracks[0].id.clone());
        signed.menu = Some(Menu::new(anchor, tracks));
        let Some(id) = single else {
            return Task::none();
        };
        Task::perform(
            async move {
                let asked = id.clone();
                let flags = store
                    .call(move |w| w.store.flags(&asked).ok().flatten())
                    .await
                    .flatten();
                let flags = flags.map_or_else(Flags::default, |f| Flags {
                    not_interested: f.not_interested,
                    dont_count: f.dont_count,
                });
                (id, flags)
            },
            |(id, flags)| Message::Row(RowAction::FlagsLoaded(id, flags)),
        )
    }

    /// An album's, artist's or playlist's menu: its songs are fetched for it.
    fn open_collection_menu(
        &mut self,
        anchor: String,
        collection: Collection,
        title: String,
    ) -> Task<Message> {
        let store = self.store.clone();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        if signed.menu.as_ref().is_some_and(|m| m.anchor == anchor) {
            signed.menu = None;
            return Task::none();
        }
        signed.menu = Some(Menu::for_collection(
            anchor.clone(),
            collection.clone(),
            title,
        ));
        let client = signed.connection.client.clone();
        Task::perform(
            crate::actions::collection_tracks(client, store, collection),
            move |result| match result {
                Err(crate::actions::Failure::Server(aoide_jellyfin::Error::Unauthorized)) => {
                    Message::TokenRejected
                }
                result => Message::Row(RowAction::CollectionTracks(
                    anchor.clone(),
                    result
                        .map(|items| items.iter().map(TrackMeta::from_item).collect())
                        .map_err(|failure| match failure {
                            crate::actions::Failure::Server(e) => {
                                crate::failure::describe(&e).to_string()
                            }
                            crate::actions::Failure::Store(e) => e,
                        }),
                )),
            },
        )
    }

    fn collection_tracks(
        &mut self,
        anchor: &str,
        result: Result<Vec<TrackMeta>, String>,
    ) -> Task<Message> {
        if let Some(menu) = self.menu_mut().filter(|m| m.anchor == anchor) {
            menu.loading = false;
            match result {
                Ok(tracks) if tracks.is_empty() => {
                    menu.error = Some("There are no songs here.".to_owned());
                }
                Ok(tracks) => menu.tracks = tracks,
                Err(error) => menu.error = Some(error),
            }
        }
        Task::none()
    }

    /// The single song's flags, read for the menu opened on it.
    fn flags_loaded(&mut self, id: &str, flags: Flags) -> Task<Message> {
        if let Some(menu) = self
            .menu_mut()
            .filter(|m| m.single().is_some_and(|t| t.id == id))
        {
            menu.flags = Some(flags);
        }
        Task::none()
    }

    /// Get Info, on the menu's song.
    fn menu_info(&mut self) -> Task<Message> {
        match self.menu().and_then(Menu::single).cloned() {
            Some(track) => self.info(crate::info::Msg::Open {
                id: track.id,
                title: track.title,
            }),
            None => Task::none(),
        }
    }

    /// Start Radio, from whatever the menu is on.
    fn menu_radio(&mut self) -> Task<Message> {
        match self.menu().and_then(crate::radio::Station::of_menu) {
            Some(station) => self.radio(crate::radio::Msg::Start(station)),
            None => Task::none(),
        }
    }

    /// Play, or Shuffle: the menu's songs replace the queue.
    fn play_menu(&mut self, shuffle: bool) -> Task<Message> {
        let Some(menu) = self.take_menu() else {
            return Task::none();
        };
        if menu.tracks.is_empty() {
            return Task::none();
        }
        let request = if shuffle {
            PlayRequest::shuffled(&menu.tracks, aoide_store::PlaySource::Shuffle)
        } else {
            PlayRequest::from(&menu.tracks, 0, menu.source())
        };
        let context = menu
            .collection
            .as_ref()
            .and_then(|(c, title)| crate::handoff::collection_context(c, title));
        let remembered = self.remembered(context);
        Task::batch([self.play(request), remembered])
    }

    /// Play Next, or Play Last.
    fn enqueue_menu(&mut self, how: Enqueue) -> Task<Message> {
        let Some(menu) = self.take_menu() else {
            return Task::none();
        };
        let source = menu.source();
        self.play(PlayRequest {
            tracks: menu.tracks,
            how,
            source,
        })
    }

    /// Adds the menu's songs to this playlist, or (`None`) to a new one
    /// named as typed; with no name typed, the menu stays open.
    fn add_to_playlist(&mut self, playlist: Option<String>) -> Task<Message> {
        let store = self.store.clone();
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let Some(menu) = signed.menu.take() else {
            return Task::none();
        };
        let target = playlist.ok_or_else(|| menu.new_name.trim().to_owned());
        if target.as_ref().is_err_and(String::is_empty) {
            signed.menu = Some(menu);
            return Task::none();
        }
        let tracks = menu.tracks;
        let now = aoide_store::now_millis();
        Task::perform(
            async move {
                store
                    .call(move |w| {
                        let id = match target {
                            Ok(id) => id,
                            Err(name) => w.store.create_playlist(&name, None, None, now)?.id,
                        };
                        let rows: Vec<_> = tracks.iter().map(|t| t.track_ref(now)).collect();
                        w.store.cache_tracks(&rows)?;
                        let links: Vec<_> = tracks.iter().map(|t| t.link()).collect();
                        w.store.add_tracks(&links, &id, now)?;
                        Ok::<_, aoide_store::StoreError>(())
                    })
                    .await
                    .map(|r| r.map_err(|e| e.to_string()))
                    .unwrap_or_else(|| Err("the library is closing".into()))
            },
            |result| match result {
                Ok(()) => Message::PlaylistsChanged,
                Err(error) => Message::Notice(format!("The songs couldn't be added: {error}")),
            },
        )
    }

    /// Not Interested (true), or Don't Count: flipped for the menu's song.
    fn toggle_flag(&mut self, not_interested: bool) -> Task<Message> {
        let Some(menu) = self.menu_mut() else {
            return Task::none();
        };
        let Some(track) = menu.single().cloned() else {
            return Task::none();
        };
        let mut flags = menu.flags.unwrap_or_default();
        if not_interested {
            flags.not_interested = !flags.not_interested;
        } else {
            flags.dont_count = !flags.dont_count;
        }
        menu.flags = Some(flags);
        let now = aoide_store::now_millis();
        self.store.send(move |w| {
            let _ = w.store.cache_tracks(&[track.track_ref(now)]);
            let key = track.content_key();
            let _ = if not_interested {
                w.store
                    .set_not_interested(flags.not_interested, &track.id, &key, now)
            } else {
                w.store
                    .set_dont_count(flags.dont_count, &track.id, &key, now)
            };
        });
        Task::none()
    }

    fn favourite(&mut self) -> Task<Message> {
        let Some(signed) = self.session.as_mut() else {
            return Task::none();
        };
        let Some(track) = signed.menu.as_ref().and_then(Menu::single).cloned() else {
            return Task::none();
        };
        let current = signed
            .favourites
            .get(&track.id)
            .copied()
            .unwrap_or(track.favourite);
        let client = signed.connection.client.clone();
        let id = track.id.clone();
        Task::perform(
            async move { client.set_favorite(&id, !current).await },
            move |result| match result {
                Err(aoide_jellyfin::Error::Unauthorized) => Message::TokenRejected,
                result => Message::Favourited(
                    track.id.clone(),
                    result
                        .map(|data| data.is_favorite)
                        .map_err(|e| crate::failure::describe(&e).to_string()),
                ),
            },
        )
    }

    /// The server's answer to Favourite.
    pub(super) fn favourited(&mut self, id: String, result: Result<bool, String>) -> Task<Message> {
        match result {
            Ok(favourite) => {
                if let Some(signed) = self.session.as_mut() {
                    signed.favourites.insert(id, favourite);
                }
            }
            Err(error) => self.notice = Some(error),
        }
        Task::none()
    }

    /// Go to Album (true), or Go to Artist.
    fn go_to(&mut self, album: bool) -> Task<Message> {
        let Some(track) = self.take_menu().and_then(|m| m.single().cloned()) else {
            return Task::none();
        };
        let route = if album {
            track.album_id.map(|id| Route::Album {
                id,
                title: track.album.clone(),
            })
        } else {
            track.artist_id.map(|id| Route::Artist {
                id,
                name: track
                    .album_artist
                    .clone()
                    .unwrap_or_else(|| track.artist.clone()),
            })
        };
        route.map_or_else(Task::none, |route| self.open(route))
    }
}
