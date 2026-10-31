//! Asking for music the library doesn't have (`/api/v1/requests`): find an
//! artist, album or song on MusicBrainz, ask for it, and follow how it's
//! getting on. A request waits for an admin unless its asker's requests go
//! straight through; once approved, Melete looks for it and fetches it.

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{MeleteClient, Result};

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
}
