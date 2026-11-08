//! Asking for music the library doesn't have (`/api/v1/requests`): find an
//! artist, album or song on MusicBrainz, ask for it, and follow how it's
//! getting on. A request waits for an admin unless its asker's requests go
//! straight through; once approved, Melete looks for it and fetches it.

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::events::EventStream;
use crate::{CoverSize, MeleteClient, Result, Url};

/// What can be asked for in music.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestKind {
    /// A whole artist: their albums, and new ones as they come out.
    Artist,
    /// One album (a MusicBrainz release group: any edition will do).
    Album,
    /// One song.
    Song,
    /// Movies and series: not shown here, but a request list may hold them.
    #[serde(other)]
    Other,
}

impl RequestKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Artist => "artist",
            Self::Album => "album",
            Self::Song => "song",
            Self::Other => "other",
        }
    }
}

/// Something MusicBrainz knows, as a request search finds it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Candidate {
    /// Artist, album or song.
    pub kind: RequestKind,
    /// An artist's, and an album's or song's first artist.
    pub mb_artist_id: Option<String>,
    /// An album's.
    pub mb_release_group_id: Option<String>,
    /// A song's.
    pub mb_recording_id: Option<String>,
    /// The title, or an artist's name.
    pub title: String,
    /// The year it came out, when known.
    pub year: Option<i32>,
    /// An album's or song's artists, as credited.
    pub artist: Option<String>,
    /// What tells it from others of the name: an artist's kind and country, an
    /// album's type, the album a song is on and its length.
    pub detail: Option<String>,
    /// On disk already, in a library this user may see.
    pub in_library: bool,
    /// On the wanted list and looked out for.
    pub wanted: bool,
    /// This user's open request for it, by id.
    pub requested: Option<i64>,
    /// Its picture, as a path on the server: an album's or song's cover, an
    /// artist's portrait. Fetched with the token at a [`CoverSize`]
    /// ([`MeleteClient::poster_url`]). `None` without one, and from a
    /// server that doesn't show them (off by default, and before 0.8.0).
    ///
    /// [`CoverSize`]: crate::CoverSize
    #[serde(default)]
    pub poster: Option<String>,
    /// Who made an artist's portrait and under what licence, to show with it.
    #[serde(default)]
    pub poster_credit: Option<PosterCredit>,
}

/// What a portrait's licence asks to be shown with it, as Wikimedia Commons
/// gives it. Any part may be missing.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub struct PosterCredit {
    /// Who made it, as plain text.
    pub author: Option<String>,
    /// Its licence's short name, such as "CC BY-SA 4.0".
    pub licence: Option<String>,
    /// Where the licence is written.
    pub licence_url: Option<String>,
    /// The picture's page on Commons.
    pub source_url: Option<String>,
}

impl Candidate {
    /// Whether there's anything left to ask: not here, not wanted, not asked for.
    pub fn askable(&self) -> bool {
        !self.in_library && !self.wanted && self.requested.is_none()
    }
}

/// Where a request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestState {
    /// Waiting for an admin.
    Requested,
    /// On the wanted list: looked for and fetched.
    Approved,
    /// Turned down, with a reason.
    Declined,
}

/// How an approved request is getting on.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Progress {
    /// wanted, searching, downloading, transferring, importing, in_library or failed.
    pub step: String,
    /// How much is downloaded, while downloading.
    pub percent: Option<f64>,
    /// The latest word on it, in plain words.
    pub message: Option<String>,
    /// Where it's coming from: `torrent` or `soulseek`.
    pub source: Option<String>,
    /// Unix seconds it last changed; none for a step worked out from the library.
    pub updated_at: Option<i64>,
    /// While it's copied home: how far that has got.
    pub copied: Option<Copying>,
}

/// A download being copied home to the server.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Copying {
    /// Bytes copied so far.
    pub bytes: u64,
    /// Bytes to copy in all.
    pub total: u64,
    /// The copy's speed so far, once it has run a second.
    pub bytes_per_second: Option<u64>,
    /// At that speed, rounded up.
    pub seconds_left: Option<u64>,
}

/// One of this user's requests.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MusicRequest {
    /// The server's id for it.
    pub id: i64,
    /// What was asked for.
    pub kind: RequestKind,
    /// The title, or an artist's name.
    pub title: String,
    /// The year it came out, when known.
    pub year: Option<i32>,
    /// An album's or song's artists.
    pub artist: Option<String>,
    /// Where it stands.
    pub state: RequestState,
    /// Why it was declined.
    pub reason: Option<String>,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds it was approved or declined.
    pub decided_at: Option<i64>,
    /// Who decided; none when it was approved at once.
    pub decided_by: Option<String>,
    /// How it's getting on, once approved; none before, and for a whole artist.
    pub progress: Option<Progress>,
}

#[derive(Deserialize)]
struct Page {
    items: Vec<MusicRequest>,
}

impl MeleteClient {
    /// Artists, albums or songs on MusicBrainz matching `words`, best first (20
    /// at most), each marked in the library, wanted, or asked for already.
    ///
    /// # Errors
    ///
    /// HTTP 409 as [`Error::Http`](crate::Error::Http) when the server has
    /// MusicBrainz off; 502 when MusicBrainz didn't answer; otherwise the
    /// [request errors](MeleteClient#errors).
    pub async fn request_search(&self, kind: RequestKind, words: &str) -> Result<Vec<Candidate>> {
        let query = [
            ("kind", kind.as_str().to_owned()),
            ("q", words.trim().to_owned()),
        ];
        self.get(&["api", "v1", "requests", "search"], &query).await
    }

    /// A search result's [`poster`](Candidate::poster) at most `size` wide,
    /// beneath the server's own address so a reverse proxy's prefix is kept.
    /// Fetching it needs [`authorization`](Self::authorization).
    ///
    /// `None` for anything but a plain absolute path on this server, as
    /// [`avatar_url`](Self::avatar_url) refuses.
    pub fn poster_url(&self, poster: &str, size: CoverSize) -> Option<Url> {
        let mut url = self.avatar_url(poster)?;
        url.query_pairs_mut()
            .append_pair("size", &size.pixels().to_string());
        Some(url)
    }

    /// Asks for `candidate`: approved at once for an admin, or a user whose
    /// requests go straight through; otherwise it waits for an admin.
    ///
    /// # Errors
    ///
    /// HTTP 409 as [`Error::Http`](crate::Error::Http) when it's been asked for
    /// already; otherwise the [request errors](MeleteClient#errors).
    pub async fn ask_for(&self, candidate: &Candidate) -> Result<MusicRequest> {
        // Only the id of its own kind: another kind's is refused.
        let (field, id) = match candidate.kind {
            RequestKind::Artist => ("mb_artist_id", &candidate.mb_artist_id),
            RequestKind::Album => ("mb_release_group_id", &candidate.mb_release_group_id),
            _ => ("mb_recording_id", &candidate.mb_recording_id),
        };
        let mut body = json!({ "kind": candidate.kind.as_str() });
        body[field] = json!(id);
        self.send_decoded(Method::POST, &["api", "v1", "requests"], &[], Some(&body))
            .await
    }

    /// This user's music requests, newest first (the latest 200).
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors).
    pub async fn music_requests(&self) -> Result<Vec<MusicRequest>> {
        let page: Page = self
            .get(
                &["api", "v1", "requests"],
                &[("limit", "200".to_owned()), ("service", "music".to_owned())],
            )
            .await?;
        Ok(page.items)
    }

    /// Takes back a request still waiting for an admin.
    ///
    /// # Errors
    ///
    /// HTTP 409 as [`Error::Http`](crate::Error::Http) once it's been decided;
    /// otherwise the [request errors](MeleteClient#errors).
    pub async fn withdraw_request(&self, id: i64) -> Result<()> {
        let id = id.to_string();
        self.send_empty(Method::DELETE, &["api", "v1", "requests", &id], &[])
            .await
    }

    /// Follows this user's requests live: their 50 newest, each time one changes
    /// (Melete looks every 2 seconds). The stream ends after 10 minutes; open
    /// another.
    ///
    /// # Errors
    ///
    /// The [request errors](MeleteClient#errors).
    pub async fn follow_requests(&self) -> Result<RequestFeed> {
        let response = self
            .open_events(&["api", "v1", "requests", "events"])
            .await?;
        Ok(RequestFeed {
            events: EventStream::new(response),
        })
    }
}

/// This user's requests as they change; see [`MeleteClient::follow_requests`].
pub struct RequestFeed {
    events: EventStream,
}

impl RequestFeed {
    /// The music requests as they stand now; `None` once the stream has ended.
    ///
    /// # Errors
    ///
    /// When the connection fails, or an event can't be read.
    pub async fn next(&mut self) -> Result<Option<Vec<MusicRequest>>> {
        while let Some(event) = self.events.next().await? {
            if event.name != "requests" {
                continue;
            }
            let requests: Vec<MusicRequest> =
                serde_json::from_str(&event.data).map_err(|e| crate::Error::Decoding {
                    path: "api/v1/requests/events".to_owned(),
                    message: e.to_string(),
                })?;
            return Ok(Some(
                requests
                    .into_iter()
                    .filter(|r| r.kind != RequestKind::Other)
                    .collect(),
            ));
        }
        Ok(None)
    }
}
