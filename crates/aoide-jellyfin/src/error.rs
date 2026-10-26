/// Everything a call to the server can fail with.
///
/// `Clone` and `Eq` so it can travel inside UI messages and be compared in tests;
/// that is why transport failures carry a message rather than the reqwest error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// What was typed can't be turned into an http(s) address.
    #[error("\"{0}\" isn't a valid server address")]
    MalformedAddress(String),
    /// Something answered at the address, but not the way Jellyfin answers
    /// `/System/Info/Public`: a router page, a proxy 404, another service.
    #[error("{0} responded, but doesn't look like a Jellyfin server")]
    NotJellyfin(String),
    /// The call needs a signed-in user and the client has no credentials.
    #[error("not signed in")]
    NotSignedIn,
    /// HTTP 401: a wrong password at sign-in, or a token the server has revoked.
    #[error("the server didn't accept the credentials (HTTP 401)")]
    Unauthorized,
    /// HTTP 404.
    #[error("the server has no {path}")]
    NotFound {
        /// The request path, relative to the server address.
        path: String,
    },
    /// Any other non-2xx status.
    #[error(
        "{path} returned an error (HTTP {status}){}",
        .detail.as_deref().map(|d| format!(": {d}")).unwrap_or_default()
    )]
    Http {
        /// The HTTP status: any outside 200–299 but 401 and 404.
        status: u16,
        /// The request path, relative to the server address.
        path: String,
        /// A short prefix of the response body. An ASP.NET error page runs to
        /// kilobytes, and its first line is the part that names the fault.
        detail: Option<String>,
    },
    /// A 2xx response whose body didn't decode.
    #[error("couldn't read the server's answer to {path}: {message}")]
    Decoding {
        /// The request path, relative to the server address.
        path: String,
        /// What didn't fit: serde_json's message, or that the answer was larger
        /// than any Jellyfin answer.
        message: String,
    },
    /// The request never got an HTTP answer: DNS, refused connection, TLS, timeout.
    #[error("{0}")]
    Transport(String),
}

/// The longest response-body prefix kept in [`Error::Http`].
const MAX_DETAIL_BYTES: usize = 400;

/// Maps a status and body to `Ok` for 2xx, or the matching error.
pub(crate) fn check_status(status: u16, path: &str, body: &[u8]) -> Result<(), Error> {
    match status {
        200..=299 => Ok(()),
        401 => Err(Error::Unauthorized),
        404 => Err(Error::NotFound {
            path: path.to_owned(),
        }),
        _ => {
            let text = String::from_utf8_lossy(body);
            let cut = text.floor_char_boundary(MAX_DETAIL_BYTES);
            let detail = text[..cut].trim();
            Err(Error::Http {
                status,
                path: path.to_owned(),
                detail: (!detail.is_empty()).then(|| detail.to_owned()),
            })
        }
    }
}

/// A reqwest failure as a transport error, with its cause chain spelled out
/// (reqwest's own message is just "error sending request") and the URL left out,
/// because URLs carry search terms and, for media, the token.
pub(crate) fn transport(error: reqwest::Error) -> Error {
    let error = error.without_url();
    let mut message = error.to_string();
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !message.contains(&cause_text) {
            message.push_str(": ");
            message.push_str(&cause_text);
        }
        source = cause.source();
    }
    Error::Transport(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_statuses_pass() {
        assert_eq!(check_status(200, "Items", b"{}"), Ok(()));
        assert_eq!(check_status(204, "Sessions/Logout", b""), Ok(()));
    }

    #[test]
    fn unauthorized_is_typed() {
        assert_eq!(
            check_status(
                401,
                "Users/AuthenticateByName",
                b"Error processing request."
            ),
            Err(Error::Unauthorized)
        );
    }

    #[test]
    fn not_found_names_the_path() {
        assert_eq!(
            check_status(404, "Audio/abc/Lyrics", b""),
            Err(Error::NotFound {
                path: "Audio/abc/Lyrics".into()
            })
        );
    }

    #[test]
    fn other_statuses_carry_a_short_body_prefix() {
        let body = format!("System.NullReferenceException: boom\n{}", "x".repeat(5_000));
        let Err(Error::Http {
            status,
            path,
            detail,
        }) = check_status(500, "Items", body.as_bytes())
        else {
            panic!("expected Error::Http");
        };
        assert_eq!(status, 500);
        assert_eq!(path, "Items");
        let detail = detail.expect("a detail");
        assert!(detail.starts_with("System.NullReferenceException: boom"));
        assert!(detail.len() <= MAX_DETAIL_BYTES);
    }

    #[test]
    fn forbidden_is_a_plain_http_error() {
        assert!(matches!(
            check_status(403, "Items", b""),
            Err(Error::Http {
                status: 403,
                detail: None,
                ..
            })
        ));
    }

    #[test]
    fn the_prefix_never_splits_a_character() {
        // 399 ASCII bytes then a 2-byte character straddling the cut.
        let body = format!("{}é tail", "a".repeat(399));
        let Err(Error::Http { detail, .. }) = check_status(502, "Items", body.as_bytes()) else {
            panic!("expected Error::Http");
        };
        let detail = detail.expect("a detail");
        assert_eq!(detail, "a".repeat(399));
    }

    #[test]
    fn a_blank_body_has_no_detail() {
        assert_eq!(
            check_status(503, "Items", b"  \n"),
            Err(Error::Http {
                status: 503,
                path: "Items".into(),
                detail: None
            })
        );
    }

    #[test]
    fn http_errors_read_well() {
        let with = Error::Http {
            status: 500,
            path: "Items".into(),
            detail: Some("boom".into()),
        };
        assert_eq!(with.to_string(), "Items returned an error (HTTP 500): boom");
        let without = Error::Http {
            status: 500,
            path: "Items".into(),
            detail: None,
        };
        assert_eq!(without.to_string(), "Items returned an error (HTTP 500)");
    }
}
