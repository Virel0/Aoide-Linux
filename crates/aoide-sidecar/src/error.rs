/// Everything a call to the sidecar can fail with.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SidecarError {
    /// The endpoint itself is not there: 404, or a 405/501 from a proxy answering the
    /// path. An older sidecar, or none — fall back rather than retry.
    #[error("the server has no {path}")]
    Absent { path: String },
    #[error("the token was not accepted")]
    Unauthorized,
    #[error("this account is not allowed to do that")]
    Forbidden,
    /// Any other status. `detail` is the start of the body, which for a sidecar
    /// failure is usually the whole diagnosis.
    #[error("{path} returned an error (HTTP {status}){}", detail.as_deref().map(|d| format!(": {d}")).unwrap_or_default())]
    Status {
        status: u16,
        path: String,
        detail: Option<String>,
    },
    /// The round trip never completed: no network, a timeout, a dropped connection.
    #[error("network: {0}")]
    Network(String),
    #[error("{path} sent a response this client could not read: {detail}")]
    Decode { path: String, detail: String },
    #[error("not a usable server address: {0}")]
    InvalidUrl(String),
    /// The transport does not do this at all (a test double without images, say).
    #[error("this transport does not support {0}")]
    Unsupported(&'static str),
}

impl SidecarError {
    /// Whether this means the sidecar (or this endpoint) is missing, as opposed to a
    /// failure worth trying again.
    pub fn is_absent(&self) -> bool {
        matches!(self, Self::Absent { .. })
    }

    /// The server failed on what it was sent, rather than refusing it or being
    /// unreachable — the case where one op in a batch may be poisoning it.
    ///
    /// Not 503: the contract uses it for "storage unavailable", where the ops are
    /// fine and must stay queued, not be isolated and quarantined. Not 502 or 504
    /// either: those are a gateway saying the server could not be reached, and every
    /// op would reproduce them alone.
    pub fn is_server_fault(&self) -> bool {
        matches!(self, Self::Status { status, .. } if (500..=599).contains(status) && !matches!(status, 502..=504))
    }

    /// What the server itself said, for a person to read: a `ProblemDetails`
    /// body's `title` and `detail` (the sidecar's 503 names the real cause there),
    /// or the start of any other body. `None` when the server said nothing.
    pub fn server_text(&self) -> Option<String> {
        let Self::Status {
            detail: Some(detail),
            ..
        } = self
        else {
            return None;
        };
        let problem = serde_json::from_str::<serde_json::Value>(detail)
            .ok()
            .filter(serde_json::Value::is_object);
        let Some(problem) = problem else {
            return Some(detail.clone());
        };
        let field = |key: &str| problem.get(key).and_then(serde_json::Value::as_str);
        match (field("title"), field("detail")) {
            (Some(title), Some(detail)) => Some(format!("{title}: {detail}")),
            (Some(only), None) | (None, Some(only)) => Some(only.to_owned()),
            (None, None) => Some(detail.clone()),
        }
    }

    /// The HTTP status, when the server answered with one.
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Status { status, .. } => Some(*status),
            Self::Unauthorized => Some(401),
            Self::Forbidden => Some(403),
            _ => None,
        }
    }
}
