/// Everything a call to the server can fail with.
///
/// `Clone` and `Eq` so it can travel inside UI messages and be compared in tests;
/// that is why transport failures carry a message rather than the reqwest error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// What was typed can't be turned into an http(s) address.
    #[error("\"{0}\" isn't a valid server address")]
    MalformedAddress(String),
    /// Something answered at the address, but not the way Melete answers
    /// `/api/v1/health`: a router page, a proxy 404, another service.
    #[error("{0} responded, but doesn't look like a Melete server")]
    NotMelete(String),
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
        /// What went wrong: Melete's `{"error": …}` message, or else a short
        /// prefix of the body (a proxy's error page runs to kilobytes).
        detail: Option<String>,
    },
    /// A 2xx response whose body didn't decode.
    #[error("couldn't read the server's answer to {path}: {message}")]
    Decoding {
        /// The request path, relative to the server address.
        path: String,
        /// What didn't fit: serde_json's message, or that the answer was larger
        /// than any Melete answer.
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
        _ => Err(Error::Http {
            status,
            path: path.to_owned(),
            detail: detail(body),
        }),
    }
}

/// Every Melete error is `{"error": "…"}`; anything else in front of it (a proxy)
/// is kept as a prefix of its body.
fn detail(body: &[u8]) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Shape {
        error: String,
    }
    let text = match serde_json::from_slice::<Shape>(body) {
        Ok(shape) => shape.error,
        Err(_) => String::from_utf8_lossy(body).into_owned(),
    };
    let cut = text.floor_char_boundary(MAX_DETAIL_BYTES);
    let detail = text[..cut].trim();
    (!detail.is_empty()).then(|| detail.to_owned())
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
        assert_eq!(check_status(200, "api/v1/music/albums", b"{}"), Ok(()));
        assert_eq!(check_status(204, "api/v1/auth/logout", b""), Ok(()));
    }

    #[test]
    fn unauthorized_is_typed() {
        assert_eq!(
            check_status(401, "api/v1/auth/login", br#"{"error":"wrong password"}"#),
            Err(Error::Unauthorized)
        );
    }

    #[test]
    fn not_found_names_the_path() {
        assert_eq!(
            check_status(404, "api/v1/music/tracks/7/lyrics", b""),
            Err(Error::NotFound {
                path: "api/v1/music/tracks/7/lyrics".into()
            })
        );
    }

    #[test]
    fn other_statuses_carry_a_short_body_prefix() {
        let body = format!("<html>Bad Gateway\n{}", "x".repeat(5_000));
        let Err(Error::Http {
            status,
            path,
            detail,
        }) = check_status(500, "api/v1/music/albums", body.as_bytes())
        else {
            panic!("expected Error::Http");
        };
        assert_eq!(status, 500);
        assert_eq!(path, "api/v1/music/albums");
        let detail = detail.expect("a detail");
        assert!(detail.starts_with("<html>Bad Gateway"));
        assert!(detail.len() <= MAX_DETAIL_BYTES);
    }

    #[test]
    fn melete_error_bodies_give_their_message() {
        assert_eq!(
            check_status(
                409,
                "api/v1/admin/system/tasks/library/run",
                br#"{"error":"the library task is running already"}"#
            ),
            Err(Error::Http {
                status: 409,
                path: "api/v1/admin/system/tasks/library/run".into(),
                detail: Some("the library task is running already".into()),
            })
        );
    }

    #[test]
    fn forbidden_is_a_plain_http_error() {
        assert!(matches!(
            check_status(403, "api/v1/music/albums", b""),
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
        let Err(Error::Http { detail, .. }) =
            check_status(502, "api/v1/music/albums", body.as_bytes())
        else {
            panic!("expected Error::Http");
        };
        let detail = detail.expect("a detail");
        assert_eq!(detail, "a".repeat(399));
    }

    #[test]
    fn a_blank_body_has_no_detail() {
        assert_eq!(
            check_status(503, "api/v1/music/albums", b"  \n"),
            Err(Error::Http {
                status: 503,
                path: "api/v1/music/albums".into(),
                detail: None
            })
        );
    }

    #[test]
    fn http_errors_read_well() {
        let with = Error::Http {
            status: 500,
            path: "api/v1/music/albums".into(),
            detail: Some("boom".into()),
        };
        assert_eq!(
            with.to_string(),
            "api/v1/music/albums returned an error (HTTP 500): boom"
        );
        let without = Error::Http {
            status: 500,
            path: "api/v1/music/albums".into(),
            detail: None,
        };
        assert_eq!(
            without.to_string(),
            "api/v1/music/albums returned an error (HTTP 500)"
        );
    }
}
