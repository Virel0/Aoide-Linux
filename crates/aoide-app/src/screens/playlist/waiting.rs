//! A playlist's songs in its order, those the library has and those still
//! waiting for theirs (Melete's pending entries), and where each waiting song
//! stands, in words, from this user's music requests.

use std::collections::HashMap;

use aoide_melete::MusicRequest;
use aoide_store::{PendingSong, PlaylistItem};
use iced::Task;

use super::{Message, State};
use crate::app::Message as App;
use crate::playback::tracks::TrackMeta;
use crate::screens::{Ctx, PageMsg, fetch};

/// One line of the list: a playable track, or a song not in the library yet,
/// by place in `State::tracks` or `State::waiting`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    Track(usize),
    Waiting(usize),
}

/// A playlist's entries sorted out: the tracks found (with the entry behind
/// each), the songs waiting (with theirs), and the lines in the playlist's order.
#[derive(Debug, Default, PartialEq)]
pub struct Arranged {
    pub entries: Vec<String>,
    pub tracks: Vec<TrackMeta>,
    pub waiting: Vec<(String, PendingSong)>,
    pub lines: Vec<Line>,
}

/// Sorts `items` out against the tracks the server `found`, by id. An entry
/// whose track is gone (or marked unresolved) is left out: it is counted as
/// missing, not shown.
pub fn arrange(items: &[PlaylistItem], found: &HashMap<String, TrackMeta>) -> Arranged {
    let mut arranged = Arranged::default();
    for item in items {
        if let Some(song) = item.pending.as_ref().filter(|_| item.is_waiting()) {
            arranged.lines.push(Line::Waiting(arranged.waiting.len()));
            arranged.waiting.push((item.id.clone(), song.clone()));
        } else if !item.is_unresolved()
            && let Some(track) = found.get(item.linked_jellyfin_id())
        {
            arranged.lines.push(Line::Track(arranged.tracks.len()));
            arranged.entries.push(item.id.clone());
            arranged.tracks.push(track.clone());
        }
    }
    arranged
}

/// The ids to ask the server for: every entry with a track that may resolve.
pub fn track_ids(items: &[PlaylistItem]) -> Vec<String> {
    items
        .iter()
        .filter(|item| !item.is_unresolved() && !item.is_waiting())
        .map(|item| item.linked_jellyfin_id().to_owned())
        .collect()
}

/// Where a waiting song stands: its request's standing, or why there is none.
pub fn status(song: &PendingSong, requests: &HashMap<i64, MusicRequest>) -> String {
    match song.request_id {
        None => song
            .why
            .as_deref()
            .map_or_else(|| "Not found".to_owned(), |why| format!("Not found: {why}")),
        // Not among the latest requests, or taken back: still asked for here.
        Some(id) => requests.get(&id).map_or_else(
            || "Asked for".to_owned(),
            crate::screens::requests::standing,
        ),
    }
}

impl State {
    /// The lines again, in the order of `items`, after an entry moved or went.
    pub(super) fn relines(&mut self) {
        let tracks: HashMap<&str, usize> = self
            .entries
            .iter()
            .enumerate()
            .map(|(at, id)| (id.as_str(), at))
            .collect();
        let waiting: HashMap<&str, usize> = self
            .waiting
            .iter()
            .enumerate()
            .map(|(at, (id, _))| (id.as_str(), at))
            .collect();
        self.lines = self
            .items
            .iter()
            .filter_map(|item| {
                let id = item.id.as_str();
                tracks
                    .get(id)
                    .map(|&at| Line::Track(at))
                    .or_else(|| waiting.get(id).map(|&at| Line::Waiting(at)))
            })
            .collect();
    }

    /// This user's music requests, when a waiting song has one to look up.
    pub(super) fn find_requests(&self, ctx: &Ctx<'_>) -> Task<App> {
        let asked = self.items.iter().any(|item| {
            item.is_waiting()
                && item
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.request_id.is_some())
        });
        if !asked {
            return Task::none();
        }
        let client = ctx.client.clone();
        fetch(ctx.key, async move { client.music_requests().await }, |r| {
            PageMsg::Playlist(Message::Requests(r))
        })
    }

    /// Takes a waiting song off the list at once; the store soft-deletes it.
    pub(super) fn remove_waiting(&mut self, entry: &str) -> bool {
        let Some(at) = self.waiting.iter().position(|(id, _)| id == entry) else {
            return false;
        };
        self.waiting.remove(at);
        self.items.retain(|i| i.id != entry);
        self.relines();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, track: &str, pending: Option<PendingSong>) -> PlaylistItem {
        let mut item = PlaylistItem::new("p", track, format!("k-{track}"), "V", 1, "desk");
        item.id = id.to_owned();
        if pending.is_some() {
            item.jellyfin_id.clear();
            item.content_key.clear();
        }
        item.pending = pending;
        item
    }

    fn song(request_id: Option<i64>, why: Option<&str>) -> PendingSong {
        PendingSong {
            request_id,
            title: "No Surprises".into(),
            artist: Some("Radiohead".into()),
            album: None,
            duration_ms: None,
            why: why.map(str::to_owned),
        }
    }

    #[test]
    fn waiting_songs_keep_their_place_and_are_never_asked_for_as_tracks() {
        let items = [
            item("a", "t1", None),
            item("b", "", Some(song(Some(5), None))),
            item("gone", "t9", None),
            item("c", "t2", None),
        ];
        assert_eq!(track_ids(&items), ["t1", "t9", "t2"]);
        let found: HashMap<String, TrackMeta> = ["t1", "t2"]
            .into_iter()
            .map(|id| {
                let item = aoide_melete::Item {
                    id: id.to_owned(),
                    ..aoide_melete::Item::default()
                };
                (id.to_owned(), TrackMeta::from_item(&item))
            })
            .collect();
        let arranged = arrange(&items, &found);
        assert_eq!(arranged.entries, ["a", "c"]);
        assert_eq!(arranged.waiting.len(), 1);
        assert_eq!(
            arranged.lines,
            [Line::Track(0), Line::Waiting(0), Line::Track(1)]
        );
    }

    #[test]
    fn a_waiting_song_stands_as_its_request_does_or_says_why_there_is_none() {
        let none = HashMap::new();
        assert_eq!(
            status(&song(None, Some("no recording fits")), &none),
            "Not found: no recording fits"
        );
        assert_eq!(status(&song(None, None), &none), "Not found");
        assert_eq!(status(&song(Some(5), None), &none), "Asked for");
        let request: MusicRequest = serde_json::from_value(serde_json::json!({
            "id": 5, "kind": "song", "title": "No Surprises", "year": null,
            "artist": "Radiohead", "state": "requested", "reason": null,
            "created_at": 1_790_000_000, "decided_at": null, "decided_by": null,
            "progress": null,
        }))
        .unwrap();
        let requests = HashMap::from([(5, request)]);
        assert_eq!(
            status(&song(Some(5), None), &requests),
            "Waiting for approval"
        );
    }
}
