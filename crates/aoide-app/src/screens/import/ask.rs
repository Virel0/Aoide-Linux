//! Asking Melete for the songs a playlist has that the library doesn't: they
//! go to the server as a playlist import, matched on MusicBrainz and asked for
//! one by one, and followed on the Requests page.

use aoide_melete::{Error, playlist_csv};
use aoide_sidecar::ImportResult;
use iced::Task;

use super::{Message, State, Step};
use crate::app::Message as App;
use crate::screens::{Ctx, PageMsg};

/// Where asking for the missing songs stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Asking {
    #[default]
    Not,
    Sending,
    Sent,
}

/// The songs not found, as the CSV Melete reads.
pub fn missing_csv(result: &ImportResult) -> String {
    let missing = result.missing();
    let artists: Vec<String> = missing.iter().map(|t| t.artists.join(", ")).collect();
    playlist_csv(missing.iter().zip(&artists).map(|(track, artist)| {
        (
            track.title.as_str(),
            artist.as_str(),
            track.album.as_deref(),
            track.duration_ms.and_then(|ms| u64::try_from(ms).ok()),
            track.isrc.as_deref(),
        )
    }))
}

impl State {
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
}
