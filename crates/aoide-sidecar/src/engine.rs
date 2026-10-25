//! The sync loop: push everything pending, then read everything new.
//!
//! Push before pull, always. A device's own ops come back down with a sequence number,
//! which is how it learns they are durable — and pulling first would merge a server
//! state missing the local changes.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use aoide_store::{
    Millis, Op, PullCursor, PullResponse, PushRequest, PushResponse, Store, StoreError, SyncEntity,
    SyncFault, SyncPhase, SyncPolicy, SyncReport, WireOp, now_millis,
};

use crate::client::SidecarClient;
use crate::error::SidecarError;
use crate::types::PlaylistShare;

/// The network, held at arm's length so the engine can run against a stand-in.
///
/// Anything a transport fails with that is not a server fault (see
/// [`SidecarError::is_server_fault`]) is treated as transient: retrying a healthy op
/// costs a round trip, quarantining one because a router blinked costs the change.
pub trait SyncTransport: Send + Sync {
    fn push(
        &self,
        request: &PushRequest,
    ) -> impl Future<Output = Result<PushResponse, SidecarError>> + Send;

    /// `since` is `None` on a device that has never synced.
    fn pull(
        &self,
        since: Option<PullCursor>,
        limit: usize,
    ) -> impl Future<Output = Result<PullResponse, SidecarError>> + Send;

    /// The entities the server says push will accept, or `None` when it cannot say —
    /// which holds every entity that needs server support. A held op waits; a refused
    /// op is quarantined.
    fn accepted_entities(&self) -> impl Future<Output = Option<BTreeSet<String>>> + Send {
        async { None }
    }

    /// Whether the cover endpoints exist. A transport without them still syncs
    /// everything else, and covers stay on the device that chose them.
    fn supports_images(&self) -> bool {
        false
    }

    fn has_image(&self, _sha256: &str) -> impl Future<Output = Result<bool, SidecarError>> + Send {
        async { Err(SidecarError::Unsupported("images")) }
    }

    fn upload_image(
        &self,
        _sha256: &str,
        _bytes: Vec<u8>,
        _mime: &str,
    ) -> impl Future<Output = Result<(), SidecarError>> + Send {
        async { Err(SidecarError::Unsupported("images")) }
    }

    /// `GET /aoide/shares`. A transport without sharing has no shares to re-read,
    /// and an edit held back for one stays held back and visible.
    fn list_shares(&self) -> impl Future<Output = Result<Vec<PlaylistShare>, SidecarError>> + Send {
        async { Err(SidecarError::Unsupported("shares")) }
    }
}

impl SyncTransport for SidecarClient {
    fn push(
        &self,
        request: &PushRequest,
    ) -> impl Future<Output = Result<PushResponse, SidecarError>> + Send {
        SidecarClient::push(self, request)
    }

    fn pull(
        &self,
        since: Option<PullCursor>,
        limit: usize,
    ) -> impl Future<Output = Result<PullResponse, SidecarError>> + Send {
        SidecarClient::pull(self, since, limit)
    }

    /// A server that answers but lists nothing predates the list, and is read as
    /// "nothing new accepted"; one that cannot answer at all holds the same entities.
    async fn accepted_entities(&self) -> Option<BTreeSet<String>> {
        let status = self.status().await.ok()?;
        Some(
            status
                .accepted_entities
                .unwrap_or_default()
                .into_iter()
                .collect(),
        )
    }

    fn supports_images(&self) -> bool {
        true
    }

    fn has_image(&self, sha256: &str) -> impl Future<Output = Result<bool, SidecarError>> + Send {
        SidecarClient::has_image(self, sha256)
    }

    fn upload_image(
        &self,
        sha256: &str,
        bytes: Vec<u8>,
        mime: &str,
    ) -> impl Future<Output = Result<(), SidecarError>> + Send {
        self.put_image(sha256, bytes, mime)
    }

    fn list_shares(&self) -> impl Future<Output = Result<Vec<PlaylistShare>, SidecarError>> + Send {
        SidecarClient::list_shares(self)
    }
}

// MARK: - Share-revoked edits

/// How long a share refusal is believed before `/aoide/shares` is read again. A held
/// edit is not pushed again, so nothing else would ever ask; five minutes is well
/// under the patience of somebody just re-added to a playlist.
pub const SHARE_RECHECK_MS: Millis = 5 * 60 * 1000;

/// The words of the one refusal that is not permanent:
/// `Playlist 'p1' belongs to another user and is not shared with you for editing.`
/// Only this clause is matched — the quoted id and the full stop are formatting, and
/// requiring them would turn a reworded message back into a lost edit.
pub const REVOKED_SHARE_CLAUSE: &str =
    "belongs to another user and is not shared with you for editing";

/// Whether a rejection reason is the share-revoked refusal.
pub fn is_share_revoked(reason: &str) -> bool {
    reason.to_lowercase().contains(REVOKED_SHARE_CLAUSE)
}

/// An edit refused because the playlist's share was revoked while it sat queued.
///
/// **Not quarantined and not dropped**: the op stays in the log, because a restored
/// share would see it accepted. **Not retried blindly** either: it is left out of
/// every push until a re-read of `/aoide/shares` says this user may edit that
/// playlist again. Held in memory, so a restart asks the server once more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedEdit {
    pub op_id: String,
    /// From the op (a playlist's own id, or an entry's `playlistId`), else from the
    /// server's message. `None` when neither says.
    pub playlist_id: Option<String>,
    /// The server's words.
    pub reason: String,
    /// Whether the last read of `/aoide/shares` lists the playlist as editable by
    /// this user. `None`: not asked yet, or the read failed — unknown, not gone.
    pub still_shared: Option<bool>,
}

fn playlist_of(op: &Op, reason: &str) -> Option<String> {
    let from_op = match op.entity.as_str() {
        "playlist" => Some(op.entity_id.clone()),
        _ => serde_json::from_str::<serde_json::Value>(&op.payload)
            .ok()
            .and_then(|payload| payload.get("playlistId")?.as_str().map(str::to_owned)),
    };
    from_op.filter(|id| !id.is_empty()).or_else(|| {
        let start = reason.find("laylist '")? + "laylist '".len();
        let end = reason[start..].find('\'')? + start;
        Some(reason[start..end].to_owned()).filter(|id| !id.is_empty())
    })
}

/// Reads the bytes behind a cover hash from wherever this device keeps them. The store
/// records that a blob exists and whether it was uploaded; the bytes are the app's.
pub trait ImageByteSource: Send + Sync {
    fn bytes(&self, sha256: &str) -> Option<Vec<u8>>;
}

/// A sync that failed as a whole, rather than one op that will never work.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// Another sync is in flight. Refused rather than joined: a caller that has just
    /// written a row would otherwise get the report of a sync that started before it.
    #[error("a sync is already running")]
    AlreadyRunning,
    /// A push round trip changed nothing; continuing would spin forever. The counts
    /// tell apart a server acknowledging nothing, one rejecting in a shape this build
    /// cannot match, and one naming ops it was never sent.
    #[error(
        "sync stalled with {pending} changes waiting: the server accepted {accepted}, rejected {rejected}, \
         of which {unmatched_rejections} named no op it was sent"
    )]
    PushStalled {
        pending: usize,
        accepted: usize,
        rejected: usize,
        unmatched_rejections: usize,
    },
    #[error("transport: {0}")]
    Transport(#[from] SidecarError),
    #[error("store: {0}")]
    Store(#[from] StoreError),
}

impl SyncError {
    /// Whether the server simply has no sidecar — "your server can't do this yet"
    /// rather than "not right now".
    pub fn is_sidecar_absent(&self) -> bool {
        matches!(self, Self::Transport(error) if error.is_absent())
    }
}

/// A sync status panel's whole content, as plain values: render it, never compute
/// in the view. From [`SyncEngine::overview`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOverview {
    /// A sync is in flight now.
    pub running: bool,
    /// Where it has got to: pushing, pulling, failed with a reason, or idle.
    pub phase: SyncPhase,
    /// When a sync last finished cleanly on this device, across restarts.
    pub last_synced_at: Option<Millis>,
    /// When one was last started, in this run of the app.
    pub last_attempt_at: Option<Millis>,
    /// The last clean sync's counts: changes sent, received, and that changed a row.
    pub pushed: usize,
    pub pulled: usize,
    pub applied: usize,
    /// Changes waiting to be sent, all of them ("Waiting to send").
    pub waiting: i64,
    /// Changes set aside because they can never sync, inbound and outbound. Offer
    /// the list (`Store::quarantined_ops`) wherever this is not zero.
    pub quarantined: i64,
    /// Of those, the server faults worth offering a retry for.
    pub retryable: usize,
    /// Changes kept here because the server does not accept their kind yet
    /// (`track_flags` on an older sidecar). They go once it does.
    pub held_back: i64,
    /// Those kinds, by local table name.
    pub held_entities: Vec<String>,
    /// Edits held because a playlist's share was revoked; see `blocked_edits`.
    pub blocked_by_share: usize,
    pub blocked_edits: Vec<BlockedEdit>,
    /// Why the last sync failed, with the server's own words when it gave any. `None`
    /// after a clean sync.
    pub error: Option<String>,
    /// Why the last read of the share list failed.
    pub share_error: Option<String>,
    /// The last failure was the server having no sidecar at all: say "your server
    /// can't sync yet", not "try again".
    pub sidecar_absent: bool,
    /// The last clean sync's whole report, for a detail view.
    pub last_report: Option<SyncReport>,
}

/// A failure as a person should read it: the server's words, not only a status.
fn failure_text(error: &SyncError) -> String {
    match error {
        SyncError::Transport(transport @ SidecarError::Status { status, path, .. }) => {
            match transport.server_text() {
                Some(said) => format!("{path} returned HTTP {status}: {said}"),
                None => transport.to_string(),
            }
        }
        other => other.to_string(),
    }
}

type Clock = Arc<dyn Fn() -> Millis + Send + Sync>;
/// The bisection's result — `(quarantined, pushed)` — boxed because it recurses.
type Isolation<'a> = Pin<Box<dyn Future<Output = Result<(usize, usize), SyncError>> + Send + 'a>>;
type Observer = Arc<dyn Fn(&SyncPhase) + Send + Sync>;

struct EngineState {
    phase: SyncPhase,
    last_report: Option<SyncReport>,
    held_entities: BTreeSet<String>,
    blocked: BTreeMap<String, BlockedEdit>,
    shares: Option<Vec<PlaylistShare>>,
    shares_checked_at: Option<Millis>,
    share_error: Option<String>,
    lost_shares: BTreeSet<String>,
    last_attempt_at: Option<Millis>,
    last_error: Option<String>,
    sidecar_absent: bool,
}

/// Runs syncs for one store over one transport, one at a time.
///
/// **Never blocks sign-out on the network.** A sync is an ordinary future: dropping it
/// at any await point (a cancelled task) leaves the store consistent — every store
/// write is its own transaction, an op the server accepted but whose acknowledgement
/// was lost is simply pushed again and accepted again — and releases the engine for the
/// next sync. Nothing on the sign-out path (`Store::clear_queue`, claiming an account)
/// waits on the engine.
pub struct SyncEngine<T: SyncTransport> {
    store: Arc<Store>,
    transport: T,
    image_bytes: Option<Arc<dyn ImageByteSource>>,
    policy: SyncPolicy,
    clock: Clock,
    observer: Option<Observer>,
    user_id: Option<String>,
    running: AtomicBool,
    state: Mutex<EngineState>,
}

/// Clears the running flag however the sync ends — returned, failed, or dropped.
struct RunningGuard<'a>(&'a AtomicBool);

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl<T: SyncTransport> SyncEngine<T> {
    pub fn new(store: Arc<Store>, transport: T) -> Self {
        Self {
            store,
            transport,
            image_bytes: None,
            policy: SyncPolicy::default(),
            clock: Arc::new(now_millis),
            observer: None,
            user_id: None,
            running: AtomicBool::new(false),
            state: Mutex::new(EngineState {
                phase: SyncPhase::Idle,
                last_report: None,
                held_entities: BTreeSet::new(),
                blocked: BTreeMap::new(),
                shares: None,
                shares_checked_at: None,
                share_error: None,
                lost_shares: BTreeSet::new(),
                last_attempt_at: None,
                last_error: None,
                sidecar_absent: false,
            }),
        }
    }

    pub fn with_policy(mut self, policy: SyncPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// This device's clock, for skew judgement and quarantine times.
    pub fn with_clock(mut self, clock: impl Fn() -> Millis + Send + Sync + 'static) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    /// Called at every phase change: a first sync with years of history is many round
    /// trips, and a spinner that says nothing for two minutes reads as broken.
    pub fn on_phase_change(
        mut self,
        observer: impl Fn(&SyncPhase) + Send + Sync + 'static,
    ) -> Self {
        self.observer = Some(Arc::new(observer));
        self
    }

    /// Where cover bytes come from. Without one, covers are not uploaded.
    pub fn with_image_bytes(mut self, source: Arc<dyn ImageByteSource>) -> Self {
        self.image_bytes = Some(source);
        self
    }

    /// This user's Jellyfin id: what tells a share granted to this user from one this
    /// user granted. Without it only ownerless shares count as editable.
    pub fn with_user_id(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    /// Edits held back because a playlist's share was revoked — every one still
    /// held, not only those refused on the last sync. Show them, with the count, so
    /// nobody's change disappears without a word.
    pub fn blocked_edits(&self) -> Vec<BlockedEdit> {
        self.state().blocked.values().cloned().collect()
    }

    pub fn blocked_edit_count(&self) -> usize {
        self.state().blocked.len()
    }

    /// The share list as last read, if it has been.
    pub fn shares(&self) -> Option<Vec<PlaylistShare>> {
        self.state().shares.clone()
    }

    /// Why the last read of `/aoide/shares` failed, if it did.
    pub fn share_error(&self) -> Option<String> {
        self.state().share_error.clone()
    }

    /// Playlists shared **to** this user that a later read no longer lists: access
    /// was revoked. The contract says to drop them locally
    /// ([`aoide_store::Store::drop_playlist_locally`]); reported rather than done,
    /// so the app can say so first. Needs [`with_user_id`](Self::with_user_id).
    pub fn lost_shares(&self) -> BTreeSet<String> {
        self.state().lost_shares.clone()
    }

    /// Reads `/aoide/shares` now and lets go of every held edit it says may go out
    /// again. Syncs do this themselves at most every [`SHARE_RECHECK_MS`]; this is
    /// for a sharing screen that wants the list.
    pub async fn refresh_shares(&self) -> Result<Vec<PlaylistShare>, SidecarError> {
        self.state().shares_checked_at = Some((self.clock)());
        let read = self.transport.list_shares().await;
        let mut state = self.state();
        let shares = match read {
            Ok(shares) => shares,
            Err(error) => {
                // A server or transport without sharing is not a failure to report.
                let unsupported =
                    error.is_absent() || matches!(error, SidecarError::Unsupported(_));
                state.share_error = (!unsupported).then(|| error.to_string());
                return Err(error);
            }
        };
        state.share_error = None;
        let user = self.user_id.as_deref();
        let editable: BTreeSet<&str> = shares
            .iter()
            .filter(|share| share.grants_edit_to(user))
            .map(|share| share.playlist_id.as_str())
            .collect();
        let mut released = Vec::new();
        for edit in state.blocked.values_mut() {
            let Some(playlist) = edit.playlist_id.as_deref() else {
                continue;
            };
            let shared = editable.contains(playlist);
            edit.still_shared = Some(shared);
            if shared {
                released.push(edit.op_id.clone());
            }
        }
        for op_id in released {
            state.blocked.remove(&op_id);
        }
        if let (Some(user), Some(previous)) = (user, state.shares.as_ref()) {
            let granted_to_me = |list: &[PlaylistShare]| -> BTreeSet<String> {
                list.iter()
                    .filter(|share| {
                        share.grantee_user_id == user
                            && share.owner_user_id.as_deref() != Some(user)
                    })
                    .map(|share| share.playlist_id.clone())
                    .collect()
            };
            let now_granted = granted_to_me(&shares);
            let lost: Vec<String> = granted_to_me(previous)
                .into_iter()
                .filter(|playlist| !now_granted.contains(playlist))
                .collect();
            state.lost_shares.extend(lost);
            state
                .lost_shares
                .retain(|playlist| !now_granted.contains(playlist));
        }
        state.shares = Some(shares.clone());
        Ok(shares)
    }

    fn shares_due(&self) -> bool {
        let now = (self.clock)();
        self.state()
            .shares_checked_at
            .is_none_or(|checked| now - checked >= SHARE_RECHECK_MS)
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn phase(&self) -> SyncPhase {
        self.state().phase.clone()
    }

    pub fn last_report(&self) -> Option<SyncReport> {
        self.state().last_report.clone()
    }

    /// Local table names whose ops the last sync held back for want of server support.
    pub fn held_entities(&self) -> BTreeSet<String> {
        self.state().held_entities.clone()
    }

    fn state(&self) -> MutexGuard<'_, EngineState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn transition(&self, phase: SyncPhase) {
        self.state().phase = phase.clone();
        if let Some(observer) = &self.observer {
            observer(&phase);
        }
    }

    /// Runs one full sync: covers, then push, then pull.
    pub async fn sync(&self) -> Result<SyncReport, SyncError> {
        if self.running.swap(true, Ordering::AcqRel) {
            return Err(SyncError::AlreadyRunning);
        }
        let _guard = RunningGuard(&self.running);

        self.state().last_attempt_at = Some((self.clock)());
        match self.run().await {
            Ok(report) => {
                {
                    let mut state = self.state();
                    state.last_report = Some(report.clone());
                    state.last_error = None;
                    state.sidecar_absent = false;
                }
                // Best effort: a store that cannot write this has bigger news.
                let _ = self.store.set_last_synced_at((self.clock)());
                self.transition(SyncPhase::Idle);
                Ok(report)
            }
            Err(error) => {
                {
                    let mut state = self.state();
                    state.last_error = Some(failure_text(&error));
                    state.sidecar_absent = error.is_sidecar_absent();
                }
                self.transition(SyncPhase::Failed {
                    reason: error.to_string(),
                });
                Err(error)
            }
        }
    }

    /// Everything a sync status panel shows, read now. See [`SyncOverview`].
    pub fn overview(&self) -> Result<SyncOverview, StoreError> {
        let counts = self.store.pending_op_counts()?;
        let last_synced_at = self.store.last_synced_at()?;
        let quarantined = self.store.quarantine_count()?;
        let retryable = self.store.retryable_quarantine_count()?;
        let state = self.state();
        let held_back = counts
            .iter()
            .filter(|(table, _)| state.held_entities.contains(*table))
            .map(|(_, count)| *count)
            .sum();
        let report = state.last_report.clone().unwrap_or_default();
        Ok(SyncOverview {
            running: self.running.load(Ordering::Acquire),
            phase: state.phase.clone(),
            last_synced_at,
            last_attempt_at: state.last_attempt_at,
            pushed: report.pushed,
            pulled: report.pulled,
            applied: report.applied,
            waiting: counts.values().sum(),
            quarantined,
            retryable,
            held_back,
            held_entities: state.held_entities.iter().cloned().collect(),
            blocked_by_share: state.blocked.len(),
            blocked_edits: state.blocked.values().cloned().collect(),
            error: state.last_error.clone(),
            share_error: state.share_error.clone(),
            sidecar_absent: state.sidecar_absent,
            last_report: state.last_report.clone(),
        })
    }

    /// Pushes this device's waiting `queueState` op, and nothing else, so another
    /// device can pick up what is playing without waiting for the next full sync.
    /// Call it after [`aoide_store::QueueBroadcast`] writes a row. Refused with
    /// [`SyncError::AlreadyRunning`] while a sync runs — that sync pushes the row
    /// anyway. Returns how many ops the server acknowledged.
    pub async fn publish_queue(&self) -> Result<usize, SyncError> {
        if self.running.swap(true, Ordering::AcqRel) {
            return Err(SyncError::AlreadyRunning);
        }
        let _guard = RunningGuard(&self.running);
        let everything_else: BTreeSet<String> = SyncEntity::ALL
            .into_iter()
            .filter(|entity| *entity != SyncEntity::QueueState)
            .map(|entity| entity.local_table().to_owned())
            .collect();
        let pushed = self.push_pending(&everything_else).await;
        self.transition(SyncPhase::Idle);
        Ok(pushed?.pushed)
    }

    async fn run(&self) -> Result<SyncReport, SyncError> {
        // Before push, never after: an op naming a hash the server does not hold
        // leaves every other device with a cover it can never fetch.
        let uploaded = self.upload_pending_images().await?;
        // Before deciding what is pushable: a held edit is never pushed, so it can
        // never be refused again, and only this re-read can let it go.
        let mut shares_read = false;
        if self.shares_due() {
            let _ = self.refresh_shares().await;
            shares_read = true;
        }
        let held = self.entities_to_hold().await;
        self.state().held_entities = held.clone();
        let mut report = self.push_pending(&held).await?;
        report.images_uploaded = uploaded;
        if report.blocked_by_share > 0 && !shares_read {
            let _ = self.refresh_shares().await;
        }
        let pulled = self.pull_and_apply().await?;
        report.absorb(&pulled);
        Ok(report)
    }

    // MARK: - Push

    /// Which local tables must wait for the server, asked once per sync.
    async fn entities_to_hold(&self) -> BTreeSet<String> {
        let needing: Vec<SyncEntity> = SyncEntity::ALL
            .into_iter()
            .filter(|e| e.needs_server_support())
            .collect();
        if needing.is_empty() {
            return BTreeSet::new();
        }
        let accepted = self.transport.accepted_entities().await;
        needing
            .into_iter()
            .filter(|entity| {
                accepted
                    .as_ref()
                    .is_none_or(|accepted| !accepted.contains(entity.wire_name()))
            })
            .map(|entity| entity.local_table().to_owned())
            .collect()
    }

    /// Gets every cover named by a waiting op onto the server before any of them are
    /// pushed. Asked before sent: content addressing makes another device having
    /// uploaded the same bytes likely.
    async fn upload_pending_images(&self) -> Result<usize, SyncError> {
        let Some(bytes) = self
            .image_bytes
            .as_ref()
            .filter(|_| self.transport.supports_images())
        else {
            return Ok(0);
        };
        let mut uploaded = 0;
        for blob in self.store.images_needed_before_push()? {
            if self.transport.has_image(&blob.hash).await? {
                self.store.mark_image_uploaded(&blob.hash, (self.clock)())?;
                continue;
            }
            let Some(data) = bytes.bytes(&blob.hash) else {
                continue;
            };
            self.transport
                .upload_image(&blob.hash, data, &blob.mime)
                .await?;
            self.store.mark_image_uploaded(&blob.hash, (self.clock)())?;
            uploaded += 1;
        }
        Ok(uploaded)
    }

    async fn push_pending(&self, held: &BTreeSet<String>) -> Result<SyncReport, SyncError> {
        let mut report = SyncReport::default();
        // An edit whose op has left the log (its playlist dropped here) is held no more.
        let blocked: Vec<String> = self.state().blocked.keys().cloned().collect();
        if !blocked.is_empty() {
            let present = self.store.unsynced_op_ids(&blocked)?;
            self.state()
                .blocked
                .retain(|op_id, _| present.contains(op_id));
        }
        let mut previous_batch: BTreeSet<String> = BTreeSet::new();
        // What the previous round trip came back with, so a stall can say why.
        let mut last_exchange = (0, 0, 0);

        loop {
            // Held edits are the oldest unsynced rows, so they head every page; the
            // page is widened past them so they never starve what queued behind.
            let blocked: BTreeSet<String> = self.state().blocked.keys().cloned().collect();
            let pending: Vec<Op> = self
                .store
                .pending_ops(self.policy.push_batch_size + blocked.len(), held)?
                .into_iter()
                .filter(|op| !blocked.contains(&op.op_id))
                .take(self.policy.push_batch_size)
                .collect();
            if pending.is_empty() {
                break;
            }

            // The same batch twice means the last round trip changed nothing, and the
            // next would send the same bytes to the same effect.
            let ids: BTreeSet<String> = pending.iter().map(|op| op.op_id.clone()).collect();
            if ids == previous_batch {
                return Err(SyncError::PushStalled {
                    pending: pending.len(),
                    accepted: last_exchange.0,
                    rejected: last_exchange.1,
                    unmatched_rejections: last_exchange.2,
                });
            }
            previous_batch = ids;

            let mut wire = Vec::with_capacity(pending.len());
            for op in &pending {
                match WireOp::make(op) {
                    Ok(valid) => wire.push(valid),
                    Err(fault) => {
                        self.store.quarantine(op, &fault, (self.clock)())?;
                        report.quarantined_outbound += 1;
                    }
                }
            }
            // All invalid: they have left the queue, so the next fetch differs.
            if wire.is_empty() {
                continue;
            }

            let request = PushRequest {
                device_id: self.store.device_id().to_owned(),
                ops: wire,
            };
            let response = match self.transport.push(&request).await {
                Ok(response) => response,
                Err(error) if error.is_server_fault() => {
                    // The server threw on this batch. Sent again unchanged it throws
                    // again, and everything behind it waits; bisecting finds the one
                    // op that fails alone and lets the rest through.
                    let status = error.status().unwrap_or(500);
                    let (quarantined, pushed) = self
                        .isolate_server_fault(request.ops, &pending, status)
                        .await?;
                    report.quarantined_outbound += quarantined;
                    report.pushed += pushed;
                    previous_batch.clear();
                    continue;
                }
                Err(error) => return Err(error.into()),
            };

            // `accepted` includes ops the server had already seen.
            self.store.mark_synced(&response.accepted)?;
            report.pushed += response.accepted.len();

            let mut unmatched = 0;
            for rejection in &response.rejected {
                // A rejected op will never be accepted; it leaves the queue here.
                let Some(op) = pending.iter().find(|op| op.op_id == rejection.op_id) else {
                    unmatched += 1;
                    continue;
                };
                // The one refusal a restored share would turn into an acceptance:
                // held back and surfaced, never quarantined.
                if is_share_revoked(&rejection.reason) {
                    self.state().blocked.insert(
                        op.op_id.clone(),
                        BlockedEdit {
                            op_id: op.op_id.clone(),
                            playlist_id: playlist_of(op, &rejection.reason),
                            reason: rejection.reason.clone(),
                            still_shared: None,
                        },
                    );
                    report.blocked_by_share += 1;
                    continue;
                }
                self.store.quarantine(
                    op,
                    &SyncFault::ServerRejected(rejection.reason.clone()),
                    (self.clock)(),
                )?;
                report.quarantined_outbound += 1;
                report.rejected += 1;
            }
            last_exchange = (response.accepted.len(), response.rejected.len(), unmatched);

            // The response's cursor is deliberately dropped: it is the server's head,
            // not this device's read position.
            self.transition(SyncPhase::Pushing {
                pushed: report.pushed,
            });
        }
        Ok(report)
    }

    /// Halves the failing batch until the op that fails *by itself* is found, and
    /// quarantines that one. Halves that succeed on the way are genuinely pushed. A half
    /// that stops failing when sent alone was a transient fault and blames nobody.
    fn isolate_server_fault<'a>(
        &'a self,
        wire: Vec<WireOp>,
        pending: &'a [Op],
        status: u16,
    ) -> Isolation<'a> {
        Box::pin(async move {
            if wire.len() == 1 {
                let Some(op) = pending.iter().find(|op| op.op_id == wire[0].op_id) else {
                    return Ok((0, 0));
                };
                self.store
                    .quarantine(op, &SyncFault::ServerFaulted { status }, (self.clock)())?;
                return Ok((1, 0));
            }
            let mut quarantined = 0;
            let mut pushed = 0;
            let middle = wire.len() / 2;
            let mut halves = wire;
            let second = halves.split_off(middle);
            for half in [halves, second] {
                if half.is_empty() {
                    continue;
                }
                let request = PushRequest {
                    device_id: self.store.device_id().to_owned(),
                    ops: half,
                };
                match self.transport.push(&request).await {
                    Ok(response) => {
                        self.store.mark_synced(&response.accepted)?;
                        pushed += response.accepted.len();
                    }
                    Err(error) if error.is_server_fault() => {
                        let status = error.status().unwrap_or(500);
                        let (inner_quarantined, inner_pushed) = self
                            .isolate_server_fault(request.ops, pending, status)
                            .await?;
                        quarantined += inner_quarantined;
                        pushed += inner_pushed;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Ok((quarantined, pushed))
        })
    }

    // MARK: - Pull

    async fn pull_and_apply(&self) -> Result<SyncReport, SyncError> {
        let mut report = SyncReport::default();
        let mut cursor = self.store.pull_cursor()?;

        loop {
            let response = self.transport.pull(cursor, self.policy.pull_limit).await?;

            // An empty page teaches nothing verifiable, so its cursor is not stored;
            // advancing on one is the one way left to skip an op. A page of nothing but
            // unreadable entries is not empty: its cursor has to clear them.
            if response.ops.is_empty() && response.unreadable_ops == 0 {
                break;
            }

            let mut applied =
                self.store
                    .apply(&response.ops, response.cursor, self.policy, (self.clock)())?;
            applied.unreadable = response.unreadable_ops;
            applied.pulled += response.unreadable_ops;
            report.absorb(&applied);
            self.transition(SyncPhase::Pulling {
                applied: report.applied,
            });

            // Applied before this check, because applying is idempotent; looping on a
            // cursor that does not advance would ask the same question forever.
            if cursor.is_some_and(|cursor| response.cursor <= cursor) {
                break;
            }
            cursor = Some(response.cursor);
            if !response.has_more {
                break;
            }
        }
        Ok(report)
    }
}
