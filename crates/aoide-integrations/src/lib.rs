//! Aoide's integrations with services outside the music server, each
//! independent of the others and of the UI, and the app's half of the
//! server's music AI:
//!
//! - [`discord`]: Discord Rich Presence over Discord's local socket.
//! - [`music_ai`]: whether a phrase is worth a search by description, and a
//!   mix's rules checked and resolved; Melete's AI turns the words into
//!   filters and rules.
//! - [`translate`]: whether lyrics are worth translating, worked out on this
//!   machine; the server translates them.
//! - [`update`]: whether a newer release is out on GitHub, at most daily.
//!
//! Nothing here knows about iced or the other Aoide crates. Async parts run
//! on the caller's tokio runtime.
//!
//! # Privacy
//!
//! Every module says exactly what it sends and where. None of them sends
//! anything until the app turns it on, and none sends an identifier of the
//! listener or the machine.

#![warn(missing_docs)]
#![warn(clippy::missing_errors_doc)]

pub mod discord;
mod links;
pub mod music_ai;
pub mod translate;
pub mod update;

/// rustls with the ring provider: reqwest is built without one. An app that
/// installed its own first keeps it.
fn install_crypto() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}
