//! For You (`/api/v1/music/for-you`, Melete with ListenBrainz on): songs
//! for this listener from their own play history, some in the library to
//! play, some not, to ask for, each with the song or artist of theirs it
//! came from. The server makes the list in the background and serves it at
//! once; an administrator turns it on.

use serde::Deserialize;

use crate::model::WireTrack;
use crate::{Candidate, Error, Item, MeleteClient, RequestKind, Result};

/// Where this listener's list stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForYouState {
    /// For You isn't on, or MusicBrainz is off.
    Off,
    /// Nothing finished in the last 90 days to go on.
    NoHistory,
    /// The list hasn't been made yet; it is made in the background.
    Waiting,
    /// The list is made: its songs (none when nothing was found).
    Ready,
}

/// This listener's list, best first.
#[derive(Debug, Clone, PartialEq)]
pub struct ForYou {
    /// Where the list stands.
    pub state: ForYouState,
    /// When it was made, in Unix seconds; `None` until it's ready.
    pub made_at: Option<i64>,
    /// Its songs; empty unless it's ready.
    pub items: Vec<ForYouItem>,
}

/// One song on the list, and why it's there.
#[derive(Debug, Clone, PartialEq)]
pub struct ForYouItem {
    /// To play, asked for already, or to ask for.
    pub suggestion: Suggestion,
    /// The listener's song or artist it came from.
    pub because: Because,
}

/// What can be done with a song on the list.
#[derive(Debug, Clone, PartialEq)]
pub enum Suggestion {
    /// In the library: its track.
    Playable(Box<Item>),
    /// Not in the library, and wanted already or asked for by this listener
    /// (`request_id`, their request, when they asked and weren't declined).
    Requested {
        /// The song.
        song: ForYouSong,
        /// This listener's request for it.
        request_id: Option<i64>,
    },
    /// Not in the library, to ask for.
    Requestable(ForYouSong),
}

/// A song the library doesn't have, with what asking for it takes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ForYouSong {
    /// Its MusicBrainz recording: what [`MeleteClient::ask_for`] sends.
    pub mb_recording_id: String,
    /// Its title, as ListenBrainz gives it.
    pub title: String,
    /// Its artists, as one line.
    pub artist: String,
    /// Its credited artists' MusicBrainz ids, in order; empty when unknown.
    #[serde(default, deserialize_with = "crate::model::nullable")]
    pub mb_artist_ids: Vec<String>,
    /// The album it's listed from.
    #[serde(default)]
    pub release: Option<String>,
    /// Its album's cover as a path on the server, read through
    /// [`MeleteClient::poster_url`]; `None` with covers of music not in the
    /// library off.
    #[serde(default)]
    pub poster: Option<String>,
}

impl ForYouSong {
    /// The song as a request search result, to ask for with
    /// [`MeleteClient::ask_for`].
    pub fn candidate(&self) -> Candidate {
        Candidate {
            kind: RequestKind::Song,
            mb_artist_id: self.mb_artist_ids.first().cloned(),
            mb_release_group_id: None,
            mb_recording_id: Some(self.mb_recording_id.clone()),
            title: self.title.clone(),
            year: None,
            artist: Some(self.artist.clone()),
            detail: self.release.clone(),
            in_library: false,
            wanted: false,
            requested: None,
            poster: self.poster.clone(),
            poster_credit: None,
        }
    }
}

/// Whether a song came from one of the listener's songs or artists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BecauseKind {
    /// A song they played.
    Song,
    /// An artist they played.
    Artist,
}

/// The listener's song or artist a suggestion came from.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Because {
    /// A song or an artist.
    pub kind: BecauseKind,
    /// The song's title; `None` for an artist.
    #[serde(default)]
    pub title: Option<String>,
    /// The song's artist, or the artist's name.
    pub artist: String,
}

impl Because {
    /// What to name in "Because you listened to …": the song's title, or the
    /// artist's name.
    pub fn seed(&self) -> &str {
        self.title
            .as_deref()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or(&self.artist)
    }
}

#[derive(Deserialize)]
struct WireForYou {
    state: ForYouState,
    #[serde(default)]
    made_at: Option<i64>,
    #[serde(default, deserialize_with = "crate::model::nullable")]
    items: Vec<WireItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum WireItemState {
    Playable,
    Requested,
    Requestable,
}

#[derive(Deserialize)]
struct WireItem {
    state: WireItemState,
    #[serde(default)]
    track: Option<WireTrack>,
    #[serde(default)]
    song: Option<ForYouSong>,
    #[serde(default)]
    request_id: Option<i64>,
    because: Because,
}

impl WireItem {
    /// The item, or `None` when it lacks what its state needs (a track to
    /// play, a song to ask for).
    fn into_item(self) -> Option<ForYouItem> {
        let suggestion = match self.state {
            WireItemState::Playable => Suggestion::Playable(Box::new(self.track?.into())),
            WireItemState::Requested => Suggestion::Requested {
                song: self.song?,
                request_id: self.request_id,
            },
            WireItemState::Requestable => Suggestion::Requestable(self.song?),
        };
        Some(ForYouItem {
            suggestion,
            because: self.because,
        })
    }
}

impl MeleteClient {
    /// This listener's For You list, its first `limit` songs (1 to 100).
    /// `None` from a server without For You (HTTP 404, before it).
    ///
    /// # Errors
    ///
    /// HTTP 400 as [`Error::Http`] for a `limit` out of range; otherwise the
    /// [request errors](MeleteClient#errors), but for a 404, which is
    /// `Ok(None)`.
    pub async fn for_you(&self, limit: u32) -> Result<Option<ForYou>> {
        let query = [("limit", limit.to_string())];
        let wire: WireForYou = match self.get(&["api", "v1", "music", "for-you"], &query).await {
            Ok(wire) => wire,
            Err(Error::NotFound { .. }) => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(Some(ForYou {
            state: wire.state,
            made_at: wire.made_at,
            items: wire
                .items
                .into_iter()
                .filter_map(WireItem::into_item)
                .collect(),
        }))
    }
}
