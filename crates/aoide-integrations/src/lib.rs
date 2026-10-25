//! Aoide's integrations with services outside the music server, each
//! independent of the others and of the UI:
//!
//! - [`discord`]: Discord Rich Presence over Discord's local socket.
//! - [`openrouter`]: search by description and mixes from a mood, through
//!   OpenRouter; the model only translates words into filters and rules.
//! - [`translate`]: lyric translation. Language detection always; with the
//!   `local-translation` feature, translation on this machine with models
//!   downloaded on first use.
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

pub mod discord;
mod links;
pub mod openrouter;
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
