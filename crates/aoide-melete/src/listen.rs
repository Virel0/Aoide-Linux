//! Listening together: a session that several devices play in step, kept by
//! Melete (`/api/v1/music/sessions`). Each device plays its own stream; the
//! server holds the queue, which track, where in it and whether it plays, and
//! raises the version with every change. A change is made against the version
//! the device last saw, and one made against an older version is refused
//! ([`Error::Http`] with status 409), so two devices can't undo each other
//! unknowingly.

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{Error, MeleteClient, Result};

/// What's playing, as a device sets it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListenState {
    /// Track ids, in order.
    pub queue: Vec<i64>,
    /// Which entry of the queue plays, from 0 (0 for an empty queue).
    pub index: usize,
    /// How far into it, in milliseconds, now.
    pub position_ms: u64,
    /// Playing, not paused.
    pub playing: bool,
}

/// A session, as the server last told it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ListenSession {
    /// The server's id for it.
    pub id: i64,
    /// Six letters and digits others join with.
    pub code: String,
    /// The user name of whoever started it.
    pub host: String,
    /// Track ids, in order.
    pub queue: Vec<i64>,
    /// Which entry of the queue plays.
    pub index: usize,
    /// Where in the track at `position_at`.
    pub position_ms: u64,
    /// Server time, Unix milliseconds.
    pub position_at: i64,
    /// Playing, not paused.
    pub playing: bool,
    /// Raised by every change.
    pub version: i64,
    /// Every device in it, this one included.
    pub members: Vec<ListenMember>,
}

impl ListenSession {
    /// Where the track is at server time `server_now`: it moves on from
    /// `position_at` while playing.
    pub fn position_at(&self, server_now: i64) -> u64 {
        if !self.playing {
            return self.position_ms;
        }
        let since = u64::try_from(server_now.saturating_sub(self.position_at)).unwrap_or(0);
        self.position_ms.saturating_add(since)
    }
}

/// One device in a session.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ListenMember {
    /// The user the device is signed in as.
    pub username: String,
    /// The device's name, as signed in.
    pub device: String,
    /// The device asking.
    pub you: bool,
}

/// A session with the server's clock when it answered, to set this device's
/// against.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Listening {
    /// The session as it is now.
    pub session: ListenSession,
    /// Unix milliseconds.
    pub server_time: i64,
}

impl MeleteClient {
    /// The sessions this device is in.
    ///
    /// # Errors
    ///
    /// HTTP 403 as [`Error::Http`] for a user who may see only some music
    /// libraries; otherwise the [request errors](MeleteClient#errors).
    pub async fn listening_sessions(&self) -> Result<Vec<ListenSession>> {
        self.get(&["api", "v1", "music", "sessions"], &[]).await
    }

    /// Starts a session playing `state`, with this device in it.
    ///
    /// # Errors
    ///
    /// HTTP 400 as [`Error::Http`] for a track that isn't in the library;
    /// otherwise as [`listening_sessions`](Self::listening_sessions).
    pub async fn start_listening(&self, state: &ListenState) -> Result<Listening> {
        let body = json!(state);
        self.send_decoded(
            Method::POST,
            &["api", "v1", "music", "sessions"],
            &[],
            Some(&body),
        )
        .await
    }

    /// Joins the session with this code (any case, a space or dash allowed).
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when no session has the code; HTTP 429 as
    /// [`Error::Http`] after five wrong codes in 15 minutes.
    pub async fn join_listening(&self, code: &str) -> Result<Listening> {
        let body = json!({ "code": code });
        self.send_decoded(
            Method::POST,
            &["api", "v1", "music", "sessions", "join"],
            &[],
            Some(&body),
        )
        .await
    }

    /// One session this device is in, as it is now.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] once it has ended or this device left it.
    pub async fn listening(&self, id: i64) -> Result<Listening> {
        let id = id.to_string();
        self.get(&["api", "v1", "music", "sessions", &id], &[])
            .await
    }

    /// Sets what plays, made against `version`.
    ///
    /// # Errors
    ///
    /// HTTP 409 as [`Error::Http`] when the session changed since `version`
    /// (see [`is_stale`]); [`Error::NotFound`] once it has ended.
    pub async fn change_listening(
        &self,
        id: i64,
        version: i64,
        state: &ListenState,
    ) -> Result<Listening> {
        let mut body = json!(state);
        body["version"] = json!(version);
        let id = id.to_string();
        self.send_decoded(
            Method::PUT,
            &["api", "v1", "music", "sessions", &id],
            &[],
            Some(&body),
        )
        .await
    }

    /// Leaves a session; the last to leave ends it.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when it had ended already.
    pub async fn leave_listening(&self, id: i64) -> Result<()> {
        let id = id.to_string();
        self.send_empty(
            Method::POST,
            &["api", "v1", "music", "sessions", &id, "leave"],
            &[],
        )
        .await
    }

    /// Ends a session for everyone in it: for whoever started it, or an admin.
    ///
    /// # Errors
    ///
    /// HTTP 403 as [`Error::Http`] for anyone else.
    pub async fn end_listening(&self, id: i64) -> Result<()> {
        let id = id.to_string();
        self.send_empty(
            Method::DELETE,
            &["api", "v1", "music", "sessions", &id],
            &[],
        )
        .await
    }
}

/// Whether a change was refused for being made against an old version.
pub fn is_stale(error: &Error) -> bool {
    matches!(error, Error::Http { status: 409, .. })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(playing: bool) -> ListenSession {
        ListenSession {
            id: 1,
            code: "ABC234".into(),
            host: "alice".into(),
            queue: vec![4, 5],
            index: 0,
            position_ms: 10_000,
            position_at: 1_000_000,
            playing,
            version: 3,
            members: Vec::new(),
        }
    }

    #[test]
    fn the_position_moves_on_only_while_playing() {
        assert_eq!(session(true).position_at(1_002_500), 12_500);
        assert_eq!(session(false).position_at(1_002_500), 10_000);
        // A clock behind the server's mark doesn't wind it back.
        assert_eq!(session(true).position_at(999_000), 10_000);
    }
}
