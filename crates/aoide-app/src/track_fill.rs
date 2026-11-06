//! Caching the tracks of songs played on other devices.
//!
//! Replay's top artists and albums, and the history rules of smart playlists,
//! read a play's artist, album and length from the local track cache. A play
//! synced from the phone, of a song this computer never played or listed, has
//! no row there: it counts in a recap's totals but not in its lists. The phone
//! caches its whole library; this computer caches what it touches, so after a
//! sync (and once the store is claimed) the played tracks it lacks are asked
//! of Melete and cached.
//!
//! An id Melete doesn't have (a file deleted since it was played) is
//! remembered for the session and not asked about again. A request that fails
//! keeps what the batches before it cached; the rest is asked at the next sync.

use std::collections::HashSet;

use aoide_melete::MeleteClient;

use crate::playback::tracks::TrackMeta;
use crate::store_worker::StoreHandle;

/// Ids asked of Melete at a time, each batch cached as it arrives: Melete's own
/// limit for one request.
const BATCH: usize = 100;

/// What the session keeps between fills.
#[derive(Debug, Default)]
pub struct TrackFill {
    running: bool,
    /// A fill was asked for while one ran: it runs again when that one ends.
    again: bool,
    /// Played ids Melete doesn't have.
    gone: HashSet<String>,
}

impl TrackFill {
    /// The ids to leave out when a fill should start now; `None` while one is
    /// running, which then runs once more when it ends.
    pub fn start(&mut self) -> Option<HashSet<String>> {
        if self.running {
            self.again = true;
            return None;
        }
        self.running = true;
        self.again = false;
        Some(self.gone.clone())
    }

    /// A fill ended: what Melete didn't have is remembered. True when another
    /// fill was asked for meanwhile.
    pub fn finished(&mut self, gone: Vec<String>) -> bool {
        self.running = false;
        self.gone.extend(gone);
        std::mem::take(&mut self.again)
    }
}

/// One fill's outcome.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filled {
    /// Rows written to the cache.
    pub cached: usize,
    /// Ids Melete answered without: not asked about again this session.
    pub gone: Vec<String>,
    /// Why it stopped early, if it did.
    pub failed: Option<String>,
}

/// Asks Melete for the played tracks the cache lacks, but for `skip`, and
/// caches them, a batch at a time.
pub async fn fill(client: MeleteClient, store: StoreHandle, skip: HashSet<String>) -> Filled {
    let mut filled = Filled::default();
    let missing = match store
        .call(|w| w.store.played_ids_missing_from_cache())
        .await
    {
        Some(Ok(ids)) => ids,
        Some(Err(error)) => return failed(filled, error.to_string()),
        None => return failed(filled, "the library is closing".to_owned()),
    };
    // Melete's ids are numbers; anything else (history from before Melete) is
    // never found, so it isn't asked for.
    let (wanted, foreign): (Vec<String>, Vec<String>) = missing
        .into_iter()
        .filter(|id| !skip.contains(id))
        .partition(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()));
    filled.gone = foreign;
    for batch in wanted.chunks(BATCH) {
        if let Err(error) = fill_batch(&client, &store, batch, &mut filled).await {
            return failed(filled, error);
        }
    }
    filled
}

/// One batch asked for and cached; the ids Melete left out are added to `gone`.
async fn fill_batch(
    client: &MeleteClient,
    store: &StoreHandle,
    batch: &[String],
    filled: &mut Filled,
) -> Result<(), String> {
    let items = client
        .items_by_ids(batch)
        .await
        .map_err(|e| e.to_string())?;
    let now = aoide_store::now_millis();
    let rows: Vec<_> = items
        .iter()
        .map(|item| TrackMeta::from_item(item).track_ref(now))
        .collect();
    let found: HashSet<&str> = rows.iter().map(|r| r.jellyfin_id.as_str()).collect();
    filled.gone.extend(
        batch
            .iter()
            .filter(|id| !found.contains(id.as_str()))
            .cloned(),
    );
    let count = rows.len();
    store
        .call(move |w| w.store.cache_tracks(&rows))
        .await
        .ok_or_else(|| "the library is closing".to_owned())?
        .map_err(|e| e.to_string())?;
    filled.cached += count;
    Ok(())
}

fn failed(mut filled: Filled, error: String) -> Filled {
    filled.failed = Some(error);
    filled
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_json::{page, track};
    use aoide_melete::{Credentials, Device, ServerAddress};
    use aoide_store::PlayEvent;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(server: &MockServer) -> MeleteClient {
        MeleteClient::new(
            ServerAddress::parse(&server.uri()).unwrap(),
            Device::new("Test", "device-1"),
        )
        .unwrap()
        .with_credentials(Credentials {
            user_id: "1".into(),
            access_token: "tok".into(),
            username: "alice".into(),
        })
    }

    /// A store whose history has plays, from the phone, of these ids.
    async fn played_on_the_phone(ids: &[&str]) -> StoreHandle {
        let store = StoreHandle::in_memory("desk").unwrap();
        let ids: Vec<String> = ids.iter().map(|id| (*id).to_owned()).collect();
        store
            .call(move |w| {
                for (at, id) in (1_000..).zip(&ids) {
                    let event = PlayEvent::new(id, id, at, "phone").finished(
                        Some(at + 200_000),
                        200_000,
                        true,
                        false,
                    );
                    w.store.append(event).unwrap();
                }
            })
            .await
            .unwrap();
        store
    }

    async fn missing(store: &StoreHandle) -> Vec<String> {
        store
            .call(|w| w.store.played_ids_missing_from_cache().unwrap())
            .await
            .unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn played_tracks_melete_has_are_cached_and_the_rest_are_gone() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/music/tracks"))
            .and(query_param("id", "1,2,3"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(page(vec![track(1, "Lantern"), track(3, "Tide")])),
            )
            .expect(1)
            .mount(&server)
            .await;
        let store = played_on_the_phone(&["1", "2", "3", "jellyfin-guid"]).await;

        let filled = fill(client(&server), store.clone(), HashSet::new()).await;

        assert_eq!(filled.failed, None);
        assert_eq!(filled.cached, 2);
        let mut gone = filled.gone.clone();
        gone.sort();
        assert_eq!(gone, ["2", "jellyfin-guid"]);
        assert_eq!(missing(&store).await, ["2", "jellyfin-guid"]);
        let cached = store
            .call(|w| w.store.cached_tracks(&["1".to_owned()]).unwrap())
            .await
            .unwrap();
        let row = &cached["1"];
        assert_eq!(
            (row.title.as_str(), row.artist.as_str(), row.album.as_str()),
            ("Lantern", "An Artist", "An Album")
        );
        assert_eq!(row.duration_ms, Some(200_000));
    }

    /// What the session remembers as gone is not asked for again; nothing left
    /// to ask is no request at all.
    #[tokio::test(flavor = "multi_thread")]
    async fn ids_known_to_be_gone_are_not_asked_again() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(vec![])))
            .expect(0)
            .mount(&server)
            .await;
        let store = played_on_the_phone(&["2"]).await;

        let filled = fill(client(&server), store, HashSet::from(["2".to_owned()])).await;

        assert_eq!(filled, Filled::default());
    }

    /// A failed request is reported; the next sync asks again.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_request_marks_nothing_gone() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/music/tracks"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let store = played_on_the_phone(&["1"]).await;

        let filled = fill(client(&server), store.clone(), HashSet::new()).await;

        assert!(filled.failed.is_some());
        assert_eq!((filled.cached, filled.gone.len()), (0, 0));
        assert_eq!(missing(&store).await, ["1"]);
    }

    #[test]
    fn a_fill_asked_for_while_one_runs_runs_once_more_after() {
        let mut state = TrackFill::default();
        assert_eq!(state.start(), Some(HashSet::new()));
        assert_eq!(state.start(), None);
        assert_eq!(state.start(), None);
        assert!(state.finished(vec!["9".into()]));
        assert_eq!(state.start(), Some(HashSet::from(["9".to_owned()])));
        assert!(!state.finished(Vec::new()));
        assert!(state.start().is_some(), "not running any more");
    }
}
