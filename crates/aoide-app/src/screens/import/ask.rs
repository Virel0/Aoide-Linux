//! Saving through Melete: the whole playlist goes to the server as a playlist
//! import that makes an Aoide playlist (Melete 0.4.0), every song in it, those
//! the library lacks asked for and waiting in it until they arrive. A server
//! before that refuses the request; then the songs found are saved here and the
//! rest asked for on their own, as a playlist import without a playlist.

use aoide_melete::{Error, playlist_csv};
use aoide_sidecar::{ImportResult, ImportedTrack};
use iced::Task;

use super::{Message, State, Step};
use crate::app::Message as App;
use crate::screens::{Ctx, PageMsg, Route};

/// Where asking for the missing songs stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Asking {
    #[default]
    Not,
    Sending,
    Sent,
}

/// What sending the whole playlist came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sent {
    /// Melete made the playlist: its id.
    Made(String),
    /// A server that won't make it: one before 0.4.0 (400 or 422), a user it
    /// keeps Aoide's playlists from (403), or one with MusicBrainz off (409).
    Refused,
    /// Anything else, in words.
    Failed(String),
}

/// The songs not found, as the CSV Melete reads.
pub fn missing_csv(result: &ImportResult) -> String {
    tracks_csv(&result.missing())
}

/// Every song of the playlist, in its order, as the CSV Melete reads.
pub fn whole_csv(result: &ImportResult) -> String {
    let tracks: Vec<&ImportedTrack> = result.matches.iter().map(|m| &m.track).collect();
    tracks_csv(&tracks)
}

fn tracks_csv(tracks: &[&ImportedTrack]) -> String {
    let artists: Vec<String> = tracks.iter().map(|t| t.artists.join(", ")).collect();
    playlist_csv(tracks.iter().zip(&artists).map(|(track, artist)| {
        (
            track.title.as_str(),
            artist.as_str(),
            track.album.as_deref(),
            track.duration_ms.and_then(|ms| u64::try_from(ms).ok()),
            track.isrc.as_deref(),
        )
    }))
}

/// The server's answer to a playlist import that makes a playlist.
fn sent(answer: Result<aoide_melete::PlaylistImportSummary, Error>) -> Sent {
    match answer {
        Ok(summary) => summary.playlist_id.map_or(Sent::Refused, Sent::Made),
        Err(Error::Http {
            status: 400 | 403 | 409 | 422,
            ..
        }) => Sent::Refused,
        Err(e) => Sent::Failed(crate::failure::describe(&e).to_string()),
    }
}

impl State {
    /// Sends every song to the server, which makes the playlist.
    pub(super) fn send_whole(&mut self, ctx: &Ctx<'_>) -> Task<App> {
        let Step::Matched(found) = &self.step else {
            return Task::none();
        };
        let name = self.name.trim().to_owned();
        if name.is_empty() {
            return Task::none();
        }
        let csv = whole_csv(&found.1);
        self.begin_saving();
        self.error = None;
        let client = ctx.client.clone();
        let key = ctx.key;
        Task::perform(
            async move { sent(client.import_as_playlist(&name, &csv).await) },
            move |r| App::Page(key, PageMsg::Import(Message::Sent(r))),
        )
    }

    /// Made: sync it here and open it. Refused: the two steps an older server
    /// takes. Failed: back to what was matched, saying why.
    pub(super) fn sent(&mut self, sent: Sent, ctx: &Ctx<'_>) -> Task<App> {
        match sent {
            Sent::Made(id) => {
                self.step = Step::Start;
                self.link.clear();
                self.note = None;
                Task::done(App::SyncNow)
                    .chain(Task::done(App::PlaylistsChanged))
                    .chain(Task::done(App::Open(Route::Playlist { id })))
            }
            Sent::Refused => {
                self.back_to_matched();
                // Asked first: saving moves the step on.
                let ask = self.ask_for_missing(ctx);
                Task::batch([ask, self.save(ctx)])
            }
            Sent::Failed(error) => {
                self.back_to_matched();
                self.error = Some(format!("The playlist couldn't be saved: {error}"));
                Task::none()
            }
        }
    }

    /// Matched becomes Saving, keeping what was matched.
    pub(super) fn begin_saving(&mut self) {
        if let Step::Matched(found) = std::mem::replace(&mut self.step, Step::Start) {
            self.step = Step::Saving(found);
        }
    }

    /// Saving failed or was handed back: what was matched shows again.
    pub(super) fn back_to_matched(&mut self) {
        if let Step::Saving(found) = std::mem::replace(&mut self.step, Step::Start) {
            self.step = Step::Matched(found);
        }
    }

    pub(super) fn ask_for_missing(&mut self, ctx: &Ctx<'_>) -> Task<App> {
        let Step::Matched(found) = &self.step else {
            return Task::none();
        };
        if self.asking != Asking::Not || found.1.missing().is_empty() {
            return Task::none();
        }
        let name = match self.name.trim() {
            "" => found.0.name.clone(),
            typed => typed.to_owned(),
        };
        let csv = missing_csv(&found.1);
        self.asking = Asking::Sending;
        self.error = None;
        let client = ctx.client.clone();
        let key = ctx.key;
        Task::perform(
            async move {
                client
                    .import_playlist(&name, &csv)
                    .await
                    .map(drop)
                    .map_err(|e| match e {
                        Error::Http { status: 409, .. } => {
                            "Your server can't look music up: MusicBrainz is off in its settings."
                                .to_owned()
                        }
                        e => crate::failure::describe(&e).to_string(),
                    })
            },
            move |r| App::Page(key, PageMsg::Import(Message::Asked(r))),
        )
    }

    pub(super) fn asked(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.asking = Asking::Sent;
                self.note = Some(
                    "Your server is asking for the missing songs. Follow them on the Requests page."
                        .to_owned(),
                );
            }
            Err(error) => {
                self.asking = Asking::Not;
                self.error = Some(format!("The missing songs couldn't be asked for: {error}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoide_sidecar::{ImportMatch, ImportedTrack, MatchedBy};

    #[test]
    fn only_the_songs_not_found_are_asked_for() {
        let mut undertow =
            ImportedTrack::new("Undertow", &["Odd Hours", "Mara Vey"], Some(200_000));
        undertow.album = Some("Tides".into());
        let result = ImportResult {
            matches: vec![
                ImportMatch {
                    track: ImportedTrack::new("Lantern", &["Mara Vey"], None),
                    found: Some(aoide_sidecar::MatchedTrack {
                        jellyfin_id: "t1".into(),
                        title: None,
                        artists: Vec::new(),
                        album: None,
                        duration_ms: None,
                        score: 1.0,
                    }),
                },
                ImportMatch {
                    track: undertow,
                    found: None,
                },
            ],
            by: MatchedBy::Library,
        };
        assert_eq!(
            missing_csv(&result),
            "Track Name,Artist Name(s),Album Name,Track Duration (ms),ISRC\n\
             \"Undertow\",\"Odd Hours, Mara Vey\",\"Tides\",200000,\"\"\n"
        );
    }

    #[test]
    fn saving_sends_every_song_in_the_playlist_s_order() {
        let found = aoide_sidecar::MatchedTrack {
            jellyfin_id: "t1".into(),
            title: None,
            artists: Vec::new(),
            album: None,
            duration_ms: None,
            score: 1.0,
        };
        let result = ImportResult {
            matches: vec![
                ImportMatch {
                    track: ImportedTrack::new("Undertow", &["Odd Hours"], None),
                    found: None,
                },
                ImportMatch {
                    track: ImportedTrack::new("Lantern", &["Mara Vey"], Some(200_000)),
                    found: Some(found),
                },
            ],
            by: MatchedBy::Sidecar,
        };
        assert_eq!(
            whole_csv(&result),
            "Track Name,Artist Name(s),Album Name,Track Duration (ms),ISRC\n\
             \"Undertow\",\"Odd Hours\",\"\",,\"\"\n\
             \"Lantern\",\"Mara Vey\",\"\",200000,\"\"\n"
        );
    }

    fn refusal(status: u16) -> Error {
        Error::Http {
            status,
            path: "api/v1/requests/imports".into(),
            detail: None,
            reason: None,
        }
    }

    #[test]
    fn a_server_that_won_t_make_the_playlist_gets_the_two_steps() {
        for status in [400, 403, 409, 422] {
            assert_eq!(sent(Err(refusal(status))), Sent::Refused, "{status}");
        }
        assert!(matches!(sent(Err(refusal(500))), Sent::Failed(_)));
        let summary = |playlist_id: serde_json::Value| {
            serde_json::from_value(serde_json::json!({
                "id": 2, "name": "Road trip", "state": "queued", "message": null,
                "created_at": 1_790_000_000, "tracks": 2, "asked": 0, "already": 0,
                "unmatched": 0, "waiting": 0, "in_library": 0, "playlist_id": playlist_id,
            }))
            .unwrap()
        };
        assert_eq!(
            sent(Ok(summary(serde_json::json!("P1")))),
            Sent::Made("P1".into())
        );
        assert_eq!(sent(Ok(summary(serde_json::Value::Null))), Sent::Refused);
    }
}
