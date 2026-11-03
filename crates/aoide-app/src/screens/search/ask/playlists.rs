//! Putting a song the library doesn't have in a hand-made playlist, from Ask
//! for It: asked for first when it isn't yet, then added as a waiting entry,
//! greyed in the playlist until Melete swaps its track in.

use aoide_melete::{Candidate, MusicRequest, RequestKind};
use aoide_store::PendingSong;
use iced::widget::{Column, button, column, text, text_input};
use iced::{Element, Fill, Task};

use super::{Asking, Message, wrap};
use crate::app::Message as App;
use crate::screens::{Ctx, View};
use crate::{actions, theme, widgets};

/// A song put in a playlist: the playlist's name, and the request made for it
/// when it hadn't been asked for yet.
#[derive(Debug, Clone)]
pub struct Added {
    pub playlist: String,
    pub request: Option<MusicRequest>,
}

/// Whether `candidate` is a song that could wait in a playlist.
fn can_wait(candidate: &Candidate) -> bool {
    candidate.kind == RequestKind::Song
        && !candidate.in_library
        && candidate.mb_recording_id.is_some()
}

impl Asking {
    /// This user's request for the candidate at `place`: asked for from here,
    /// or before.
    fn request_for(&self, place: usize, candidate: &Candidate) -> Option<i64> {
        let asked_here = self.asked.iter().find(|(p, _)| *p == place);
        asked_here
            .and_then(|(_, answer)| answer.as_ref().ok().map(|r| r.id))
            .or(candidate.requested)
    }

    /// Asks for the song at `place` unless it has been, then adds it to the
    /// playlist `id` as a waiting entry.
    pub(super) fn add(
        &mut self,
        place: usize,
        id: String,
        name: String,
        ctx: &Ctx<'_>,
    ) -> Task<App> {
        let Some(candidate) = self.found.ready().and_then(|f| f.get(place)).cloned() else {
            return Task::none();
        };
        if self.adding.is_some() || !can_wait(&candidate) {
            return Task::none();
        }
        self.adding = Some(place);
        self.choosing = None;
        let known = self.request_for(place, &candidate);
        let (client, store, key) = (ctx.client.clone(), ctx.store.clone(), ctx.key);
        Task::perform(
            async move {
                let (request_id, request) = match known {
                    Some(id) => (id, None),
                    None => {
                        let asked = client.ask_for(&candidate).await.map_err(|e| {
                            format!("It couldn't be asked for: {}", crate::failure::describe(&e))
                        })?;
                        (asked.id, Some(asked))
                    }
                };
                let song = PendingSong::requested(request_id, candidate.title, candidate.artist);
                let now = aoide_store::now_millis();
                store
                    .call(move |w| w.store.add_waiting(&[song], &id, now))
                    .await
                    .ok_or_else(|| "the library is closing".to_owned())?
                    .map_err(|e| e.to_string())?;
                Ok(Box::new(Added {
                    playlist: name,
                    request,
                }))
            },
            move |r| App::Page(key, wrap(Message::Added(place, r))),
        )
    }

    /// The song is in the playlist (and asked for), or why not.
    pub(super) fn added(&mut self, place: usize, result: Result<Box<Added>, String>) -> Task<App> {
        self.adding = None;
        self.added.retain(|(p, _)| *p != place);
        let Ok(added) = result else {
            self.added.push((place, result.map(|a| a.playlist)));
            return Task::none();
        };
        if let Some(request) = added.request {
            self.answer(place, Ok(request));
        }
        self.added.push((place, Ok(added.playlist)));
        Task::done(App::PlaylistsChanged)
    }

    /// "Added to Road Trip", or why it couldn't be.
    pub(super) fn added_note(&self, place: usize) -> Option<String> {
        self.added
            .iter()
            .find(|(p, _)| *p == place)
            .map(|(_, result)| match result {
                Ok(playlist) => format!("Added to {playlist}"),
                Err(why) => why.clone(),
            })
    }

    /// Add to Playlist… for a song not in the library.
    pub(super) fn add_button<'a>(
        &self,
        place: usize,
        candidate: &Candidate,
        key: u64,
    ) -> Option<Element<'a, App>> {
        if !can_wait(candidate) {
            return None;
        }
        let busy = self.adding == Some(place);
        let label = if busy {
            "Adding…"
        } else {
            "Add to playlist…"
        };
        let press = App::Page(key, wrap(Message::Choose(place)));
        Some(
            button(text(label).size(12))
                .style(theme::tonal)
                .padding([4, 12])
                .on_press_maybe(self.adding.is_none().then_some(press))
                .into(),
        )
    }

    /// The hand-made playlists to add the song at `place` to, found by name
    /// once there are many.
    pub(super) fn choices<'a>(&'a self, place: usize, view: &View<'a>) -> Element<'a, App> {
        let key = view.key;
        let message = move |m: Message| App::Page(key, wrap(m));
        let hand_made: Vec<_> = view.playlists.iter().filter(|p| !p.smart).collect();
        let mut list = Column::new().spacing(2);
        if hand_made.len() > actions::PLAYLIST_SEARCH_AFTER {
            list = list.push(
                text_input("Find a playlist", &self.filter)
                    .on_input(move |s| message(Message::Filter(s)))
                    .padding(6)
                    .size(13)
                    .style(theme::field),
            );
        }
        let shown = actions::matching(&hand_made, |p| p.name.as_str(), &self.filter);
        if shown.is_empty() {
            list = list.push(widgets::secondary("No playlists of your own yet.").size(13));
        }
        for playlist in shown {
            let add = Message::AddTo {
                place,
                id: playlist.id.clone(),
                name: playlist.name.clone(),
            };
            list = list.push(
                button(text(playlist.name.as_str()).size(13))
                    .width(Fill)
                    .style(theme::row)
                    .padding([4, 8])
                    .on_press(message(add)),
            );
        }
        iced::widget::container(column![list])
            .padding(8)
            .width(Fill)
            .style(theme::menu)
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(in_library: bool, recording: Option<&str>) -> Candidate {
        serde_json::from_value(serde_json::json!({
            "kind": "song", "mb_artist_id": null, "mb_release_group_id": null,
            "mb_recording_id": recording, "title": "No Surprises", "year": 1997,
            "artist": "Radiohead", "detail": null, "in_library": in_library,
            "wanted": false, "requested": null,
        }))
        .unwrap()
    }

    #[test]
    fn only_a_song_not_in_the_library_can_wait_in_a_playlist() {
        assert!(can_wait(&song(false, Some("rec-1"))));
        assert!(!can_wait(&song(true, Some("rec-1"))));
        assert!(!can_wait(&song(false, None)), "nothing to ask for it by");
        let mut album = song(false, Some("rec-1"));
        album.kind = RequestKind::Album;
        assert!(!can_wait(&album));
    }
}
