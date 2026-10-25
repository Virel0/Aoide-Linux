//! The sidecar over HTTP: same origin and same token as Jellyfin.

use std::collections::BTreeMap;
use std::time::Duration;

use aoide_store::{PullCursor, PullResponse, PushRequest, PushResponse};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{Client, Method, RequestBuilder, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::SidecarError;
use crate::types::{
    AnalysisCoverage, AudioAnalysisResponse, DiscoverResponse, NextRequest, NextResponse,
    PlaylistShare, QueueEntry, ShareRequest, SoundBounds, SoundBoundsResponse, SyncStatus,
    list_from,
};

/// How many ids the measurement endpoints take per call.
pub const MAX_IDS_PER_CALL: usize = 200;

/// How much of an error body is kept: an ASP.NET exception page runs to kilobytes, and
/// the first line names the fault.
const DETAIL_LIMIT: usize = 400;

/// A connection to one user's sidecar.
///
/// Paths are joined onto the server URL segment by segment, so a Jellyfin behind a
/// reverse-proxy subpath (`https://host/jellyfin`) keeps its prefix. Bodies are read
/// with a plain JSON decoder: op payloads are opaque client JSON the server stores
/// verbatim, and a key spelled `Notes` must arrive as `Notes`.
#[derive(Clone)]
pub struct SidecarClient {
    http: Client,
    base: Url,
}

impl std::fmt::Debug for SidecarClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The token stays out of logs.
        formatter
            .debug_struct("SidecarClient")
            .field("base", &self.base.as_str())
            .finish_non_exhaustive()
    }
}

impl SidecarClient {
    /// `base_url` is the Jellyfin server address the user signed in to; `token` is the
    /// Jellyfin access token. There is no separate login.
    pub fn new(base_url: &str, token: &str) -> Result<Self, SidecarError> {
        let base = Url::parse(base_url.trim())
            .map_err(|error| SidecarError::InvalidUrl(error.to_string()))?;
        if !matches!(base.scheme(), "http" | "https") || base.cannot_be_a_base() {
            return Err(SidecarError::InvalidUrl(format!(
                "{} is not an http(s) address",
                base.scheme()
            )));
        }

        let authorization = HeaderValue::from_str(&format!("MediaBrowser Token=\"{token}\""))
            .map_err(|_| {
                SidecarError::InvalidUrl(
                    "the token contains characters a header cannot carry".to_owned(),
                )
            })?;
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization.clone());
        // Jellyfin 10.8 and earlier read this one instead.
        headers.insert("X-Emby-Authorization", authorization);
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

        // ring, never aws-lc: see the manifest. Installing is first-wins and harmless
        // when another part of the process got there first.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = Client::builder()
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(60))
            .redirect(same_origin_redirects())
            .build()
            .map_err(|error| SidecarError::Network(error.to_string()))?;
        Ok(Self { http, base })
    }

    /// The URL for `path` under the server, with `query` appended.
    pub fn url(&self, path: &str, query: &[(&str, String)]) -> Url {
        let mut url = self.base.clone();
        if let Ok(mut segments) = url.path_segments_mut() {
            segments.pop_if_empty();
            segments.extend(path.split('/').filter(|segment| !segment.is_empty()));
        }
        if !query.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (name, value) in query {
                pairs.append_pair(name, value);
            }
        }
        url
    }

    // MARK: - Sync

    /// `GET /aoide/sync/status`.
    pub async fn status(&self) -> Result<SyncStatus, SidecarError> {
        self.get_json("aoide/sync/status", &[]).await
    }

    /// `POST /aoide/sync/push`. Safe to repeat: the server is idempotent on op id.
    pub async fn push(&self, request: &PushRequest) -> Result<PushResponse, SidecarError> {
        self.send_json(Method::POST, "aoide/sync/push", &[], request)
            .await
    }

    /// `GET /aoide/sync/pull`. `since` is always sent — as 0 on a device that has never
    /// synced — because a server binding it as required would otherwise fail the very
    /// first request a new device makes.
    pub async fn pull(
        &self,
        since: Option<PullCursor>,
        limit: usize,
    ) -> Result<PullResponse, SidecarError> {
        let query = [
            ("since", since.map_or(0, |cursor| cursor.0).to_string()),
            ("limit", limit.to_string()),
        ];
        self.get_json("aoide/sync/pull", &query).await
    }

    // MARK: - Covers

    /// `HEAD /aoide/images/{sha256}`: whether the server already holds these bytes.
    pub async fn has_image(&self, sha256: &str) -> Result<bool, SidecarError> {
        let path = image_path(sha256);
        let (status, body) = self
            .execute(self.request(Method::HEAD, &path, &[]), &path)
            .await?;
        if status == 404 {
            return Ok(false);
        }
        check(status, &body, &path)?;
        Ok(true)
    }

    /// `GET /aoide/images/{sha256}`. `None` when the server does not hold them.
    pub async fn get_image(&self, sha256: &str) -> Result<Option<Vec<u8>>, SidecarError> {
        let path = image_path(sha256);
        let (status, body) = self
            .execute(self.request(Method::GET, &path, &[]), &path)
            .await?;
        if status == 404 {
            return Ok(None);
        }
        check(status, &body, &path)?;
        Ok(Some(body))
    }

    /// `PUT /aoide/images/{sha256}` with the raw bytes. JPEG, PNG or WebP, up to 5 MB;
    /// the server recomputes the hash and refuses a mismatch. Idempotent.
    pub async fn put_image(
        &self,
        sha256: &str,
        bytes: Vec<u8>,
        mime: &str,
    ) -> Result<(), SidecarError> {
        let path = image_path(sha256);
        let request = self
            .request(Method::PUT, &path, &[])
            .header(CONTENT_TYPE, mime)
            .body(bytes);
        let (status, body) = self.execute(request, &path).await?;
        check(status, &body, &path)
    }

    // MARK: - Handoff

    /// `GET /aoide/queue`: one entry per device, most recently updated first by the
    /// server's clock. Entries this build cannot read are skipped, not fatal.
    pub async fn queues(&self) -> Result<Vec<QueueEntry>, SidecarError> {
        let body: serde_json::Value = self.get_json("aoide/queue", &[]).await?;
        Ok(list_from(&body, &["entries", "queues"])
            .iter()
            .filter_map(QueueEntry::from_json)
            .collect())
    }

    // MARK: - Collaborative playlists

    /// `GET /aoide/shares`: shares owned by, and granted to, the caller. Entries this
    /// build cannot read are skipped.
    pub async fn list_shares(&self) -> Result<Vec<PlaylistShare>, SidecarError> {
        let body: serde_json::Value = self.get_json("aoide/shares", &[]).await?;
        Ok(list_from(&body, &["shares"])
            .iter()
            .filter_map(|entry| serde_json::from_value(entry.clone()).ok())
            .collect())
    }

    /// `POST /aoide/shares`: invites another Jellyfin user to a playlist this user
    /// owns. Their edits and this user's then both travel through the ordinary pull.
    pub async fn share_playlist(
        &self,
        playlist_id: &str,
        grantee_user_id: &str,
        can_edit: bool,
    ) -> Result<(), SidecarError> {
        let path = "aoide/shares";
        let request = ShareRequest {
            playlist_id: playlist_id.to_owned(),
            grantee_user_id: grantee_user_id.to_owned(),
            can_edit,
        };
        let body = serde_json::to_vec(&request).map_err(|error| decode_error(path, &error))?;
        let request = self
            .request(Method::POST, path, &[])
            .header(CONTENT_TYPE, "application/json")
            .body(body);
        let (status, body) = self.execute(request, path).await?;
        check(status, &body, path)
    }

    /// `DELETE /aoide/shares/{playlistId}/{granteeUserId}`. Either side may revoke.
    /// Revoking stops future changes reaching the collaborator; it withdraws nothing
    /// they already wrote.
    pub async fn revoke_share(
        &self,
        playlist_id: &str,
        grantee_user_id: &str,
    ) -> Result<(), SidecarError> {
        let path = "aoide/shares";
        let mut url = self.url(path, &[]);
        if let Ok(mut segments) = url.path_segments_mut() {
            // Each id is one segment, escaped, whatever it contains.
            segments.push(playlist_id).push(grantee_user_id);
        }
        let (status, body) = self
            .execute(self.http.request(Method::DELETE, url), path)
            .await?;
        check(status, &body, path)
    }

    // MARK: - Infinity and Discover

    /// `POST /aoide/next`. A 404 here is an older sidecar — or a seed this user cannot
    /// see; the caller tells them apart by whether the seed is a track it just played.
    pub async fn next(&self, request: &NextRequest) -> Result<NextResponse, SidecarError> {
        self.send_json(Method::POST, "aoide/next", &[], request)
            .await
    }

    /// `GET /aoide/discover`. `limit` is held to 1…100. A 404 is an older sidecar: hide
    /// the tile.
    pub async fn discover(&self, limit: u32) -> Result<DiscoverResponse, SidecarError> {
        let limit = limit.clamp(1, DiscoverResponse::MAX_LIMIT);
        self.get_json("aoide/discover", &[("limit", limit.to_string())])
            .await
    }

    // MARK: - Measurements

    /// `GET /aoide/sound-bounds`, in calls of at most 200 ids, merged. A 404 on the
    /// endpoint is "trim nothing".
    pub async fn sound_bounds(&self, ids: &[String]) -> Result<SoundBoundsResponse, SidecarError> {
        let mut merged = SoundBoundsResponse::default();
        for chunk in ids.chunks(MAX_IDS_PER_CALL) {
            let page: SoundBoundsResponse = self
                .get_json("aoide/sound-bounds", &[("ids", chunk.join(","))])
                .await?;
            merged.absorb(page);
        }
        Ok(merged)
    }

    /// `POST /aoide/sound-bounds`: this device's own measurements, `None` meaning
    /// measured with nothing to trim.
    pub async fn post_sound_bounds(
        &self,
        bounds: &BTreeMap<String, Option<SoundBounds>>,
    ) -> Result<(), SidecarError> {
        #[derive(Serialize)]
        struct Body<'a> {
            bounds: &'a BTreeMap<String, Option<SoundBounds>>,
        }
        let path = "aoide/sound-bounds";
        let body =
            serde_json::to_vec(&Body { bounds }).map_err(|error| decode_error(path, &error))?;
        let request = self
            .request(Method::POST, path, &[])
            .header(CONTENT_TYPE, "application/json")
            .body(body);
        let (status, body) = self.execute(request, path).await?;
        check(status, &body, path)
    }

    /// `GET /aoide/audio-analysis`, in calls of at most 200 ids, merged.
    pub async fn audio_analysis(
        &self,
        ids: &[String],
    ) -> Result<AudioAnalysisResponse, SidecarError> {
        let mut merged = AudioAnalysisResponse::default();
        for chunk in ids.chunks(MAX_IDS_PER_CALL) {
            let page: AudioAnalysisResponse = self
                .get_json("aoide/audio-analysis", &[("ids", chunk.join(","))])
                .await?;
            merged.absorb(page);
        }
        Ok(merged)
    }

    /// `GET /aoide/analysis/coverage`.
    pub async fn analysis_coverage(&self) -> Result<AnalysisCoverage, SidecarError> {
        self.get_json("aoide/analysis/coverage", &[]).await
    }

    // MARK: - Plumbing

    fn request(&self, method: Method, path: &str, query: &[(&str, String)]) -> RequestBuilder {
        self.http.request(method, self.url(path, query))
    }

    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, SidecarError> {
        let (status, body) = self
            .execute(self.request(Method::GET, path, query), path)
            .await?;
        check(status, &body, path)?;
        decode(&body, path)
    }

    /// Compact JSON, which is load-bearing: the 256 KB payload ceiling is measured on
    /// compact text, and the store checked every op against it in that form.
    pub(crate) async fn send_json<B: Serialize, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: &B,
    ) -> Result<T, SidecarError> {
        let body = serde_json::to_vec(body).map_err(|error| decode_error(path, &error))?;
        let request = self
            .request(method, path, query)
            .header(CONTENT_TYPE, "application/json")
            .body(body);
        let (status, body) = self.execute(request, path).await?;
        check(status, &body, path)?;
        decode(&body, path)
    }

    async fn execute(
        &self,
        request: RequestBuilder,
        path: &str,
    ) -> Result<(u16, Vec<u8>), SidecarError> {
        let response = request
            .send()
            .await
            .map_err(|error| network(path, &error))?;
        let status = response.status().as_u16();
        let body = response
            .bytes()
            .await
            .map_err(|error| network(path, &error))?;
        Ok((status, body.to_vec()))
    }
}

fn image_path(sha256: &str) -> String {
    format!("aoide/images/{sha256}")
}

/// Turns a status into this crate's vocabulary.
fn check(status: u16, body: &[u8], path: &str) -> Result<(), SidecarError> {
    match status {
        200..=299 => Ok(()),
        401 => Err(SidecarError::Unauthorized),
        403 => Err(SidecarError::Forbidden),
        // The endpoint itself is missing: an older sidecar, or a proxy answering the
        // path. One is "your server can't do this yet", never "try again".
        404 | 405 | 501 => Err(SidecarError::Absent {
            path: path.to_owned(),
        }),
        _ => {
            let text = String::from_utf8_lossy(body);
            let trimmed = text.trim();
            let detail: String = trimmed.chars().take(DETAIL_LIMIT).collect();
            Err(SidecarError::Status {
                status,
                path: path.to_owned(),
                detail: (!detail.is_empty()).then_some(detail),
            })
        }
    }
}

fn decode<T: DeserializeOwned>(body: &[u8], path: &str) -> Result<T, SidecarError> {
    serde_json::from_slice(body).map_err(|error| decode_error(path, &error))
}

fn decode_error(path: &str, error: &serde_json::Error) -> SidecarError {
    SidecarError::Decode {
        path: path.to_owned(),
        detail: error.to_string(),
    }
}

fn network(path: &str, error: &reqwest::Error) -> SidecarError {
    SidecarError::Network(format!("{path}: {error}"))
}

/// Redirects are followed only within the server's own origin (scheme, host
/// and port), at most ten: the token rides in `X-Emby-Authorization` (a default header),
/// which reqwest doesn't strip when a redirect leaves for another host or
/// drops to plain HTTP, as it does `Authorization`. A redirect elsewhere is
/// returned as the redirect response, an error to the caller.
fn same_origin_redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let same = attempt
            .previous()
            .first()
            .is_some_and(|first| first.origin() == attempt.url().origin());
        if !same {
            attempt.stop()
        } else if attempt.previous().len() > 10 {
            attempt.error("too many redirects")
        } else {
            attempt.follow()
        }
    })
}
