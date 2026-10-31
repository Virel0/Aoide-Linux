//! Comparing this player with the session, and what to do about a difference:
//! no player state here, so every rule is tested on its own.

use aoide_melete::{ListenSession, ListenState};

/// How far this player may be from the session before it counts as elsewhere:
/// enough for a stream that took a moment to start, short of a skip anyone
/// would hear.
pub const TOLERANCE_MS: u64 = 3_000;

/// The player as it stands, in the queue's own ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Local {
    pub ids: Vec<String>,
    pub index: usize,
    pub position_ms: u64,
    pub playing: bool,
}

impl Local {
    /// What to send for it; `None` when an id isn't one of Melete's (numbers).
    pub fn state(&self) -> Option<ListenState> {
        let queue = self
            .ids
            .iter()
            .map(|id| id.parse().ok())
            .collect::<Option<Vec<i64>>>()?;
        Some(ListenState {
            queue,
            index: self.index,
            position_ms: self.position_ms,
            playing: self.playing,
        })
    }
}

/// What playing in step with the session takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Already there.
    Nothing,
    /// A different queue: load the session's.
    Rebuild,
    /// The same queue: move within it.
    Adjust {
        /// Play this entry instead.
        jump: Option<usize>,
        /// Go to this place in the track.
        seek: Option<u64>,
        /// Play (true) or pause.
        play: Option<bool>,
    },
}

/// Whether the session's queue is the player's.
fn same_queue(session: &ListenSession, local: &Local) -> bool {
    session.queue.len() == local.ids.len()
        && session
            .queue
            .iter()
            .zip(&local.ids)
            .all(|(id, local)| id.to_string() == *local)
}

/// Whether the player is where the session says, at server time `server_now`.
pub fn in_step(session: &ListenSession, local: &Local, server_now: i64) -> bool {
    plan(session, local, server_now) == Plan::Nothing
}

/// What brings the player to where the session is at server time `server_now`.
pub fn plan(session: &ListenSession, local: &Local, server_now: i64) -> Plan {
    if !same_queue(session, local) {
        return Plan::Rebuild;
    }
    if session.queue.is_empty() {
        return Plan::Nothing;
    }
    let expected = session.position_at(server_now);
    let jump = (session.index != local.index).then_some(session.index);
    let seek =
        (jump.is_some() || expected.abs_diff(local.position_ms) > TOLERANCE_MS).then_some(expected);
    let play = (session.playing != local.playing).then_some(session.playing);
    if jump.is_none() && seek.is_none() && play.is_none() {
        Plan::Nothing
    } else {
        Plan::Adjust { jump, seek, play }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(queue: Vec<i64>, index: usize, position_ms: u64, playing: bool) -> ListenSession {
        ListenSession {
            id: 1,
            code: "ABC234".into(),
            host: "alice".into(),
            queue,
            index,
            position_ms,
            position_at: 10_000,
            playing,
            version: 1,
            members: Vec::new(),
        }
    }

    fn local(ids: &[&str], index: usize, position_ms: u64, playing: bool) -> Local {
        Local {
            ids: ids.iter().map(|s| (*s).to_owned()).collect(),
            index,
            position_ms,
            playing,
        }
    }

    #[test]
    fn a_player_where_the_session_says_is_in_step() {
        let shared = session(vec![4, 5], 1, 20_000, true);
        // Two seconds on, a player two seconds in plays in step.
        assert!(in_step(
            &shared,
            &local(&["4", "5"], 1, 22_000, true),
            12_000
        ));
        // So does one a little behind, within the tolerance.
        assert!(in_step(
            &shared,
            &local(&["4", "5"], 1, 19_500, true),
            12_000
        ));
    }

    #[test]
    fn a_different_queue_is_rebuilt() {
        let shared = session(vec![4, 5], 0, 0, true);
        assert_eq!(
            plan(&shared, &local(&["4"], 0, 0, true), 10_000),
            Plan::Rebuild
        );
        assert_eq!(
            plan(&shared, &local(&["5", "4"], 0, 0, true), 10_000),
            Plan::Rebuild
        );
    }

    #[test]
    fn another_track_is_jumped_to_at_the_sessions_place() {
        let shared = session(vec![4, 5], 1, 1_000, true);
        assert_eq!(
            plan(&shared, &local(&["4", "5"], 0, 60_000, true), 10_500),
            Plan::Adjust {
                jump: Some(1),
                seek: Some(1_500),
                play: None
            }
        );
    }

    #[test]
    fn drift_past_the_tolerance_is_sought_and_a_pause_followed() {
        let paused = session(vec![4, 5], 0, 30_000, false);
        assert_eq!(
            plan(&paused, &local(&["4", "5"], 0, 40_000, true), 99_000),
            Plan::Adjust {
                jump: None,
                seek: Some(30_000),
                play: Some(false)
            }
        );
        assert_eq!(
            plan(&paused, &local(&["4", "5"], 0, 31_000, true), 99_000),
            Plan::Adjust {
                jump: None,
                seek: None,
                play: Some(false)
            }
        );
    }

    #[test]
    fn an_empty_session_asks_nothing_of_an_empty_player() {
        let empty = session(Vec::new(), 0, 0, false);
        assert_eq!(plan(&empty, &local(&[], 0, 0, true), 10_000), Plan::Nothing);
    }

    #[test]
    fn only_meletes_number_ids_can_be_sent() {
        let sent = local(&["4", "5"], 1, 1_200, true).state().unwrap();
        assert_eq!(sent.queue, [4, 5]);
        assert_eq!(
            (sent.index, sent.position_ms, sent.playing),
            (1, 1_200, true)
        );
        assert_eq!(local(&["4", "local-file"], 0, 0, true).state(), None);
    }
}
