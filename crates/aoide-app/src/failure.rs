//! A failed request in words: one plain sentence to show, and the
//! technical text (the transport's own message, the HTTP status) kept apart
//! for the "Details" disclosure under it.

use std::fmt;
use std::sync::RwLock;

use aoide_melete::Error;

/// Something that failed, as the listener reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// What happened and what to do, in a sentence or two.
    pub summary: String,
    /// The technical text behind it, if there is any worth showing.
    pub details: Option<String>,
}

impl Failure {
    pub fn new(summary: impl Into<String>, details: Option<String>) -> Self {
        Self {
            summary: summary.into(),
            details,
        }
    }
}

/// Words that are already plain (the local store's, the app's own) have no
/// details.
impl From<String> for Failure {
    fn from(summary: String) -> Self {
        Self::new(summary, None)
    }
}

impl From<&str> for Failure {
    fn from(summary: &str) -> Self {
        Self::new(summary, None)
    }
}

/// The summary: what a notice or a line of text shows.
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.summary)
    }
}

/// The server this session is signed in to, as its errors name it. Set on
/// sign-in and cleared on sign-out; only ever read to word an error, so a
/// request answered after a sign-out at worst names no server.
static SERVER: RwLock<Option<String>> = RwLock::new(None);

pub fn set_server(server: Option<&str>) {
    if let Ok(mut current) = SERVER.write() {
        *current = server.map(shown_address);
    }
}

/// An address as people type it: no scheme, no trailing slash.
fn shown_address(server: &str) -> String {
    let server = server.trim();
    let bare = server
        .strip_prefix("https://")
        .or_else(|| server.strip_prefix("http://"))
        .unwrap_or(server);
    bare.trim_end_matches('/').to_owned()
}

/// An `/aoide/*` error in words, naming the signed-in server.
pub fn sidecar(error: &aoide_sidecar::SidecarError) -> Failure {
    let server = SERVER.read().ok().and_then(|s| s.clone());
    sidecar_at(error, server.as_deref())
}

pub fn sidecar_at(error: &aoide_sidecar::SidecarError, server: Option<&str>) -> Failure {
    use aoide_sidecar::SidecarError as E;
    match error {
        E::Network(message) => describe_at(&Error::Transport(message.clone()), server),
        E::Status { status, path, .. } => Failure::new(
            describe_at(
                &Error::Http {
                    status: *status,
                    path: path.clone(),
                    detail: None,
                },
                server,
            )
            .summary,
            Some(error.to_string()),
        ),
        E::Decode { .. } => Failure::new(
            "Your server sent an answer Aoide couldn't read.",
            Some(error.to_string()),
        ),
        E::Absent { .. } => Failure::new("Your server can't do this.", Some(error.to_string())),
        E::Unauthorized => "Your server didn't accept this sign-in.".into(),
        E::Forbidden => "Your server doesn't allow this account to do that.".into(),
        E::InvalidUrl(_) | E::Unsupported(_) => Failure::new(
            "Aoide couldn't talk to your server.",
            Some(error.to_string()),
        ),
    }
}

/// OpenRouter's error in words; its own text (the transport's, or what it
/// said when it refused) goes to the details.
pub fn openrouter(error: &aoide_integrations::openrouter::OpenRouterError) -> Failure {
    use aoide_integrations::openrouter::OpenRouterError as E;
    let summary = match error {
        E::Unreachable(_) => "Can't reach OpenRouter. Check this computer is online.",
        E::Timeout => "OpenRouter didn't answer in time. Try again in a moment.",
        E::Refused { .. } => "OpenRouter turned the request down. Check your key and credit.",
        E::UnfamiliarReply => "OpenRouter sent an answer Aoide couldn't read.",
    };
    Failure::new(summary, Some(error.to_string()))
}

/// A search or mix by description: the server's error, or OpenRouter's
/// already in words.
impl From<crate::smart::Failure> for Failure {
    fn from(failure: crate::smart::Failure) -> Self {
        match failure {
            crate::smart::Failure::Server(error) => describe(&error),
            crate::smart::Failure::Other(words) => words.into(),
        }
    }
}

/// A Melete error in words, naming the signed-in server.
pub fn describe(error: &Error) -> Failure {
    let server = SERVER.read().ok().and_then(|s| s.clone());
    describe_at(error, server.as_deref())
}

/// A Melete error in words, naming `server` when there is one.
pub fn describe_at(error: &Error, server: Option<&str>) -> Failure {
    match error {
        Error::Transport(message) => Failure::new(
            match server {
                Some(server) => format!(
                    "Can't reach your server at {server}. Check it's running and on the network."
                ),
                None => "Can't reach your server. Check it's running and on the network.".into(),
            },
            Some(message.clone()),
        ),
        Error::Http { status, .. } => Failure::new(
            match status {
                403 => "Your server doesn't allow this account to do that.",
                500..=599 => {
                    "Your server ran into a problem answering. Try again, or look in its logs."
                }
                _ => "Your server turned the request down.",
            },
            Some(error.to_string()),
        ),
        Error::NotFound { .. } => Failure::new(
            "Your server doesn't have this any more. It may have been removed or rescanned.",
            Some(error.to_string()),
        ),
        Error::Decoding { .. } => Failure::new(
            "Your server sent an answer Aoide couldn't read.",
            Some(error.to_string()),
        ),
        Error::MalformedAddress(raw) => format!(
            "\u{201c}{}\u{201d} isn't a valid server address.",
            raw.trim()
        )
        .into(),
        Error::NotMelete(host) => {
            format!("{host} answered, but doesn't look like a Melete server.").into()
        }
        Error::Unauthorized => "Your server didn't accept this sign-in.".into(),
        Error::NotSignedIn => "You're not signed in.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unreachable_server_is_named_and_the_transport_text_kept_for_details() {
        let refused = Error::Transport(
            "error sending request: client error (Connect): tcp connect error: Connection \
             refused (os error 61)"
                .into(),
        );
        let failure = describe_at(&refused, Some("music.example.net:8096"));
        assert_eq!(
            failure.summary,
            "Can't reach your server at music.example.net:8096. Check it's running and on the \
             network."
        );
        assert!(!failure.summary.contains("os error"));
        assert!(failure.details.as_deref().unwrap().contains("os error 61"));
        assert_eq!(
            describe_at(&refused, None).summary,
            "Can't reach your server. Check it's running and on the network."
        );
    }

    #[test]
    fn http_failures_say_what_it_means_and_keep_the_status_for_details() {
        let failure = describe_at(
            &Error::Http {
                status: 502,
                path: "Items".into(),
                detail: Some("Bad Gateway".into()),
            },
            Some("music.example.net"),
        );
        assert!(
            failure
                .summary
                .starts_with("Your server ran into a problem")
        );
        assert!(!failure.summary.contains("502"));
        assert_eq!(
            failure.details.as_deref(),
            Some("Items returned an error (HTTP 502): Bad Gateway")
        );
        let forbidden = describe_at(
            &Error::Http {
                status: 403,
                path: "Items".into(),
                detail: None,
            },
            None,
        );
        assert_eq!(
            forbidden.summary,
            "Your server doesn't allow this account to do that."
        );
    }

    #[test]
    fn plain_failures_have_no_details() {
        let failure = describe_at(&Error::NotMelete("music.example.net".into()), None);
        assert_eq!(
            failure.summary,
            "music.example.net answered, but doesn't look like a Melete server."
        );
        assert_eq!(failure.details, None);
        assert_eq!(Failure::from("The library is closing.").details, None);
    }

    #[test]
    fn a_sidecar_network_failure_reads_like_the_server_one() {
        let failure = sidecar_at(
            &aoide_sidecar::SidecarError::Network("tcp connect error (os error 61)".into()),
            Some("music.example.net"),
        );
        assert!(
            failure
                .summary
                .starts_with("Can't reach your server at music.example.net.")
        );
        assert!(failure.details.as_deref().unwrap().contains("os error 61"));
    }

    #[test]
    fn the_address_is_shown_as_typed() {
        assert_eq!(
            shown_address("https://music.example.net/"),
            "music.example.net"
        );
        assert_eq!(shown_address("http://192.0.2.10:8096"), "192.0.2.10:8096");
        assert_eq!(shown_address("melete.local:8100"), "melete.local:8100");
    }
}
