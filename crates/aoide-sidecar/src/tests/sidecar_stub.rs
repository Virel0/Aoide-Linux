//! The sidecar, in memory, and deliberately as dumb as the real one — a port of
//! `SidecarStub.swift` with the knobs of `FakeSyncServer` folded in.
//!
//! Faithful rather than permissive: it refuses exactly what the contract says the
//! server refuses, so a sync that ends with an empty `rejected` list is evidence the
//! client's own validation agrees. It interprets nothing: payloads are stored verbatim,
//! so the invariants left to the client are ones it will happily let through. One log,
//! one global sequence, many users — which is what makes the push cursor a real trap.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use aoide_store::{
    Millis, PullCursor, PullResponse, PulledOp, PushCursor, PushRequest, PushResponse, RejectedOp,
    WireOp,
};

use crate::{SidecarError, SyncTransport};

#[derive(Debug, Clone)]
pub(crate) struct Row {
    pub seq: i64,
    pub user_id: String,
    pub op: WireOp,
    pub device_id: String,
    pub received_at: Millis,
}

struct State {
    log: Vec<Row>,
    push_count: usize,
    pull_count: usize,
    largest_batch_pushed: usize,
    pull_limits_requested: Vec<usize>,
    pushes_by_device: HashMap<String, usize>,
    calls: Vec<&'static str>,
    last_pushed_device: Option<String>,
    next_seq: i64,
    seen: HashSet<(String, String)>,
    clock: Millis,
    forced_rejections: HashMap<String, String>,
    fail_push_calls: HashSet<usize>,
    fail_pull_calls: HashSet<usize>,
    offline_for_next_push: bool,
    pull_delay: Duration,
    extra_entities: HashSet<String>,
    poison: HashSet<String>,
    clear_poison_after_failure: bool,
    acknowledges_nothing: bool,
    accepts_nothing: bool,
    rejects_with_strange_ids: bool,
}

pub(crate) struct Sidecar {
    state: Mutex<State>,
}

/// Spelled out literally rather than derived from `SyncEntity`: a double sharing the
/// mapping under test could never catch the mapping being wrong.
const ALLOWED_ENTITIES: [&str; 6] = [
    "playlists",
    "playlist_items",
    "folders",
    "likes",
    "play_events",
    "queue_state",
];

/// Why the server refuses an op, or `None`. Contents are never examined — only that
/// the payload is an object and how big it is.
pub(crate) fn refusal(op: &WireOp, extra: &HashSet<String>) -> Option<String> {
    let op_id_length = op.op_id.chars().count();
    let entity_id_length = op.entity_id.chars().count();
    if op.op_id.is_empty() {
        return Some("opId must not be empty".into());
    }
    if op_id_length > 128 {
        return Some(format!(
            "opId is {op_id_length} characters, over the 128 limit"
        ));
    }
    if op.entity_id.is_empty() {
        return Some("entityId must not be empty".into());
    }
    if entity_id_length > 256 {
        return Some(format!(
            "entityId is {entity_id_length} characters, over the 256 limit"
        ));
    }
    if !ALLOWED_ENTITIES.contains(&op.entity.as_str()) && !extra.contains(&op.entity) {
        return Some(format!("unknown entity '{}'", op.entity));
    }
    if op.operation != "upsert" && op.operation != "delete" {
        return Some(format!("unknown operation '{}'", op.operation));
    }
    if !op.payload.is_object() {
        return Some("payload must be a JSON object".into());
    }
    if op.created_at <= 0 {
        return Some("createdAt must be a positive millisecond timestamp".into());
    }
    let bytes = serde_json::to_vec(&op.payload).map_or(usize::MAX, |b| b.len());
    if bytes > 256 * 1024 {
        return Some(format!("payload is {bytes} bytes, over the 256 KB limit"));
    }
    None
}

impl Sidecar {
    /// Receipt time defaults to a plausible value; every test that cares sets it.
    pub fn new() -> Arc<Self> {
        Self::with_clock(2_000_000_000_000)
    }

    pub fn with_clock(clock: Millis) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                log: Vec::new(),
                push_count: 0,
                pull_count: 0,
                largest_batch_pushed: 0,
                pull_limits_requested: Vec::new(),
                pushes_by_device: HashMap::new(),
                calls: Vec::new(),
                last_pushed_device: None,
                next_seq: 1,
                seen: HashSet::new(),
                clock,
                forced_rejections: HashMap::new(),
                fail_push_calls: HashSet::new(),
                fail_pull_calls: HashSet::new(),
                offline_for_next_push: false,
                pull_delay: Duration::ZERO,
                extra_entities: HashSet::new(),
                poison: HashSet::new(),
                clear_poison_after_failure: false,
                acknowledges_nothing: false,
                accepts_nothing: false,
                rejects_with_strange_ids: false,
            }),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    /// A transport bound to one user, as the token binds a real one.
    pub fn link(self: &Arc<Self>, user: &str) -> SidecarLink {
        SidecarLink {
            sidecar: Arc::clone(self),
            user_id: user.to_owned(),
        }
    }

    // MARK: Knobs

    pub fn set_clock(&self, value: Millis) {
        self.state().clock = value;
    }

    /// Burns sequence numbers, the way a rolled-back transaction does.
    pub fn burn_sequences(&self, count: i64) {
        self.state().next_seq += count;
    }

    /// Refuses one op permanently.
    pub fn reject(&self, op_id: &str, reason: &str) {
        self.state()
            .forced_rejections
            .insert(op_id.to_owned(), reason.to_owned());
    }

    pub fn fail_push(&self, call: usize) {
        self.state().fail_push_calls.insert(call);
    }

    pub fn fail_pull(&self, call: usize) {
        self.state().fail_pull_calls.insert(call);
    }

    pub fn go_offline_for_next_push(&self) {
        self.state().offline_for_next_push = true;
    }

    pub fn set_pull_delay(&self, delay: Duration) {
        self.state().pull_delay = delay;
    }

    /// Widens the allow-list, standing in for a server deployed ahead of this build.
    pub fn also_allow(&self, entity: &str) {
        self.state().extra_entities.insert(entity.to_owned());
    }

    /// Op ids the server throws a 500 on, taking the whole request with them.
    pub fn poison(&self, op_id: &str) {
        self.state().poison.insert(op_id.to_owned());
    }

    pub fn unpoison(&self, op_id: &str) {
        self.state().poison.remove(op_id);
    }

    /// The next request throws and everything after it succeeds: a restart.
    pub fn clear_poison_after_next_failure(&self) {
        self.state().clear_poison_after_failure = true;
    }

    /// Stores what it is sent and answers 200 acknowledging nothing.
    pub fn stop_acknowledging(&self) {
        self.state().acknowledges_nothing = true;
    }

    /// Answers 200 without storing or acknowledging anything (`FakeSyncServer`).
    pub fn accept_nothing(&self) {
        self.state().accepts_nothing = true;
    }

    /// Rejects using ids it was never sent.
    pub fn reject_with_unknown_ids(&self) {
        self.state().rejects_with_strange_ids = true;
    }

    /// Puts ops in the log as if another device had pushed them (`FakeSyncServer.seed`).
    pub fn seed(&self, user: &str, ops: Vec<WireOp>, device: &str) {
        let mut state = self.state();
        for op in ops {
            if !state.seen.insert((user.to_owned(), op.op_id.clone())) {
                continue;
            }
            let row = Row {
                seq: state.next_seq,
                user_id: user.to_owned(),
                op,
                device_id: device.to_owned(),
                received_at: state.clock,
            };
            state.log.push(row);
            state.next_seq += 1;
        }
    }

    // MARK: Observation

    /// The head sequence — what a push reports back.
    pub fn head(&self) -> i64 {
        self.state().next_seq - 1
    }

    pub fn ops_for(&self, user: &str) -> Vec<Row> {
        self.state()
            .log
            .iter()
            .filter(|row| row.user_id == user)
            .cloned()
            .collect()
    }

    pub fn op_count(&self, user: &str) -> usize {
        self.ops_for(user).len()
    }

    pub fn entities_for(&self, user: &str) -> Vec<String> {
        self.ops_for(user)
            .into_iter()
            .map(|row| row.op.entity)
            .collect()
    }

    pub fn push_count(&self) -> usize {
        self.state().push_count
    }

    pub fn pull_count(&self) -> usize {
        self.state().pull_count
    }

    pub fn largest_batch_pushed(&self) -> usize {
        self.state().largest_batch_pushed
    }

    pub fn pull_limits_requested(&self) -> Vec<usize> {
        self.state().pull_limits_requested.clone()
    }

    pub fn pushes_by(&self, device: &str) -> Option<usize> {
        self.state().pushes_by_device.get(device).copied()
    }

    pub fn calls(&self) -> Vec<&'static str> {
        self.state().calls.clone()
    }

    pub fn last_pushed_device(&self) -> Option<String> {
        self.state().last_pushed_device.clone()
    }

    // MARK: Transport

    pub fn push(&self, request: &PushRequest, user: &str) -> Result<PushResponse, SidecarError> {
        let mut state = self.state();
        state.push_count += 1;
        state.calls.push("push");
        *state
            .pushes_by_device
            .entry(request.device_id.clone())
            .or_default() += 1;
        state.largest_batch_pushed = state.largest_batch_pushed.max(request.ops.len());
        state.last_pushed_device = Some(request.device_id.clone());

        let call = state.push_count;
        if state.fail_push_calls.contains(&call) {
            return Err(SidecarError::Network("unreachable".into()));
        }
        if std::mem::take(&mut state.offline_for_next_push) {
            return Err(SidecarError::Network("offline".into()));
        }
        if request
            .ops
            .iter()
            .any(|op| state.poison.contains(&op.op_id))
        {
            if std::mem::take(&mut state.clear_poison_after_failure) {
                state.poison.clear();
            }
            return Err(SidecarError::Status {
                status: 500,
                path: "aoide/sync/push".into(),
                detail: Some("Error processing request.".into()),
            });
        }
        // Over a thousand is a 400 for the whole request, valid ops included.
        if request.ops.len() > 1_000 {
            return Err(SidecarError::Status {
                status: 400,
                path: "aoide/sync/push".into(),
                detail: Some(format!(
                    "batch of {} ops is over the 1000 limit",
                    request.ops.len()
                )),
            });
        }
        if state.accepts_nothing {
            return Ok(PushResponse {
                accepted: vec![],
                rejected: vec![],
                cursor: Some(PushCursor(state.next_seq - 1)),
            });
        }

        let mut accepted = Vec::new();
        let mut rejected = Vec::new();
        for op in &request.ops {
            let reason = state
                .forced_rejections
                .get(&op.op_id)
                .cloned()
                .or_else(|| refusal(op, &state.extra_entities));
            if let Some(reason) = reason {
                rejected.push(RejectedOp {
                    op_id: op.op_id.clone(),
                    reason,
                });
                continue;
            }
            // Already held is accepted and ignored: what makes a timed-out push safe.
            if !state.seen.insert((user.to_owned(), op.op_id.clone())) {
                accepted.push(op.op_id.clone());
                continue;
            }
            let row = Row {
                seq: state.next_seq,
                user_id: user.to_owned(),
                op: op.clone(),
                device_id: request.device_id.clone(),
                received_at: state.clock,
            };
            state.log.push(row);
            state.next_seq += 1;
            accepted.push(op.op_id.clone());
        }

        let head = Some(PushCursor(state.next_seq - 1));
        if state.acknowledges_nothing {
            return Ok(PushResponse {
                accepted: vec![],
                rejected: vec![],
                cursor: head,
            });
        }
        if state.rejects_with_strange_ids {
            let rejected = request
                .ops
                .iter()
                .map(|op| RejectedOp {
                    op_id: format!("not-{}", op.op_id),
                    reason: "nope".into(),
                })
                .collect();
            return Ok(PushResponse {
                accepted: vec![],
                rejected,
                cursor: head,
            });
        }
        Ok(PushResponse {
            accepted,
            rejected,
            cursor: head,
        })
    }

    pub fn pull(
        &self,
        since: Option<PullCursor>,
        limit: usize,
        user: &str,
    ) -> Result<PullResponse, SidecarError> {
        let mut state = self.state();
        state.pull_count += 1;
        state.calls.push("pull");
        state.pull_limits_requested.push(limit);
        let call = state.pull_count;
        if state.fail_pull_calls.contains(&call) {
            return Err(SidecarError::Network("unreachable".into()));
        }

        let effective = limit.clamp(1, 1_000);
        let since = since.map_or(0, |cursor| cursor.0);
        // One row past the limit, which is what makes `has_more` exact.
        let window: Vec<&Row> = state
            .log
            .iter()
            .filter(|row| row.user_id == user && row.seq > since)
            .take(effective + 1)
            .collect();
        let page: Vec<&Row> = window.iter().take(effective).copied().collect();
        let ops = page
            .iter()
            .map(|row| PulledOp {
                op_id: row.op.op_id.clone(),
                entity: row.op.entity.clone(),
                entity_id: row.op.entity_id.clone(),
                operation: row.op.operation.clone(),
                payload: row.op.payload.clone(),
                created_at: row.op.created_at,
                seq: row.seq,
                device_id: row.device_id.clone(),
                received_at: row.received_at,
                author_user_id: Some(row.user_id.clone()),
            })
            .collect();
        // "Everything up to here", not "the next number".
        let cursor = PullCursor(page.last().map_or(since, |row| row.seq));
        Ok(PullResponse {
            ops,
            cursor,
            has_more: window.len() > effective,
            unreadable_ops: 0,
        })
    }

    fn pull_delay(&self) -> Duration {
        self.state().pull_delay
    }
}

/// One authenticated device's view of the sidecar.
#[derive(Clone)]
pub(crate) struct SidecarLink {
    pub sidecar: Arc<Sidecar>,
    pub user_id: String,
}

impl SyncTransport for SidecarLink {
    async fn push(&self, request: &PushRequest) -> Result<PushResponse, SidecarError> {
        self.sidecar.push(request, &self.user_id)
    }

    async fn pull(
        &self,
        since: Option<PullCursor>,
        limit: usize,
    ) -> Result<PullResponse, SidecarError> {
        let delay = self.sidecar.pull_delay();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        self.sidecar.pull(since, limit, &self.user_id)
    }
}
