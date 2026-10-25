//! Collaborative playlists: the share endpoints over HTTP, and the one refusal that is
//! not permanent — held back, surfaced, re-checked every five minutes (the desktop's
//! `sync-engine.test.ts` revoked-share cases).

use std::collections::HashSet;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use aoide_store::{
    PullCursor, PullResponse, PushRequest, PushResponse, RejectedOp, Store, SyncPolicy, TrackLink,
};
use serde_json::json;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{pending, store};
use crate::{
    PlaylistShare, SHARE_RECHECK_MS, SidecarClient, SidecarError, SyncEngine, SyncTransport,
    is_share_revoked,
};

const ME: &str = "user-me";

fn revoked_reason(playlist: &str) -> String {
    format!("Playlist '{playlist}' belongs to another user and is not shared with you for editing.")
}

fn share(playlist: &str, grantee: &str, owner: Option<&str>, can_edit: bool) -> PlaylistShare {
    PlaylistShare {
        playlist_id: playlist.to_owned(),
        grantee_user_id: grantee.to_owned(),
        can_edit,
        owner_user_id: owner.map(str::to_owned),
    }
}

// MARK: - HTTP

#[tokio::test]
async fn shares_are_listed_created_and_revoked_as_the_contract_spells_them() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/aoide/shares"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "playlistId": "p1", "granteeUserId": "u2", "canEdit": true, "ownerUserId": ME },
            { "playlistId": "p2", "granteeUserId": ME, "canEdit": false },
            { "nonsense": true }
        ])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/aoide/shares"))
        .and(body_json(
            json!({ "playlistId": "p1", "granteeUserId": "u3", "canEdit": true }),
        ))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/aoide/shares/p%201/u%2F2"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let client = SidecarClient::new(&server.uri(), "tok").unwrap();

    let shares = client.list_shares().await.unwrap();
    assert_eq!(
        shares,
        [
            share("p1", "u2", Some(ME), true),
            share("p2", ME, None, false)
        ]
    );
    client.share_playlist("p1", "u3", true).await.unwrap();
    client.revoke_share("p 1", "u/2").await.unwrap();
}

#[tokio::test]
async fn a_wrapped_share_list_reads_the_same_and_a_refusal_is_forbidden() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/aoide/shares"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "shares": [ { "playlistId": "p1", "granteeUserId": "u2", "canEdit": true } ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/aoide/shares"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    let client = SidecarClient::new(&server.uri(), "tok").unwrap();
    assert_eq!(client.list_shares().await.unwrap().len(), 1);
    assert_eq!(
        client.share_playlist("p9", "u2", false).await.unwrap_err(),
        SidecarError::Forbidden,
        "only the owner can share"
    );
}

// MARK: - Whose access a share proves

#[test]
fn only_an_editable_share_naming_this_user_proves_they_may_edit() {
    assert!(share("p", ME, Some("owner"), true).grants_edit_to(Some(ME)));
    assert!(
        share("p", "someone", Some(ME), true).grants_edit_to(Some(ME)),
        "mine"
    );
    assert!(!share("p", "someone", Some("owner"), true).grants_edit_to(Some(ME)));
    assert!(!share("p", ME, Some("owner"), false).grants_edit_to(Some(ME)));
    assert!(share("p", "anyone", None, true).grants_edit_to(None));
    assert!(!share("p", ME, Some("owner"), true).grants_edit_to(None));
}

/// Matched on the clause, case-insensitively, not on the quoted id or the stop.
#[test]
fn the_revoked_share_refusal_is_recognised_by_its_clause() {
    assert!(is_share_revoked(&revoked_reason("p1")));
    assert!(is_share_revoked(
        "PLAYLIST X BELONGS TO ANOTHER USER AND IS NOT SHARED WITH YOU FOR EDITING"
    ));
    assert!(!is_share_revoked("unknown entity 'tracks'"));
}

// MARK: - The engine

#[derive(Default)]
struct State {
    revoked: HashSet<String>,
    shares: Vec<PlaylistShare>,
    fail_shares: bool,
    share_reads: usize,
    /// Every op id in every push, in order.
    pushed: Vec<String>,
}

#[derive(Clone, Default)]
struct ShareServer(Arc<Mutex<State>>);

impl ShareServer {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap()
    }
}

impl SyncTransport for ShareServer {
    async fn push(&self, request: &PushRequest) -> Result<PushResponse, SidecarError> {
        let mut state = self.state();
        let mut response = PushResponse::default();
        for op in &request.ops {
            state.pushed.push(op.op_id.clone());
            let playlist = if op.entity == "playlists" {
                Some(op.entity_id.clone())
            } else {
                op.payload["playlistId"].as_str().map(str::to_owned)
            };
            match playlist.filter(|p| state.revoked.contains(p)) {
                Some(playlist) => response.rejected.push(RejectedOp {
                    op_id: op.op_id.clone(),
                    reason: revoked_reason(&playlist),
                }),
                None => response.accepted.push(op.op_id.clone()),
            }
        }
        Ok(response)
    }

    async fn pull(
        &self,
        _since: Option<PullCursor>,
        _limit: usize,
    ) -> Result<PullResponse, SidecarError> {
        Ok(PullResponse {
            ops: vec![],
            cursor: PullCursor(0),
            has_more: false,
            unreadable_ops: 0,
        })
    }

    async fn list_shares(&self) -> Result<Vec<PlaylistShare>, SidecarError> {
        let mut state = self.state();
        state.share_reads += 1;
        if state.fail_shares {
            return Err(SidecarError::Network("offline".into()));
        }
        Ok(state.shares.clone())
    }
}

struct Rig {
    store: Arc<Store>,
    server: ShareServer,
    clock: Arc<AtomicI64>,
    engine: SyncEngine<ShareServer>,
}

fn rig(policy: SyncPolicy) -> Rig {
    let store = store("desk");
    let server = ShareServer::default();
    let clock = Arc::new(AtomicI64::new(1_000_000));
    let ticking = Arc::clone(&clock);
    let engine = SyncEngine::new(Arc::clone(&store), server.clone())
        .with_policy(policy)
        .with_user_id(ME)
        .with_clock(move || ticking.load(Ordering::SeqCst));
    Rig {
        store,
        server,
        clock,
        engine,
    }
}

impl Rig {
    fn advance(&self, ms: i64) {
        self.clock.fetch_add(ms, Ordering::SeqCst);
    }

    fn pushes_of(&self, op_id: &str) -> usize {
        self.server
            .state()
            .pushed
            .iter()
            .filter(|id| *id == op_id)
            .count()
    }
}

/// A shared playlist's id with one entry, both ops acknowledged.
fn shared_playlist(rig: &Rig, name: &str) -> String {
    let list = rig.store.create_playlist(name, None, None, 1).unwrap();
    rig.store
        .add_tracks(&[TrackLink::new("t1", "k1")], &list.id, 1)
        .unwrap();
    let ids: Vec<String> = pending(&rig.store).into_iter().map(|op| op.op_id).collect();
    rig.store.mark_synced(&ids).unwrap();
    list.id
}

#[tokio::test]
async fn an_edit_refused_for_a_revoked_share_is_held_not_quarantined_or_dropped() {
    let rig = rig(SyncPolicy::default());
    let shared = shared_playlist(&rig, "Shared");
    rig.server.state().revoked.insert(shared.clone());
    rig.store.rename_playlist(&shared, "Renamed", 2).unwrap();
    rig.store.create_playlist("Mine", None, None, 3).unwrap();

    let report = rig.engine.sync().await.unwrap();
    assert_eq!(report.blocked_by_share, 1);
    assert_eq!(report.rejected, 0);
    assert_eq!(report.quarantined_outbound, 0);
    assert_eq!(report.pushed, 1, "everything else still goes");
    assert_eq!(rig.store.quarantine_count().unwrap(), 0);

    let left = pending(&rig.store);
    assert_eq!(left.len(), 1, "the edit is still in the log");
    let edits = rig.engine.blocked_edits();
    assert_eq!(edits.len(), 1);
    assert_eq!(rig.engine.blocked_edit_count(), 1);
    assert_eq!(edits[0].op_id, left[0].op_id);
    assert_eq!(edits[0].playlist_id.as_deref(), Some(shared.as_str()));
    assert!(edits[0].reason.contains("not shared with you"));
    // The share list was read before the push, so the answer waits for a re-read.
    assert_eq!(edits[0].still_shared, None);
}

#[tokio::test]
async fn a_held_edit_is_not_pushed_again_and_does_not_starve_what_queued_behind_it() {
    let rig = rig(SyncPolicy::default().with_push_batch_size(1));
    let shared = shared_playlist(&rig, "Shared");
    rig.server.state().revoked.insert(shared.clone());
    rig.store.rename_playlist(&shared, "Renamed", 2).unwrap();
    rig.engine.sync().await.unwrap();
    let held = rig.engine.blocked_edits()[0].op_id.clone();
    assert_eq!(rig.pushes_of(&held), 1);

    for name in ["One", "Two", "Three"] {
        rig.store.create_playlist(name, None, None, 4).unwrap();
    }
    rig.advance(60_000);
    let report = rig.engine.sync().await.unwrap();
    assert_eq!(report.pushed, 3);
    assert_eq!(rig.pushes_of(&held), 1, "not pushed blindly again");
    assert_eq!(pending(&rig.store).len(), 1);
    assert_eq!(rig.engine.blocked_edit_count(), 1, "and still shown");
}

#[tokio::test]
async fn shares_are_re_read_every_five_minutes_and_a_restored_share_lets_the_edit_go() {
    let rig = rig(SyncPolicy::default());
    let shared = shared_playlist(&rig, "Shared");
    rig.server.state().revoked.insert(shared.clone());
    rig.store.rename_playlist(&shared, "Renamed", 2).unwrap();
    rig.engine.sync().await.unwrap();
    let reads = rig.server.state().share_reads;

    // Within five minutes: the list is not asked again.
    rig.advance(SHARE_RECHECK_MS - 1);
    rig.engine.sync().await.unwrap();
    assert_eq!(rig.server.state().share_reads, reads);

    // Access really is gone: re-read, still held, now known to be gone.
    rig.advance(1);
    rig.engine.sync().await.unwrap();
    assert_eq!(rig.server.state().share_reads, reads + 1);
    assert_eq!(rig.engine.blocked_edits()[0].still_shared, Some(false));

    // The owner shares it again; the next re-read releases the edit and it goes.
    {
        let mut state = rig.server.state();
        state.revoked.clear();
        state.shares = vec![share(&shared, ME, Some("owner"), true)];
    }
    rig.advance(SHARE_RECHECK_MS);
    let report = rig.engine.sync().await.unwrap();
    assert_eq!(report.pushed, 1);
    assert_eq!(rig.engine.blocked_edit_count(), 0);
    assert!(pending(&rig.store).is_empty());
}

#[tokio::test]
async fn a_failed_re_read_is_unknown_not_gone() {
    let rig = rig(SyncPolicy::default());
    let shared = shared_playlist(&rig, "Shared");
    rig.server.state().revoked.insert(shared.clone());
    rig.store.rename_playlist(&shared, "Renamed", 2).unwrap();
    rig.engine.sync().await.unwrap();
    rig.server.state().fail_shares = true;
    rig.advance(SHARE_RECHECK_MS);
    rig.engine.sync().await.unwrap();
    let edits = rig.engine.blocked_edits();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].still_shared, None);
    assert!(rig.engine.share_error().unwrap().contains("offline"));
    assert_eq!(pending(&rig.store).len(), 1);
}

/// An entry names its playlist in the payload; the refusal is the same.
#[tokio::test]
async fn an_entry_added_to_a_revoked_playlist_is_held_under_that_playlist() {
    let rig = rig(SyncPolicy::default());
    let shared = shared_playlist(&rig, "Shared");
    rig.server.state().revoked.insert(shared.clone());
    rig.store
        .add_tracks(&[TrackLink::new("t2", "k2")], &shared, 2)
        .unwrap();
    rig.engine.sync().await.unwrap();
    let edits = rig.engine.blocked_edits();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].playlist_id.as_deref(), Some(shared.as_str()));
}

#[tokio::test]
async fn a_share_this_user_loses_is_reported_and_can_be_dropped_here_only() {
    let rig = rig(SyncPolicy::default());
    let theirs = shared_playlist(&rig, "Theirs");
    rig.server.state().shares = vec![share(&theirs, ME, Some("owner"), true)];
    rig.engine.sync().await.unwrap();
    assert!(rig.engine.lost_shares().is_empty());

    // Revoked by the owner, with an edit of ours still queued for it.
    {
        let mut state = rig.server.state();
        state.shares.clear();
        state.revoked.insert(theirs.clone());
    }
    rig.store.rename_playlist(&theirs, "Mine now?", 2).unwrap();
    rig.advance(SHARE_RECHECK_MS);
    rig.engine.sync().await.unwrap();
    assert_eq!(
        rig.engine.lost_shares().into_iter().collect::<Vec<_>>(),
        std::slice::from_ref(&theirs)
    );
    assert_eq!(rig.engine.blocked_edit_count(), 1);

    let ops_before = pending(&rig.store).len();
    assert!(rig.store.drop_playlist_locally(&theirs).unwrap());
    assert_eq!(rig.store.playlist(&theirs).unwrap(), None);
    assert!(!rig.store.playlist_row_exists(&theirs).unwrap());
    assert!(rig.store.items(&theirs).unwrap().is_empty());
    assert_eq!(
        pending(&rig.store).len(),
        ops_before - 1,
        "its queued edit goes too"
    );

    rig.engine.sync().await.unwrap();
    assert_eq!(rig.engine.blocked_edit_count(), 0, "nothing left to hold");
}

#[tokio::test]
async fn any_other_refusal_is_still_quarantined() {
    let rig = rig(SyncPolicy::default());
    struct Refusing(ShareServer);
    impl SyncTransport for Refusing {
        async fn push(&self, request: &PushRequest) -> Result<PushResponse, SidecarError> {
            Ok(PushResponse {
                accepted: vec![],
                rejected: request
                    .ops
                    .iter()
                    .map(|op| RejectedOp {
                        op_id: op.op_id.clone(),
                        reason: "payload must be a JSON object".into(),
                    })
                    .collect(),
                cursor: None,
            })
        }
        async fn pull(
            &self,
            since: Option<PullCursor>,
            limit: usize,
        ) -> Result<PullResponse, SidecarError> {
            self.0.pull(since, limit).await
        }
    }
    rig.store.create_playlist("Bad", None, None, 1).unwrap();
    let engine = SyncEngine::new(Arc::clone(&rig.store), Refusing(rig.server.clone()));
    let report = engine.sync().await.unwrap();
    assert_eq!(report.rejected, 1);
    assert_eq!(report.blocked_by_share, 0);
    assert_eq!(rig.store.quarantine_count().unwrap(), 1);
    assert_eq!(engine.blocked_edit_count(), 0);
    assert_eq!(
        engine.share_error(),
        None,
        "no sharing on this transport is not an error"
    );
}
