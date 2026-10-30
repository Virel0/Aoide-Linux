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

/// What a request carries.
enum Body<'a> {
    Empty,
    Json(&'a serde_json::Value),
}

/// Redirects are followed only within the server's own origin (scheme, host
/// and port), at most ten. reqwest drops `Authorization` when a redirect leaves
/// the host, but not when one drops from https to plain http on the same host;
/// staying within the origin covers both. A redirect elsewhere is returned as
/// the redirect response, an error to the caller.
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

/// Most an answer may be: a large library's page of tracks is a few MB.
const MAX_BODY: usize = 64 << 20;

/// The body, or `None` once it passes `limit` bytes (by its declared length,
/// or while reading): a broken or hostile server can't fill memory.
pub(crate) async fn read_capped(
    mut response: reqwest::Response,
    limit: usize,
) -> std::result::Result<Option<Vec<u8>>, reqwest::Error> {
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Ok(None);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > limit {
            return Ok(None);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Some(body))
}

/// One Melete server, optionally signed in.
///
/// Cheap to clone: clones share one connection pool. Signing in makes a new value
/// with [`with_credentials`](Self::with_credentials) rather than mutating, so a
/// request in flight never sees its auth change underneath it.
///
/// # Errors
///
/// Every request can fail the same ways; each method's own `# Errors` names
/// what it adds to these:
///
/// - [`Error::Transport`]: no HTTP answer (DNS, a refused connection, TLS, a
///   timeout).
/// - [`Error::Unauthorized`]: HTTP 401, a token the server has revoked, or none
///   where one is needed.
/// - [`Error::NotFound`]: HTTP 404, usually an id the server doesn't have.
/// - [`Error::Http`]: any other non-2xx status, such as 403 for a user who isn't
///   an administrator.
/// - [`Error::Decoding`]: a 2xx answer that isn't the JSON expected, or one over
///   64 MB.
#[derive(Debug, Clone)]
pub struct MeleteClient {
    http: reqwest::Client,
    address: ServerAddress,
    device: Device,
    credentials: Option<Credentials>,
}

impl MeleteClient {
    /// A signed-out client. Fails only if the TLS backend can't initialise.
    ///
    /// # Errors
    ///
    /// [`Error::Transport`] when the HTTP client can't be built. Sends nothing.
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

    /// The server this client talks to.
    pub fn address(&self) -> &ServerAddress {
        &self.address
    }

    /// This install: its name is what sign-in shows in the server's device list,
    /// its id what sync and the event stream name it by.
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// The signed-in user, or `None` for a signed-out client.
    pub fn credentials(&self) -> Option<&Credentials> {
        self.credentials.as_ref()
    }

    pub(crate) fn url(&self, segments: &[&str], query: &[(&str, String)]) -> reqwest::Url {
        self.address.join(segments, query)
    }

    /// Sends one request and returns the body of a 2xx answer, with the token as
    /// `Authorization: Bearer …` when signed in.
    pub(crate) async fn send(
        &self,
        method: Method,
        segments: &[&str],
        query: &[(&str, String)],
        body: Option<&serde_json::Value>,
    ) -> Result<Vec<u8>> {
        self.send_with_status(method, segments, query, body)
            .await
            .map(|(_, bytes)| bytes)
    }

    /// [`send`](Self::send), with the 2xx status too, for the calls that answer
    /// differently by it (Quick Connect's 202 while it waits).
    pub(crate) async fn send_with_status(
        &self,
        method: Method,
        segments: &[&str],
        query: &[(&str, String)],
        body: Option<&serde_json::Value>,
    ) -> Result<(u16, Vec<u8>)> {
        let body = match body {
            Some(json) => Body::Json(json),
            None => Body::Empty,
        };
        self.send_body(method, segments, query, body).await
    }

    async fn send_body(
        &self,
        method: Method,
        segments: &[&str],
        query: &[(&str, String)],
        body: Body<'_>,
    ) -> Result<(u16, Vec<u8>)> {
        let has_body_slot = method != Method::GET;
        let mut request = self
            .http
            .request(method, self.url(segments, query))
            .header(ACCEPT, "application/json");
        if let Some(credentials) = &self.credentials {
            request = request.header(
                AUTHORIZATION,
                format!("Bearer {}", credentials.access_token),
            );
        }
        request = match body {
            Body::Json(body) => request.json(body),
            // `Content-Length: 0` spelled out, since hyper leaves it off an empty
            // body and some proxies answer a POST without one with 411.
            Body::Empty if has_body_slot => request.header(CONTENT_LENGTH, "0").body(Vec::new()),
            Body::Empty => request,
        };

        let response = request.send().await.map_err(transport)?;
        let status = response.status().as_u16();
        let bytes = read_capped(response, MAX_BODY)
            .await
            .map_err(transport)?
            .ok_or_else(|| Error::Decoding {
                path: segments.join("/"),
                message: "the answer is larger than any Melete answer".to_owned(),
            })?;
        check_status(status, &segments.join("/"), &bytes)?;
        Ok((status, bytes))
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
}

#[cfg(test)]
mod capped_tests {
    use super::read_capped;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn a_body_past_the_limit_is_refused() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![7u8; 100]))
            .mount(&server)
            .await;
        let _ = rustls::crypto::ring::default_provider().install_default();
        let get = || async { reqwest::get(server.uri()).await.unwrap() };
        assert_eq!(
            read_capped(get().await, 100).await.unwrap(),
            Some(vec![7u8; 100])
        );
        assert_eq!(read_capped(get().await, 99).await.unwrap(), None);
    }
}
