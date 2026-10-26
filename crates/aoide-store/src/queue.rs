//! Queue persistence, and the read side of handing playback between devices.
//!
//! `queueState` is the one syncable table that is not a `SyncRecord`: keyed by the
//! device, with no `id` and no `deleted`, and only ever overwritten by the device that
//! owns it. So the writes here open the transaction themselves and put the row and its
//! op inside it — the property `record` exists to guarantee.

use rusqlite::params;

use crate::Millis;
use crate::error::Result;
use crate::json;
use crate::models::{Op, OpKind, QueueState};
use crate::record::Record;
use crate::store::Store;

/// The limits that keep queue persistence cheap.
pub struct QueuePolicy;

impl QueuePolicy {
    /// How long another device's queue stays worth offering as a resume: evening on
    /// the phone, next morning in the car. A rule about what to offer; rows are never
    /// trimmed by age.
    pub const RESUME_WINDOW: Millis = 24 * 60 * 60 * 1000;
    /// The most track ids ever written to a queue row.
    pub const MAX_TRACKS: usize = 500;
    /// The most already-played tracks kept behind the current position.
    pub const MAX_HISTORY: usize = 25;
    /// Elapsed time must move by more than this for a save to be worth doing.
    pub const ELAPSED_THRESHOLD_MS: i64 = 5_000;

    /// The slice of `track_ids` that is persisted, with `position` rebased into it.
    pub fn window(track_ids: &[String], position: i64) -> (Vec<String>, i64) {
        if track_ids.is_empty() {
            return (Vec::new(), 0);
        }
        // A position past the end means the queue ran out; the last track is the only
        // reading of that which is playable.
        let last = track_ids.len() - 1;
        let index = usize::try_from(position.max(0))
            .unwrap_or(usize::MAX)
            .min(last);
        if track_ids.len() <= Self::MAX_TRACKS {
            return (track_ids.to_vec(), index as i64);
        }
        // Slide the window back from the end so the cap is spent on history that
        // exists, not slots past the last track.
        let start = index
            .saturating_sub(Self::MAX_HISTORY)
            .min(track_ids.len() - Self::MAX_TRACKS);
        (
            track_ids[start..start + Self::MAX_TRACKS].to_vec(),
            (index - start) as i64,
        )
    }
}

impl Store {
    /// This device's saved queue. A signed-out device keeps a row with an empty list,
    /// so check the list, not just for `None`.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn local_queue(&self) -> Result<Option<QueueState>> {
        self.read(|conn| QueueState::fetch(conn, self.device_id()))
    }

    /// Other devices' queues, most recently updated first, younger than `window` and
    /// not empty. Pass `Millis::MAX` to ignore age.
    ///
    /// # Errors
    ///
    /// Fails when the database does.
    pub fn resumable_queues(&self, now: Millis, window: Millis) -> Result<Vec<QueueState>> {
        let cutoff = if window < now {
            now - window
        } else {
            Millis::MIN
        };
        let rows = self.read(|conn| {
            QueueState::fetch_where(
                conn,
                "deviceId <> ? AND updatedAt >= ? ORDER BY updatedAt DESC",
                params![self.device_id(), cutoff],
            )
        })?;
        Ok(rows
            .into_iter()
            .filter(|queue| !queue.track_id_list().is_empty())
            .collect())
    }

    /// Upserts this device's queue unconditionally, and returns what was stored — the
    /// window may have dropped played history and rebased `position`.
    ///
    /// # Errors
    ///
    /// Fails when the row does not encode or the database fails; nothing is written then.
    pub fn save_queue(
        &self,
        device_name: &str,
        track_ids: &[String],
        position: i64,
        elapsed_ms: i64,
        now: Millis,
    ) -> Result<QueueState> {
        let state = self.queue_state(device_name, track_ids, position, elapsed_ms, now);
        self.write_queue(&state)?;
        Ok(state)
    }

    /// Upserts only if the list, the index or the name changed, or elapsed moved by more
    /// than the threshold either way. Reports whether it wrote.
    ///
    /// # Errors
    ///
    /// As [`Self::save_queue`].
    pub fn save_queue_if_changed(
        &self,
        device_name: &str,
        track_ids: &[String],
        position: i64,
        elapsed_ms: i64,
        now: Millis,
    ) -> Result<bool> {
        let candidate = self.queue_state(device_name, track_ids, position, elapsed_ms, now);
        let Some(existing) = self.local_queue()? else {
            // A player that has never played anything still ticks.
            if candidate.track_id_list().is_empty() {
                return Ok(false);
            }
            self.write_queue(&candidate)?;
            return Ok(true);
        };
        // Compared after windowing, so an edit past the cap is no change.
        let unchanged = existing.track_ids == candidate.track_ids
            && existing.position == candidate.position
            && existing.device_name == candidate.device_name
            && (existing.elapsed_ms - candidate.elapsed_ms).abs()
                <= QueuePolicy::ELAPSED_THRESHOLD_MS;
        if unchanged {
            return Ok(false);
        }
        self.write_queue(&candidate)?;
        Ok(true)
    }

    /// Empties this device's queue on sign-out, as an empty upsert that travels rather
    /// than a delete that cannot. Reports whether there was one. Local: it waits on no
    /// network, and the op goes out with the next sync.
    ///
    /// # Errors
    ///
    /// As [`Self::save_queue`].
    pub fn clear_queue(&self, now: Millis) -> Result<bool> {
        let Some(existing) = self.local_queue()? else {
            return Ok(false);
        };
        let empty: [&str; 0] = [];
        self.write_queue(&QueueState::new(
            self.device_id(),
            existing.device_name,
            &empty,
            0,
            0,
            now,
        ))?;
        Ok(true)
    }

    fn queue_state(
        &self,
        device_name: &str,
        track_ids: &[String],
        position: i64,
        elapsed_ms: i64,
        now: Millis,
    ) -> QueueState {
        let (window, position) = QueuePolicy::window(track_ids, position);
        QueueState::new(
            self.device_id(),
            device_name,
            &window,
            position,
            elapsed_ms.max(0),
            now,
        )
    }

    /// The row and its op in one transaction, superseding this device's unpushed queue
    /// op rather than stacking another behind it: queue state is a latest-value
    /// channel. Acknowledged ops are left alone; their ids keep a retry idempotent.
    fn write_queue(&self, state: &QueueState) -> Result<()> {
        let op = Op::new(
            QueueState::TABLE,
            &state.device_id,
            OpKind::Upsert,
            json::payload(state)?,
            state.updated_at,
        );
        self.transaction(|tx| {
            tx.execute(
                "DELETE FROM \"op\" WHERE \"entity\" = ? AND \"entityId\" = ? AND \"synced\" = 0",
                params![QueueState::TABLE, state.device_id],
            )?;
            state.save(tx)?;
            op.insert(tx)?;
            Ok(())
        })
    }
}
