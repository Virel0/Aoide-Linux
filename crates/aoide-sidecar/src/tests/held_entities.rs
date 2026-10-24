//! Port of `TrackFlagsTests.swift`, suite "Holding entities the server does not
//! accept": an entity a server may predate is held, never refused and quarantined.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use aoide_store::{PullCursor, PullResponse, PushRequest, PushResponse, Store};

use super::{pending, store};
use crate::{SidecarError, SyncEngine, SyncTransport};

/// Records what it was sent, accepts all of it, and advertises what it is told to.
struct Recorder {
    pushed: Mutex<Vec<String>>,
    accepted: Mutex<Option<BTreeSet<String>>>,
}

impl Recorder {
    fn new(accepted: Option<&[&str]>) -> Self {
        Self {
            pushed: Mutex::default(),
            accepted: Mutex::new(accepted.map(entity_set)),
        }
    }

    fn set_accepted(&self, accepted: &[&str]) {
        *self.accepted.lock().unwrap() = Some(entity_set(accepted));
    }

    fn pushed(&self) -> Vec<String> {
        self.pushed.lock().unwrap().clone()
    }
}

fn entity_set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

impl SyncTransport for Arc<Recorder> {
    async fn push(&self, request: &PushRequest) -> Result<PushResponse, SidecarError> {
        self.pushed
            .lock()
            .unwrap()
            .extend(request.ops.iter().map(|op| op.entity.clone()));
        Ok(PushResponse {
            accepted: request.ops.iter().map(|op| op.op_id.clone()).collect(),
            ..Default::default()
        })
    }

    async fn pull(
        &self,
        since: Option<PullCursor>,
        _limit: usize,
    ) -> Result<PullResponse, SidecarError> {
        Ok(PullResponse {
            ops: vec![],
            cursor: since.unwrap_or(PullCursor(0)),
            has_more: false,
            unreadable_ops: 0,
        })
    }

    async fn accepted_entities(&self) -> Option<BTreeSet<String>> {
        self.accepted.lock().unwrap().clone()
    }
}

fn seeded() -> Arc<Store> {
    let store = store("phone");
    store.create_playlist("p", None, None, 5).unwrap();
    store.set_not_interested(true, "t1", "k1", 6).unwrap();
    store
}

fn pending_entities(store: &Store) -> Vec<String> {
    pending(store).into_iter().map(|op| op.entity).collect()
}

#[tokio::test]
async fn a_flag_op_waits_while_the_server_does_not_list_track_flags_and_the_playlist_still_goes() {
    let store = seeded();
    let transport = Arc::new(Recorder::new(Some(&["playlists", "likes"])));
    let engine = SyncEngine::new(Arc::clone(&store), Arc::clone(&transport));
    engine.sync().await.unwrap();
    assert_eq!(transport.pushed(), ["playlists"]);
    assert_eq!(engine.held_entities(), entity_set(&["trackFlags"]));
    assert_eq!(pending_entities(&store), ["trackFlags"]);
    assert!(store.quarantined_ops(200).unwrap().is_empty());

    transport.set_accepted(&["playlists", "track_flags"]);
    engine.sync().await.unwrap();
    assert_eq!(transport.pushed(), ["playlists", "track_flags"]);
    assert!(engine.held_entities().is_empty());
    assert!(pending(&store).is_empty());
}

/// A transport that cannot say what it accepts holds the new entity, never refuses it.
#[tokio::test]
async fn no_capability_answer_at_all_means_the_new_entity_is_held_never_refused() {
    let store = seeded();
    let transport = Arc::new(Recorder::new(None));
    let engine = SyncEngine::new(Arc::clone(&store), Arc::clone(&transport));
    engine.sync().await.unwrap();
    assert_eq!(transport.pushed(), ["playlists"]);
    assert_eq!(pending_entities(&store), ["trackFlags"]);
}
