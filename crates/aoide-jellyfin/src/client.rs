use std::sync::Once;
use std::time::Duration;

use reqwest::Method;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_LENGTH};
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
            Some(body) => request.json(body),
            // `Content-Length: 0` spelled out, since hyper leaves it off an empty
            // body and some proxies answer a POST without one with 411.
            None if has_body_slot => request.header(CONTENT_LENGTH, "0").body(Vec::new()),
            None => request,
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
