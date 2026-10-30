//! Ports of CurationKit's engine suites (`SyncEngineTests`, `SyncIntegrationTests`,
//! `TrackFlagsTests` "Holding entities…"), of MeleteKit's `SyncEndpointTests` where
//! they apply, and of `NextChooserTests` / `DiscoverTests`, plus the HTTP client
//! against a local mock server. Nothing here touches the network.

mod covers;
mod end_to_end;
mod handoff;
mod held_entities;
mod http;
mod import;
mod live;
mod next_discover;
mod overview;
mod shares;
mod sidecar_stub;
mod sync_engine;
mod sync_integration;

use std::collections::BTreeSet;
use std::sync::Arc;

use aoide_store::rusqlite::Connection;
use aoide_store::{
    Folder, FractionalIndex, Like, Millis, Op, PlayEvent, Playlist, PlaylistItem, QueueState,
    Record, Store, canonical_json,
};
use serde::Serialize;
use serde_json::Value;

pub(crate) fn store(device: &str) -> Arc<Store> {
    Arc::new(Store::in_memory(device).expect("in-memory store"))
}

pub(crate) fn pending(store: &Store) -> Vec<Op> {
    store.pending_ops(500, &BTreeSet::new()).expect("pending")
}

pub(crate) fn pending_all(store: &Store) -> Vec<Op> {
    store.pending_ops(5_000, &BTreeSet::new()).expect("pending")
}

pub(crate) fn make_playlist(id: &str, name: &str, updated_at: Millis, device: &str) -> Playlist {
    let mut list = Playlist::new(name, FractionalIndex::initial(), updated_at, device);
    list.id = id.to_owned();
    list
}

/// A row as the store encodes it, parsed — the exact shape of a payload on the wire.
pub(crate) fn payload<T: Serialize>(row: &T) -> Value {
    serde_json::to_value(row).expect("encode")
}

pub(crate) fn count<R: Record>(store: &Store) -> i64 {
    store.read(R::count).expect("count")
}

/// Every syncable row, canonically ordered and fully encoded: the assertion that two
/// devices hold the same bytes, deleted rows included. The per-device tables (`track`,
/// `op`, `quarantinedOp`, `syncState`) are deliberately absent.
pub(crate) fn synced_rows(store: &Store) -> String {
    fn encoded<R: Record + Serialize>(
        conn: &Connection,
        lines: &mut Vec<String>,
    ) -> aoide_store::rusqlite::Result<()> {
        let mut rows: Vec<String> = R::fetch_all(conn)?
            .iter()
            .map(|row| {
                format!(
                    "{} {}",
                    R::TABLE,
                    canonical_json(&serde_json::to_value(row).expect("encode"))
                )
            })
            .collect();
        rows.sort();
        lines.extend(rows);
        Ok(())
    }
    store
        .read(|conn| {
            let mut lines = Vec::new();
            encoded::<Folder>(conn, &mut lines)?;
            encoded::<Playlist>(conn, &mut lines)?;
            encoded::<PlaylistItem>(conn, &mut lines)?;
            encoded::<Like>(conn, &mut lines)?;
            encoded::<PlayEvent>(conn, &mut lines)?;
            encoded::<QueueState>(conn, &mut lines)?;
            Ok(lines.join("\n"))
        })
        .expect("synced rows")
}

/// How many ops this device has queued or sent, ever — the number that must not grow
/// when a device merely applies somebody else's changes.
pub(crate) fn total_op_count(store: &Store) -> i64 {
    count::<Op>(store)
}

/// Re-queues every op ever recorded, as though the acknowledgements had been lost.
pub(crate) fn forget_acknowledgements(store: &Store) {
    store
        .write_local(|tx| tx.execute("UPDATE op SET synced = 0", []).map(|_| ()))
        .expect("forget");
}

pub(crate) fn names(store: &Store) -> Vec<String> {
    store
        .playlists()
        .expect("playlists")
        .into_iter()
        .map(|p| p.name)
        .collect()
}

pub(crate) fn item_ids(store: &Store, playlist_id: &str) -> Vec<String> {
    store
        .items(playlist_id)
        .expect("items")
        .into_iter()
        .map(|i| i.jellyfin_id)
        .collect()
}
