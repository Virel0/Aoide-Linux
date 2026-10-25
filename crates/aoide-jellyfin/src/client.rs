use std::sync::Once;
use std::time::Duration;

use reqwest::Method;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE};
use serde::de::DeserializeOwned;

use crate::error::{check_status, transport};
use crate::{Credentials, Device, Error, Result, ServerAddress};

/// How long to wait for a connection. A server that's off answers nothing at all.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a response may go silent. An idle timeout rather than a total one, so a
/// slow server sending a big page isn't cut off mid-way.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// rustls with the ring provider: reqwest is built without one, so a client
/// can't be made until one is installed. An application that installed its own
/// first keeps it.
fn install_crypto() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// What a request carries.
pub(crate) enum Body<'a> {
    Empty,
    Json(&'a serde_json::Value),
    /// Bytes sent as they are, under their own content type.
    Raw {
        content_type: &'a str,
        bytes: Vec<u8>,
    },
}

/// Redirects are followed only within the server's own origin (scheme, host
/// and port), at most ten: the token rides in `X-Emby-Authorization`,
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

/// One Jellyfin server, optionally signed in.
///
/// Cheap to clone: clones share one connection pool. Signing in makes a new value
/// with [`with_credentials`](Self::with_credentials) rather than mutating, so a
/// request in flight never sees its auth change underneath it.
#[derive(Debug, Clone)]
pub struct JellyfinClient {
    http: reqwest::Client,
    address: ServerAddress,
    device: Device,
    credentials: Option<Credentials>,
}

impl JellyfinClient {
    /// A signed-out client. Fails only if the TLS backend can't initialise.
    pub fn new(address: ServerAddress, device: Device) -> Result<Self> {
        install_crypto();
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .redirect(same_origin_redirects())
            .build()
            .map_err(transport)?;
        Ok(Self {
            http,
            address,
            device,
            credentials: None,
        })
    }

    /// This client, signed in as `credentials`.
    pub fn with_credentials(self, credentials: Credentials) -> Self {
        Self {
            credentials: Some(credentials),
            ..self
        }
    }

    pub fn address(&self) -> &ServerAddress {
        &self.address
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    pub fn credentials(&self) -> Option<&Credentials> {
        self.credentials.as_ref()
    }

    pub(crate) fn require_credentials(&self) -> Result<&Credentials> {
        self.credentials.as_ref().ok_or(Error::NotSignedIn)
    }

    pub(crate) fn user_id(&self) -> Result<&str> {
        self.require_credentials().map(|c| c.user_id.as_str())
    }

    pub(crate) fn url(&self, segments: &[&str], query: &[(&str, String)]) -> reqwest::Url {
        self.address.join(segments, query)
    }

    /// Sends one request and returns the body of a 2xx answer.
    ///
    /// Every request carries the `Authorization` header, and the same value as
    /// `X-Emby-Authorization`, which is where Jellyfin 10.8 and older look.
    pub(crate) async fn send(
        &self,
        method: Method,
        segments: &[&str],
        query: &[(&str, String)],
        body: Option<&serde_json::Value>,
    ) -> Result<Vec<u8>> {
        let body = match body {
            Some(json) => Body::Json(json),
            None => Body::Empty,
        };
        self.send_body(method, segments, query, body).await
    }

    /// [`send`](Self::send) with any kind of body, such as an image upload.
    pub(crate) async fn send_body(
        &self,
        method: Method,
        segments: &[&str],
        query: &[(&str, String)],
        body: Body<'_>,
    ) -> Result<Vec<u8>> {
        let header = self
            .device
            .authorization_header(self.credentials.as_ref().map(|c| c.access_token.as_str()));
        let has_body_slot = method != Method::GET;
        let mut request = self
            .http
            .request(method, self.url(segments, query))
            .header(AUTHORIZATION, header.as_str())
            .header("X-Emby-Authorization", header.as_str())
            .header(ACCEPT, "application/json");
        request = match body {
            Body::Json(body) => request.json(body),
            Body::Raw {
                content_type,
                bytes,
            } => request.header(CONTENT_TYPE, content_type).body(bytes),
            // `Content-Length: 0` spelled out, since hyper leaves it off an empty
            // body and some proxies answer a POST without one with 411.
            Body::Empty if has_body_slot => request.header(CONTENT_LENGTH, "0").body(Vec::new()),
            Body::Empty => request,
        };

        let response = request.send().await.map_err(transport)?;
        let status = response.status().as_u16();
        let bytes = response.bytes().await.map_err(transport)?;
        check_status(status, &segments.join("/"), &bytes)?;
        Ok(bytes.to_vec())
    }

    pub(crate) async fn send_decoded<T: DeserializeOwned>(
        &self,
        method: Method,
        segments: &[&str],
        query: &[(&str, String)],
        body: Option<&serde_json::Value>,
    ) -> Result<T> {
        let bytes = self.send(method, segments, query, body).await?;
        serde_json::from_slice(&bytes).map_err(|e| Error::Decoding {
            path: segments.join("/"),
            message: e.to_string(),
        })
    }

    pub(crate) async fn get<T: DeserializeOwned>(
        &self,
        segments: &[&str],
        query: &[(&str, String)],
    ) -> Result<T> {
        self.send_decoded(Method::GET, segments, query, None).await
    }

    /// A request whose answer has no body worth reading (most are 204).
    pub(crate) async fn send_empty(
        &self,
        method: Method,
        segments: &[&str],
        query: &[(&str, String)],
    ) -> Result<()> {
        self.send(method, segments, query, None).await.map(drop)
    }

    pub(crate) async fn post_json(
        &self,
        segments: &[&str],
        body: &serde_json::Value,
    ) -> Result<()> {
        self.send(Method::POST, segments, &[], Some(body))
            .await
            .map(drop)
    }
}
