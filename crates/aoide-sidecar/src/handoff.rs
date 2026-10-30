//! Which other device's queue to offer: the server's answer first, the op log's rows
//! when the server cannot be asked.
//!
//! `GET /aoide/queue` is asked first because only the server can say how recent a row
//! is without trusting the clock of the device that wrote it — a machine set wrong
//! would otherwise claim to be the most recent one forever and win every handover.
//! The rows the op log already brought ([`Store::local_resumable_queues`]) still work
//! with no network, ordered by the writers' own clocks.
//!
//! Publishing this device's queue is [`aoide_store::QueueBroadcast`] (debounced row
//! writes) plus [`crate::SyncEngine::publish_queue`] (push just that row now).

use std::future::Future;

use aoide_store::{Millis, QueueFreshness, QueuePolicy, ResumableQueue, Store, StoreError};

use crate::client::SidecarClient;
use crate::error::SidecarError;
use crate::types::QueueEntry;

/// Where the server's view of every device's queue comes from — Melete, or a
/// stand-in in tests.
pub trait QueueSource: Send + Sync {
    /// Every device's latest queue row, as `GET /aoide/queue` lists them.
    fn queues(&self) -> impl Future<Output = Result<Vec<QueueEntry>, SidecarError>> + Send;
}

impl QueueSource for SidecarClient {
    fn queues(&self) -> impl Future<Output = Result<Vec<QueueEntry>, SidecarError>> + Send {
        SidecarClient::queues(self)
    }
}

/// Other devices' queues worth offering, newest first, never this device's, never an
/// empty one, none older than [`QueuePolicy::RESUME_WINDOW`].
///
/// From `source` when it answers — even with nothing, since the server holds the
/// latest row of every device — and from the store's rows when it fails or there is
/// no `source` (signed in to a server without these routes).
///
/// # Errors
///
/// [`StoreError`] only from the store's fallback rows, read when `source` is `None`
/// or fails; a failing `source` is not an error.
pub async fn resumable_queues<S: QueueSource>(
    source: Option<&S>,
    store: &Store,
    now: Millis,
) -> Result<Vec<ResumableQueue>, StoreError> {
    if let Some(source) = source
        && let Ok(entries) = source.queues().await
    {
        return Ok(queues_from_server(&entries, store.device_id(), now));
    }
    store.local_resumable_queues(now)
}

/// The server's entries as queues to offer. Ordered by one criterion for the whole
/// list: by age when every entry carries one, otherwise by receipt time — mixing the
/// two would compare seconds against milliseconds. An entry with no server age is
/// aged on its writer's clock and marked so.
pub fn queues_from_server(
    entries: &[QueueEntry],
    this_device: &str,
    now: Millis,
) -> Vec<ResumableQueue> {
    let mut others: Vec<&QueueEntry> = entries
        .iter()
        .filter(|entry| {
            !entry.is_current_device
                && entry.device_id != this_device
                && !entry.track_ids.is_empty()
        })
        .collect();
    let by_age = others.iter().all(|entry| entry.age_seconds.is_some());
    others.sort_by(|a, b| {
        if by_age {
            a.age_seconds
                .unwrap_or(f64::MAX)
                .total_cmp(&b.age_seconds.unwrap_or(f64::MAX))
        } else {
            b.received_at.unwrap_or(0).cmp(&a.received_at.unwrap_or(0))
        }
    });
    others
        .into_iter()
        .map(|entry| {
            let (age_ms, freshness) = match entry.age_seconds {
                Some(seconds) => (
                    (seconds.max(0.0) * 1000.0) as Millis,
                    QueueFreshness::Server,
                ),
                None => (
                    now.saturating_sub(entry.updated_at.unwrap_or(0)).max(0),
                    QueueFreshness::Writer,
                ),
            };
            ResumableQueue {
                device_id: entry.device_id.clone(),
                device_name: entry
                    .device_name
                    .clone()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| "Another device".to_owned()),
                position: clamp(entry.position, entry.track_ids.len()),
                track_ids: entry.track_ids.clone(),
                elapsed_ms: entry.elapsed_ms.max(0),
                age_ms,
                observed_at: now,
                freshness,
            }
        })
        .filter(|queue| queue.age_ms <= QueuePolicy::RESUME_WINDOW)
        .collect()
}

fn clamp(position: i64, len: usize) -> usize {
    usize::try_from(position.max(0))
        .unwrap_or(usize::MAX)
        .min(len.saturating_sub(1))
}
