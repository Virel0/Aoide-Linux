//! Asking the server: each answer timed against this device's clock, and each
//! failure put the way the rest of the module acts on it.

use aoide_melete::Listening;

/// An answer from the server, with this device's clock halfway through asking.
#[derive(Debug, Clone)]
pub struct Timed {
    pub listening: Listening,
    pub local_mid: i64,
}

/// Why a request came to nothing.
#[derive(Debug, Clone)]
pub enum Failed {
    /// No such session: it ended, or this device is no longer in it.
    Ended,
    /// Made against an old version.
    Stale,
    /// The token was refused.
    SignedOut,
    /// Anything else, in words.
    Other(String),
}

/// Runs a request, timing it against this device's clock.
pub(super) async fn timed<F>(request: F) -> Result<Timed, Failed>
where
    F: Future<Output = aoide_melete::Result<Listening>>,
{
    let sent = aoide_store::now_millis();
    let listening = request.await.map_err(failed)?;
    let received = aoide_store::now_millis();
    Ok(Timed {
        listening,
        local_mid: sent + (received - sent) / 2,
    })
}

pub(super) fn failed(error: aoide_melete::Error) -> Failed {
    use aoide_melete::Error as E;
    match error {
        E::NotFound { .. } => Failed::Ended,
        E::Unauthorized => Failed::SignedOut,
        error if aoide_melete::is_stale(&error) => Failed::Stale,
        E::Http { status: 429, .. } => {
            Failed::Other("Too many wrong codes. Try again in 15 minutes.".into())
        }
        E::Http { status: 403, .. } => Failed::Other(
            "Listening together isn't open to you, as you can see only some music libraries."
                .into(),
        ),
        error => Failed::Other(crate::failure::describe(&error).to_string()),
    }
}
