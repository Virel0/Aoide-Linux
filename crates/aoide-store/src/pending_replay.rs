//! Waiting songs a build before 0.5.0 set aside. That build found a playlist item
//! with no track id, could not read it, and quarantined it; the pull cursor has
//! moved past it since, so the server will not offer it again and the quarantined
//! copy is the only one. Each goes back through the ordinary merge, so a newer copy
//! already here (the track Melete swapped in) still wins.

use serde_json::Value;

use crate::Millis;
use crate::error::Result;
use crate::merge::InboundApplier;
use crate::quarantine::{QuarantinedOp, SyncDirection};
use crate::record::Record;
use crate::store::Store;
use crate::sync::{PulledOp, SyncEntity, SyncPolicy, SyncReport};

impl Store {
    /// Applies the waiting songs an earlier build quarantined, and takes each one
    /// the merge accepts (or that loses to a newer copy) out of quarantine. One it
    /// refuses again stays there.
    ///
    /// # Errors
    ///
    /// Fails when the database does; nothing changes then.
    pub fn replay_quarantined_waiting_songs(
        &self,
        policy: SyncPolicy,
        now: Millis,
    ) -> Result<SyncReport> {
        self.transaction(|tx| {
            let held = QuarantinedOp::fetch_all(tx)?;
            let mut applier = InboundApplier::new(policy, now);
            for row in &held {
                let Some(op) = waiting_song(row) else {
                    continue;
                };
                let refused_before = applier.report.quarantined_inbound;
                applier.apply(&op, tx)?;
                if applier.report.quarantined_inbound == refused_before {
                    QuarantinedOp::delete(tx, &row.op_id)?;
                }
            }
            Ok(applier.report)
        })
    }
}

/// The op a quarantined waiting song came in as, near enough to merge: its own
/// payload, timed by its own `updatedAt` and received when it was set aside. `None`
/// for anything that isn't an inbound playlist item with a `pending` song.
fn waiting_song(row: &QuarantinedOp) -> Option<PulledOp> {
    if row.sync_direction() != Some(SyncDirection::Inbound)
        || row.entity != SyncEntity::PlaylistItem.wire_name()
    {
        return None;
    }
    let payload: Value = serde_json::from_str(&row.payload).ok()?;
    payload.get("pending").filter(|p| p.is_object())?;
    let deleted = payload.get("deleted").and_then(Value::as_bool) == Some(true);
    Some(PulledOp {
        op_id: row.op_id.clone(),
        entity: row.entity.clone(),
        entity_id: row.entity_id.clone(),
        operation: if deleted { "delete" } else { "upsert" }.to_owned(),
        created_at: payload
            .get("updatedAt")
            .and_then(Value::as_i64)
            .unwrap_or(row.quarantined_at),
        device_id: payload
            .get("originDevice")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        payload,
        seq: 0,
        received_at: row.quarantined_at,
        author_user_id: None,
    })
}
