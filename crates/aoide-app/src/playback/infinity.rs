//! Infinity: keeps the music going when the queue runs out.
//!
//! When fewer than three tracks are left ahead, Melete's `/aoide/next` is
//! asked what should follow the one playing, and its answer is queued in its
//! own order with the reasons shown on the queue rows. A server without it
//! answers 404, and Infinity rests for the session.

use std::collections::{HashMap, HashSet};

use aoide_audio::EntryId;
use aoide_melete::{Item, MeleteClient};
use aoide_sidecar::{NextMode, NextRequest, SidecarClient, SidecarError};

use crate::store_worker::StoreHandle;

/// Top up when fewer than this many tracks are left ahead.
pub const MINIMUM_AHEAD: usize = 3;
/// How many to add each time.
pub const BATCH: u32 = 20;

/// Decides when to ask. Fires once when the queue gets short, and not again
/// until the queue changes (another track starts, or the length moves), so a
/// fetch that finds nothing is not repeated on every position tick.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct InfinityTrigger {
    fired_for: Option<(EntryId, usize)>,
    in_flight: bool,
}

impl InfinityTrigger {
    /// Whether to fetch now. `repeat_off` because a looping queue never runs
    /// out, and filling one would quietly turn a loop into something else.
    pub fn should_fire(
        &mut self,
        enabled: bool,
        repeat_off: bool,
        current: Option<EntryId>,
        ahead: usize,
        queue_len: usize,
    ) -> bool {
        let Some(current) = current else {
            return false;
        };
        if !enabled || !repeat_off || self.in_flight || ahead >= MINIMUM_AHEAD {
            return false;
        }
        let key = (current, queue_len);
        if self.fired_for == Some(key) {
            return false;
        }
        self.fired_for = Some(key);
        self.in_flight = true;
        true
    }

    /// The fetch came back, with or without anything.
    pub fn finished(&mut self) {
        self.in_flight = false;
    }

    /// Infinity was switched on, or a new queue was loaded: look again.
    pub fn reset(&mut self) {
        self.fired_for = None;
    }
}

/// What a top-up found, in the order to queue it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TopUp {
    pub items: Vec<Item>,
    /// The server's factor summary, by track id.
    pub reasons: HashMap<String, String>,
    /// The server has no `/aoide/next`: don't ask again this session.
    pub sidecar_absent: bool,
}

/// Everything one top-up needs to know.
#[derive(Debug, Clone)]
pub struct Ask {
    pub seed: String,
    /// What is queued after the seed, in order.
    pub queue: Vec<String>,
    /// Every id already in the queue, which nothing is added twice over.
    pub known: HashSet<String>,
    /// Whether `/aoide/next` may still be asked this session.
    pub use_sidecar: bool,
}

/// Asks Melete what follows. A failure ends with nothing: the queue simply
/// ends, which is what it would have done anyway.
pub async fn top_up(
    client: MeleteClient,
    sidecar: Option<SidecarClient>,
    store: StoreHandle,
    ask: Ask,
) -> TopUp {
    if ask.use_sidecar
        && let Some(sidecar) = sidecar
    {
        let recent = store
            .call(|w| w.store.recently_played(NextRequest::MAX_RECENT))
            .await
            .and_then(Result::ok)
            .unwrap_or_default();
        let request = NextRequest::new(&ask.seed, &ask.queue, &recent, NextMode::Infinity, BATCH);
        match sidecar.next(&request).await {
            Ok(response) => {
                let chosen: Vec<_> = response
                    .candidates
                    .into_iter()
                    .filter(|c| !ask.known.contains(&c.id) && c.id != ask.seed)
                    .collect();
                let reasons = chosen
                    .iter()
                    .map(|c| (c.id.clone(), c.factors.summary()))
                    .collect();
                let ids: Vec<String> = chosen.into_iter().map(|c| c.id).collect();
                return match client.items_by_ids(&ids).await {
                    Ok(items) => TopUp {
                        items,
                        reasons,
                        sidecar_absent: false,
                    },
                    Err(_) => TopUp::default(),
                };
            }
            Err(SidecarError::Absent { .. }) => {
                return TopUp {
                    sidecar_absent: true,
                    ..TopUp::default()
                };
            }
            Err(_) => return TopUp::default(),
        }
    }
    TopUp::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_json::{page, track};
    use aoide_melete::{Credentials, Device, ServerAddress};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn fires_once_when_fewer_than_three_are_left() {
        let mut trigger = InfinityTrigger::default();
        let current = Some(EntryId(1));
        assert!(!trigger.should_fire(true, true, current, 3, 10));
        assert!(trigger.should_fire(true, true, current, 2, 10));
        // Still in flight, then finished with nothing new: not again.
        assert!(!trigger.should_fire(true, true, current, 2, 10));
        trigger.finished();
        assert!(!trigger.should_fire(true, true, current, 2, 10));
        // The queue moved on: look again.
        assert!(trigger.should_fire(true, true, Some(EntryId(2)), 1, 10));
        trigger.finished();
        // A top-up that added one is a change too.
        assert!(trigger.should_fire(true, true, Some(EntryId(2)), 2, 11));
    }

    #[test]
    fn never_fires_when_off_looping_or_empty() {
        let mut trigger = InfinityTrigger::default();
        assert!(!trigger.should_fire(false, true, Some(EntryId(1)), 0, 1));
        assert!(!trigger.should_fire(true, false, Some(EntryId(1)), 0, 1));
        assert!(!trigger.should_fire(true, true, None, 0, 0));
    }

    #[test]
    fn a_reset_looks_again_at_the_same_queue() {
        let mut trigger = InfinityTrigger::default();
        assert!(trigger.should_fire(true, true, Some(EntryId(1)), 0, 1));
        trigger.finished();
        trigger.reset();
        assert!(trigger.should_fire(true, true, Some(EntryId(1)), 0, 1));
    }

    fn signed_in(server: &MockServer) -> MeleteClient {
        MeleteClient::new(
            ServerAddress::parse(&server.uri()).unwrap(),
            Device::new("Test", "device-1"),
        )
        .unwrap()
        .with_credentials(Credentials {
            user_id: "u1".into(),
            access_token: "tok".into(),
            username: "alice".into(),
        })
    }

    fn ask() -> Ask {
        Ask {
            seed: "10".into(),
            queue: vec!["13".into()],
            known: HashSet::from(["10".to_owned(), "13".to_owned()]),
            use_sidecar: true,
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_sidecars_answer_is_queued_in_its_order_with_reasons() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/aoide/next"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [
                    {"id": "12", "score": 0.9, "factors": {"taste": 0.5, "freshness": 1.0, "similarity": 0.8, "arc": 0.4}},
                    {"id": "13", "score": 0.8, "factors": {"taste": 0.5, "freshness": 1.0, "similarity": 0.8, "arc": 0.4}},
                    {"id": "11", "score": 0.7, "factors": {"taste": 0.2, "freshness": 0.1, "similarity": 0.3, "arc": 0.4}}
                ],
                "profile": {"events": 12, "since": 0}
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/music/tracks"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(page(vec![track(11, "One"), track(12, "Two")])),
            )
            .mount(&server)
            .await;
        let client = signed_in(&server);
        let sidecar = SidecarClient::new(&server.uri(), "tok").unwrap();
        let store = StoreHandle::in_memory("device-1").unwrap();

        let found = top_up(client, Some(sidecar), store, ask()).await;
        let ids: Vec<&str> = found.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["12", "11"], "server order kept, queued ones dropped");
        assert!(
            found.reasons["11"].contains("heard lately"),
            "{:?}",
            found.reasons
        );
        assert!(found.reasons["12"].starts_with("Taste 50%"));
        assert!(!found.sidecar_absent);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_404_ends_with_nothing_and_rests_for_the_session() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/aoide/next"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let client = signed_in(&server);
        let sidecar = SidecarClient::new(&server.uri(), "tok").unwrap();
        let store = StoreHandle::in_memory("device-1").unwrap();
        let found = top_up(client, Some(sidecar), store, ask()).await;
        assert!(found.items.is_empty());
        assert!(found.sidecar_absent);
    }
}
