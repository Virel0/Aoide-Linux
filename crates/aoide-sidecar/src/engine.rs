//! The sync loop: push everything pending, then read everything new.
//!
//! Push before pull, always. A device's own ops come back down with a sequence number,
//! which is how it learns they are durable — and pulling first would merge a server
//! state missing the local changes.

use std::collections::BTreeSet;
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

type Clock = Arc<dyn Fn() -> Millis + Send + Sync>;
/// The bisection's result — `(quarantined, pushed)` — boxed because it recurses.
type Isolation<'a> = Pin<Box<dyn Future<Output = Result<(usize, usize), SyncError>> + Send + 'a>>;
type Observer = Arc<dyn Fn(&SyncPhase) + Send + Sync>;

struct EngineState {
    phase: SyncPhase,
    last_report: Option<SyncReport>,
    held_entities: BTreeSet<String>,
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
            running: AtomicBool::new(false),
            state: Mutex::new(EngineState {
                phase: SyncPhase::Idle,
                last_report: None,
                held_entities: BTreeSet::new(),
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

        match self.run().await {
            Ok(report) => {
                self.state().last_report = Some(report.clone());
                self.transition(SyncPhase::Idle);
                Ok(report)
            }
            Err(error) => {
                self.transition(SyncPhase::Failed {
                    reason: error.to_string(),
                });
                Err(error)
            }
        }
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
        let held = self.entities_to_hold().await;
        self.state().held_entities = held.clone();
        let mut report = self.push_pending(&held).await?;
        report.images_uploaded = uploaded;
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
        let mut previous_batch: BTreeSet<String> = BTreeSet::new();
        // What the previous round trip came back with, so a stall can say why.
        let mut last_exchange = (0, 0, 0);

        loop {
            let pending = self.store.pending_ops(self.policy.push_batch_size, held)?;
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
