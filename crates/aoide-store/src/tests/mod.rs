//! Ports of CurationKit's test suites, which are the specification for this crate.
//! Each file names the Swift suite it ports.

mod curation_store;
mod field_merge;
mod fractional_index;
mod play_definition;
mod play_history;
mod playlists;
mod queue_persistence;
mod recent_contexts;
mod smart_playlist;
mod sync_account;
mod sync_entity;
mod sync_merge;
mod sync_wire;
mod taste_parity;
mod taste_profile;
mod track_cache;
mod track_flags;

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use crate::record::Record;
use crate::{
    FractionalIndex, Millis, Op, OpKind, Playlist, PullCursor, PulledOp, Store, SyncEntity,
    SyncPolicy, SyncReport, TrackLink, canonical_json, new_id,
};

pub(crate) fn store() -> Store {
    Store::in_memory("phone").expect("in-memory store")
}

/// `pendingOps()` with the phone's default page of 500.
pub(crate) fn pending(store: &Store) -> Vec<Op> {
    store
        .pending_ops(500, &BTreeSet::new())
        .expect("pending ops")
}

pub(crate) fn links(ids: &[&str]) -> Vec<TrackLink> {
    ids.iter()
        .map(|id| TrackLink::new(*id, format!("key-{id}")))
        .collect()
}

pub(crate) fn count<R: Record>(store: &Store) -> i64 {
    store.read(R::count).expect("count")
}

pub(crate) fn all<R: Record>(store: &Store) -> Vec<R> {
    store.read(R::fetch_all).expect("fetch all")
}

pub(crate) fn fetch<R: Record>(store: &Store, key: &str) -> Option<R> {
    store.read(|conn| R::fetch(conn, key)).expect("fetch")
}

pub(crate) fn make_playlist(
    id: &str,
    name: &str,
    updated_at: Millis,
    deleted: bool,
    device: &str,
) -> Playlist {
    let mut list = Playlist::new(name, FractionalIndex::initial(), updated_at, device);
    list.id = id.to_owned();
    list.deleted = deleted;
    list
}

/// A row encoded the way the store encodes it, then parsed — the exact shape a payload
/// has on the wire.
pub(crate) fn payload<T: Serialize>(row: &T) -> Value {
    serde_json::from_str(&crate::json::payload(row).expect("encode")).expect("parse")
}

pub(crate) fn text(value: &Value) -> String {
    canonical_json(value)
}

/// An op as it comes back from a pull, built the way the phone's `Wire.pulled` is.
pub(crate) struct Pulled {
    entity: SyncEntity,
    payload: Value,
    entity_id: String,
    operation: OpKind,
    op_id: String,
    created_at: Millis,
    seq: i64,
    device_id: String,
    received_at: Millis,
}

impl Pulled {
    pub fn new<T: Serialize>(entity: SyncEntity, record: &T, entity_id: &str) -> Self {
        Self {
            entity,
            payload: payload(record),
            entity_id: entity_id.to_owned(),
            operation: OpKind::Upsert,
            op_id: new_id(),
            created_at: 1_000,
            seq: 1,
            device_id: "mac".to_owned(),
            received_at: 1_000,
        }
    }

    pub fn operation(mut self, operation: OpKind) -> Self {
        self.operation = operation;
        self
    }

    pub fn seq(mut self, seq: i64) -> Self {
        self.seq = seq;
        self
    }

    pub fn op_id(mut self, op_id: &str) -> Self {
        self.op_id = op_id.to_owned();
        self
    }

    pub fn created_at(mut self, created_at: Millis) -> Self {
        self.created_at = created_at;
        self
    }

    pub fn device(mut self, device_id: &str) -> Self {
        self.device_id = device_id.to_owned();
        self
    }

    pub fn received_at(mut self, received_at: Millis) -> Self {
        self.received_at = received_at;
        self
    }

    pub fn op(self) -> PulledOp {
        PulledOp {
            op_id: self.op_id,
            entity: self.entity.wire_name().to_owned(),
            entity_id: self.entity_id,
            operation: self.operation.as_str().to_owned(),
            payload: self.payload,
            created_at: self.created_at,
            seq: self.seq,
            device_id: self.device_id,
            received_at: self.received_at,
            author_user_id: None,
        }
    }
}

/// An op whose payload is arbitrary JSON, for the cases a real record cannot produce.
pub(crate) fn pulled_raw(
    entity: &str,
    entity_id: &str,
    payload: Value,
    operation: &str,
    seq: i64,
) -> PulledOp {
    PulledOp {
        op_id: new_id(),
        entity: entity.to_owned(),
        entity_id: entity_id.to_owned(),
        operation: operation.to_owned(),
        payload,
        created_at: 1_000,
        seq,
        device_id: "mac".to_owned(),
        received_at: 1_000,
        author_user_id: None,
    }
}

/// Applies one op through the real batch path, at the phone's test clock of 5000.
pub(crate) fn apply_one(store: &Store, op: PulledOp) -> SyncReport {
    apply_one_at(store, op, 5_000)
}

pub(crate) fn apply_one_at(store: &Store, op: PulledOp, now: Millis) -> SyncReport {
    let cursor = PullCursor(op.seq);
    store
        .apply(&[op], cursor, SyncPolicy::default(), now)
        .expect("apply")
}
