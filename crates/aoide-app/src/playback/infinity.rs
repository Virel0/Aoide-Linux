//! Infinity: keeps the music going when the queue runs out.
//!
//! When fewer than three tracks are left ahead, the sidecar's `/aoide/next` is
//! asked what should follow the one playing, and its answer is queued in its
//! own order with the reasons shown on the queue rows. An older server answers
//! 404; for the rest of the session Jellyfin's Instant Mix is the pool instead,
//! ranked by what this listener actually finishes (the store's taste ranking).

use std::collections::{HashMap, HashSet};

use aoide_audio::EntryId;
use aoide_jellyfin::{Item, JellyfinClient};
use aoide_sidecar::{NextMode, NextRequest, SidecarClient, SidecarError};
use aoide_store::{FinishCounts, TasteCandidate, TasteProfile, TasteRanking};

use crate::store_worker::StoreHandle;

/// Top up when fewer than this many tracks are left ahead.
pub const MINIMUM_AHEAD: usize = 3;
/// How many to add each time.
pub const BATCH: u32 = 20;
/// How many Instant Mix is asked for, to rank those from.
pub const POOL: u32 = 100;
/// How far back the taste profile reads.
pub const WINDOW_DAYS: i64 = 90;

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

    pub fn is_fetching(&self) -> bool {
        self.in_flight
    }
}

/// What a top-up found, in the order to queue it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TopUp {
    pub items: Vec<Item>,
    /// The sidecar's factor summary, by track id.
    pub reasons: HashMap<String, String>,
    /// The sidecar has no `/aoide/next`: use Instant Mix for the session.
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
    /// Whether the sidecar may still be asked this session.
    pub use_sidecar: bool,
}

/// Asks the sidecar, falling back to Instant Mix ranked locally. Errors other
/// than a missing endpoint end with nothing: the queue simply ends, which is
/// what it would have done anyway.
pub async fn top_up(
    client: JellyfinClient,
    sidecar: Option<SidecarClient>,
    store: StoreHandle,
    ask: Ask,
    now: aoide_store::Millis,
) -> TopUp {
    let mut sidecar_absent = false;
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
            Err(SidecarError::Absent { .. }) => sidecar_absent = true,
            Err(_) => return TopUp::default(),
        }
    }

    let Ok(pool) = client.instant_mix(&ask.seed, POOL).await else {
        return TopUp {
            sidecar_absent,
            ..TopUp::default()
        };
    };
    let fresh: Vec<Item> = pool
        .into_iter()
        .filter(|item| item.id != ask.seed && !ask.known.contains(&item.id))
        .collect();
    let ids: Vec<String> = fresh.iter().map(|item| item.id.clone()).collect();
    let since = now - WINDOW_DAYS * 24 * 60 * 60 * 1000;
    let judged = store
        .call(move |w| {
            let hidden = w.store.not_interested(&ids).unwrap_or_default();
            let profile = w.store.taste_profile(since, 200, 40).unwrap_or_default();
            let finish = w.store.finish_counts(&ids).unwrap_or_default();
            (hidden, profile, finish)
        })
        .await;
    let (hidden, profile, finish) = judged.unwrap_or_default();
    let allowed: Vec<Item> = fresh
        .into_iter()
        .filter(|item| !hidden.contains(&item.id))
        .collect();
    TopUp {
        items: rank(allowed, &profile, &finish, BATCH as usize),
        reasons: HashMap::new(),
        sidecar_absent,
    }
}

/// The station's suggestions in the order this listener likeliest wants them.
/// With no history to read, the station's own order stands.
pub fn rank(
    items: Vec<Item>,
    profile: &TasteProfile,
    finish: &HashMap<String, FinishCounts>,
    limit: usize,
) -> Vec<Item> {
    if profile.is_empty() {
        return items.into_iter().take(limit).collect();
    }
    let candidates: Vec<TasteCandidate> = items
        .iter()
        .map(|item| {
            let artist = item
                .album_artist
                .clone()
                .or_else(|| item.artists.first().cloned())
                .unwrap_or_else(|| "Unknown Artist".to_owned());
            TasteCandidate::new(item.id.clone(), artist, &item.genres)
        })
        .collect();
    let ranked = TasteRanking::rank(&candidates, profile, finish, &HashMap::new(), limit);
    let mut by_id: HashMap<String, Item> = items.into_iter().map(|i| (i.id.clone(), i)).collect();
    ranked
        .into_iter()
        .filter_map(|candidate| by_id.remove(&candidate.id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoide_jellyfin::{Credentials, Device, ServerAddress};
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

    fn item(id: &str, artist: &str, genre: &str) -> Item {
        serde_json::from_value(serde_json::json!({
            "Id": id, "Name": id, "Type": "Audio",
            "AlbumArtist": artist, "Genres": [genre]
        }))
        .unwrap()
    }

    #[test]
    fn with_no_history_the_stations_order_stands() {
        let items = vec![
            item("a", "x", "rock"),
            item("b", "y", "jazz"),
            item("c", "z", "pop"),
        ];
        let ranked = rank(items, &TasteProfile::default(), &HashMap::new(), 2);
        let ids: Vec<&str> = ranked.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn taste_reorders_the_pool() {
        let items = vec![item("a", "x", "rock"), item("b", "y", "jazz")];
        let profile = TasteProfile {
            genres: HashMap::from([("jazz".to_owned(), 1.0)]),
            ..TasteProfile::default()
        };
        let ranked = rank(items, &profile, &HashMap::new(), 20);
        let ids: Vec<&str> = ranked.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["b", "a"]);
    }

    fn signed_in(server: &MockServer) -> JellyfinClient {
        JellyfinClient::new(
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
            seed: "seed".into(),
            queue: vec!["q1".into()],
            known: HashSet::from(["seed".to_owned(), "q1".to_owned()]),
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
                    {"id": "n2", "score": 0.9, "factors": {"taste": 0.5, "freshness": 1.0, "similarity": 0.8, "arc": 0.4}},
                    {"id": "q1", "score": 0.8, "factors": {"taste": 0.5, "freshness": 1.0, "similarity": 0.8, "arc": 0.4}},
                    {"id": "n1", "score": 0.7, "factors": {"taste": 0.2, "freshness": 0.1, "similarity": 0.3, "arc": 0.4}}
                ],
                "profile": {"events": 12, "since": 0}
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/Items"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Items": [{"Id": "n1", "Name": "One", "Type": "Audio"}, {"Id": "n2", "Name": "Two", "Type": "Audio"}],
                "TotalRecordCount": 2
            })))
            .mount(&server)
            .await;
        let client = signed_in(&server);
        let sidecar = SidecarClient::new(&server.uri(), "tok").unwrap();
        let store = StoreHandle::in_memory("device-1").unwrap();

        let found = top_up(client, Some(sidecar), store, ask(), 0).await;
        let ids: Vec<&str> = found.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["n2", "n1"], "server order kept, queued ones dropped");
        assert!(
            found.reasons["n1"].contains("heard lately"),
            "{:?}",
            found.reasons
        );
        assert!(found.reasons["n2"].starts_with("Taste 50%"));
        assert!(!found.sidecar_absent);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_404_falls_back_to_instant_mix_without_hidden_tracks() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/aoide/next"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/Items/seed/InstantMix"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "Items": [
                    {"Id": "seed", "Name": "Seed", "Type": "Audio"},
                    {"Id": "m1", "Name": "M1", "Type": "Audio"},
                    {"Id": "m2", "Name": "M2", "Type": "Audio"},
                    {"Id": "q1", "Name": "Q1", "Type": "Audio"}
                ],
                "TotalRecordCount": 4
            })))
            .mount(&server)
            .await;
        let client = signed_in(&server);
        let sidecar = SidecarClient::new(&server.uri(), "tok").unwrap();
        let store = StoreHandle::in_memory("device-1").unwrap();
        store
            .call(|w| w.store.set_not_interested(true, "m1", "k", 0).unwrap())
            .await
            .unwrap();

        let found = top_up(client, Some(sidecar), store, ask(), 0).await;
        let ids: Vec<&str> = found.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["m2"]);
        assert!(found.sidecar_absent);
        assert!(found.reasons.is_empty());
    }
}
