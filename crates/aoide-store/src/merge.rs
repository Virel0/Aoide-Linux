//! Applies pulled ops to local rows.
//!
//! **Nothing in here may write to `op`.** An inbound change that appends an outbound
//! op is a change this device claims as its own and pushes back; the other device
//! pulls it, applies it, pushes it back, and the two trade the same row until one of
//! them is switched off. It is prevented structurally: the applier is handed a
//! transaction and never the `Store`, so `record` and `append` are out of reach, and
//! `Store::apply` checks the op count is unchanged in debug builds.
//!
//! Conflict resolution is last-writer-wins on `updatedAt`, with `originDevice` as the
//! tiebreak so two devices resolving the same pair independently reach the same
//! answer — per row for most tables, per field for playlists, folders and taste
//! flags, whose payloads carry `fieldUpdatedAt`.

use rusqlite::Transaction;
use serde::de::DeserializeOwned;

use crate::Millis;
use crate::error::Result;
use crate::field_stamps::merged;
use crate::json::{self, canonical_json};
use crate::models::{
    AlbumLike, Folder, Like, Op, OpKind, PlayEvent, Playlist, PlaylistItem, QueueState, TrackFlags,
};
use crate::quarantine::{QuarantinedOp, SyncDirection, write_pull_cursor};
use crate::record::{Record, SyncRecord};
use crate::store::Store;
use crate::sync::{PullCursor, PulledOp, SyncEntity, SyncFault, SyncPolicy, SyncReport};

/// A like of something: a row unique on the track or album it names.
trait Liking: SyncRecord {
    /// The condition matching rows that name the same thing.
    const UNIQUE_ON: &'static str;
    /// What it names.
    fn liked_item(&self) -> &str;
}

impl Liking for Like {
    const UNIQUE_ON: &'static str = "\"jellyfinId\" = ?";
    fn liked_item(&self) -> &str {
        &self.jellyfin_id
    }
}

impl Liking for AlbumLike {
    const UNIQUE_ON: &'static str = "\"albumId\" = ?";
    fn liked_item(&self) -> &str {
        &self.album_id
    }
}

pub(crate) struct InboundApplier {
    policy: SyncPolicy,
    now: Millis,
    pub(crate) report: SyncReport,
}

impl InboundApplier {
    pub(crate) fn new(policy: SyncPolicy, now: Millis) -> Self {
        Self {
            policy,
            now,
            report: SyncReport::default(),
        }
    }

    /// Applies one op, or quarantines it. Never fails for anything the op itself is
    /// guilty of — a single bad op must not abandon the rest of the batch.
    ///
    /// Our own ops come back on every pull and go through here like anyone else's.
    /// This path has to be idempotent regardless — a retried push lands twice — and a
    /// skip-by-device rule would refuse to restore a reinstalled device that kept its id.
    pub(crate) fn apply(&mut self, op: &PulledOp, tx: &Transaction<'_>) -> Result<()> {
        let Some(entity) = SyncEntity::from_wire_name(&op.entity) else {
            return self.quarantine(op, SyncFault::UnknownEntity(op.entity.clone()), tx);
        };
        let Some(operation) = OpKind::parse(&op.operation) else {
            return self.quarantine(op, SyncFault::UnknownOperation(op.operation.clone()), tx);
        };
        let Some(fields) = op.payload.as_object() else {
            return self.quarantine(op, SyncFault::PayloadNotAnObject, tx);
        };
        let identity = fields
            .get(entity.identity_key())
            .and_then(serde_json::Value::as_str);
        if identity != Some(op.entity_id.as_str()) {
            let fault = SyncFault::IdentityMismatch {
                entity_id: op.entity_id.clone(),
                payload_id: identity.map(str::to_owned),
            };
            return self.quarantine(op, fault, tx);
        }
        // Queue state has no `deleted` column and play events are append-only, so
        // neither can carry the flag; the check belongs to the tables that can.
        if operation == OpKind::Delete
            && entity != SyncEntity::QueueState
            && entity != SyncEntity::PlayEvent
            && !json::is_flag_set(fields.get("deleted"))
        {
            return self.quarantine(op, SyncFault::DeleteWithoutFlag, tx);
        }

        match entity {
            SyncEntity::Playlist => self.merge_stamped::<Playlist>(op, tx),
            SyncEntity::Folder => self.merge_stamped::<Folder>(op, tx),
            SyncEntity::PlaylistItem => self.merge_item(op, tx),
            SyncEntity::Like => self.merge_like::<Like>(op, tx),
            SyncEntity::AlbumLike => self.merge_like::<AlbumLike>(op, tx),
            SyncEntity::TrackFlags => self.merge_track_flags(op, tx),
            SyncEntity::QueueState => self.merge_queue(op, tx),
            SyncEntity::PlayEvent => self.append_event(op, operation, tx),
        }
    }

    // MARK: - Per entity

    /// Playlists and folders. A delete goes through the same path as an upsert,
    /// because it *is* one — of a row whose `deleted` flag is set — so a delete loses
    /// to a later edit exactly as any stale write does, and reviving a playlist that
    /// way is intended: deletes-always-win would let a device offline for a week
    /// destroy work done since.
    fn merge_stamped<R: SyncRecord>(&mut self, op: &PulledOp, tx: &Transaction<'_>) -> Result<()> {
        let existing = R::fetch(tx, &op.entity_id)?;
        let Some(row) = self.field_merged_winner(op, existing, tx)? else {
            return Ok(());
        };
        row.save(tx)?;
        self.report.applied += 1;
        Ok(())
    }

    /// An item whose playlist has not arrived is stored anyway: ops arrive in commit
    /// order, so the parent is usually in a later batch, and every query for items
    /// filters by playlist — it is invisible until its parent lands rather than wrong.
    fn merge_item(&mut self, op: &PulledOp, tx: &Transaction<'_>) -> Result<()> {
        let existing = PlaylistItem::fetch(tx, &op.entity_id)?;
        let Some(row) = self.winner(op, existing, tx)? else {
            return Ok(());
        };
        if Playlist::fetch(tx, &row.playlist_id)?.is_none() {
            self.report.orphaned_items += 1;
        }
        row.save(tx)?;
        self.report.applied += 1;
        Ok(())
    }

    /// Likes have an identity that is not their key: the table is unique on the track
    /// (or album), and two devices liking it offline mint two rows. The clash is
    /// settled by the same comparison as everything else and the loser removed — a hard
    /// delete without an op, safe here and only here because every device reaches it
    /// alone.
    fn merge_like<R: Liking>(&mut self, op: &PulledOp, tx: &Transaction<'_>) -> Result<()> {
        let existing = R::fetch(tx, &op.entity_id)?;
        let Some(row) = self.winner(op, existing, tx)? else {
            return Ok(());
        };
        let clash = R::fetch_where(tx, R::UNIQUE_ON, [row.liked_item()])?
            .into_iter()
            .next();
        if let Some(clash) = clash.filter(|clash| clash.id() != row.id()) {
            if !incoming_wins(
                (row.updated_at(), row.origin_device()),
                (clash.updated_at(), clash.origin_device()),
                true,
            ) {
                self.report.ignored_stale += 1;
                return Ok(());
            }
            R::delete(tx, clash.id())?;
            self.report.superseded_likes += 1;
        }
        row.save(tx)?;
        self.report.applied += 1;
        Ok(())
    }

    /// Per field like a playlist, deduped by track like a like.
    fn merge_track_flags(&mut self, op: &PulledOp, tx: &Transaction<'_>) -> Result<()> {
        let existing = TrackFlags::fetch(tx, &op.entity_id)?;
        let Some(row) = self.field_merged_winner(op, existing, tx)? else {
            return Ok(());
        };
        let clash = TrackFlags::fetch_where(tx, "\"jellyfinId\" = ?", [&row.jellyfin_id])?
            .into_iter()
            .next();
        if let Some(clash) = clash.filter(|clash| clash.id != row.id) {
            if !incoming_wins(
                (row.updated_at, &row.origin_device),
                (clash.updated_at, &clash.origin_device),
                true,
            ) {
                self.report.ignored_stale += 1;
                return Ok(());
            }
            TrackFlags::delete(tx, &clash.id)?;
        }
        row.save(tx)?;
        self.report.applied += 1;
        Ok(())
    }

    /// Keyed by device and replaced whole: merging a queue field by field could pair
    /// one device's position with another's track list, a state that existed nowhere.
    fn merge_queue(&mut self, op: &PulledOp, tx: &Transaction<'_>) -> Result<()> {
        let mut incoming: QueueState = match decode(op) {
            Ok(row) => row,
            Err(detail) => return self.quarantine(op, SyncFault::UndecodablePayload(detail), tx),
        };
        incoming.updated_at = self.correct_skew(incoming.updated_at, op.received_at);

        if let Some(existing) = QueueState::fetch(tx, &op.entity_id)? {
            let datable = self.is_datable(existing.updated_at, op.received_at);
            if !incoming_wins(
                (incoming.updated_at, &incoming.origin_device),
                (existing.updated_at, &existing.origin_device),
                datable,
            ) {
                self.report.ignored_stale += 1;
                return Ok(());
            }
        }
        incoming.save(tx)?;
        self.report.applied += 1;
        Ok(())
    }

    /// Insert, or take a *more finished* version of an event already here.
    ///
    /// The row is written when playback starts and filled in when it ends, so a sync
    /// between the two delivers the bare start first and the outcome second; plain
    /// insert-if-absent would drop the outcome for good. `endedAt` decides — only the
    /// device that opened an event writes that id, so this settles no conflict, it asks
    /// which copy is more complete.
    fn append_event(
        &mut self,
        op: &PulledOp,
        operation: OpKind,
        tx: &Transaction<'_>,
    ) -> Result<()> {
        if operation != OpKind::Upsert {
            return self.quarantine(op, SyncFault::AppendOnlyDelete, tx);
        }
        let event: PlayEvent = match decode(op) {
            Ok(row) => row,
            Err(detail) => return self.quarantine(op, SyncFault::UndecodablePayload(detail), tx),
        };
        // Counted, not corrected: an event wins no conflicts, and `startedAt` is a
        // claim about when listening happened.
        self.note_skew(op.created_at, op.received_at);

        match PlayEvent::fetch(tx, &event.id)? {
            Some(existing) if !is_more_finished(&event, &existing) => {
                self.report.ignored_duplicate += 1;
            }
            Some(_) => {
                event.save(tx)?;
                self.report.applied += 1;
            }
            None => {
                event.insert(tx)?;
                self.report.applied += 1;
            }
        }
        Ok(())
    }

    // MARK: - Merge mechanics

    /// The row-level winner decides the metadata; each content field is then taken
    /// from whichever side changed it more recently. An incoming row that loses the row
    /// comparison may still hold the newer copy of one field, so the stale path returns
    /// the merge whenever it differs from what is stored.
    fn field_merged_winner<R: SyncRecord>(
        &mut self,
        op: &PulledOp,
        existing: Option<R>,
        tx: &Transaction<'_>,
    ) -> Result<Option<R>> {
        let mut incoming: R = match decode(op) {
            Ok(row) => row,
            Err(detail) => {
                self.quarantine(op, SyncFault::UndecodablePayload(detail), tx)?;
                return Ok(None);
            }
        };

        let corrected = self.correct_skew(incoming.updated_at(), op.received_at);
        let (deleted, origin) = (incoming.deleted(), incoming.origin_device().to_owned());
        incoming.set_meta(corrected, deleted, &origin);
        if let Some(mut stamps) = incoming.field_stamps().cloned() {
            let (policy, now, received) = (self.policy, self.now, op.received_at);
            let credible = |time| is_credible(time, received, now, policy.clock_skew_tolerance_ms);
            if stamps.correcting_skew(credible, received) {
                incoming.set_field_stamps(Some(stamps));
            }
        }

        let Some(existing) = existing else {
            return Ok(Some(incoming));
        };

        // A local row this device can no longer place in time forfeits whole: its
        // field stamps come from the same unbelievable clock.
        if !self.is_datable(existing.updated_at(), op.received_at) {
            return Ok(Some(incoming));
        }

        let incoming_leads = incoming_wins(
            (incoming.updated_at(), incoming.origin_device()),
            (existing.updated_at(), existing.origin_device()),
            true,
        );
        let winner = if incoming_leads { &incoming } else { &existing };
        let row = merged(&incoming, &existing, winner)?;

        // Nothing changed, so nothing is written — which is what keeps re-applying an
        // op idempotent, and our own ops coming back from touching a row.
        if row == existing {
            self.report.ignored_stale += 1;
            return Ok(None);
        }
        Ok(Some(row))
    }

    fn winner<R: SyncRecord>(
        &mut self,
        op: &PulledOp,
        existing: Option<R>,
        tx: &Transaction<'_>,
    ) -> Result<Option<R>> {
        let mut incoming: R = match decode(op) {
            Ok(row) => row,
            Err(detail) => {
                // A corrupt row, or a newer client's row missing a column this build
                // requires — a real incompatibility, not one to paper over.
                self.quarantine(op, SyncFault::UndecodablePayload(detail), tx)?;
                return Ok(None);
            }
        };
        let corrected = self.correct_skew(incoming.updated_at(), op.received_at);
        let (deleted, origin) = (incoming.deleted(), incoming.origin_device().to_owned());
        incoming.set_meta(corrected, deleted, &origin);

        let Some(existing) = existing else {
            return Ok(Some(incoming));
        };
        let datable = self.is_datable(existing.updated_at(), op.received_at);
        if !incoming_wins(
            (incoming.updated_at(), incoming.origin_device()),
            (existing.updated_at(), existing.origin_device()),
            datable,
        ) {
            self.report.ignored_stale += 1;
            return Ok(None);
        }
        Ok(Some(incoming))
    }

    /// Pulls an inbound timestamp back to the server's receipt time when it is not
    /// credible. `receivedAt`, not this device's clock, because it is identical on
    /// every device that pulls the op, so they all correct to the same number.
    fn correct_skew(&mut self, timestamp: Millis, received_at: Millis) -> Millis {
        if self.is_datable(timestamp, received_at) {
            return timestamp;
        }
        self.report.clock_skewed += 1;
        received_at
    }

    fn note_skew(&mut self, timestamp: Millis, received_at: Millis) {
        if !self.is_datable(timestamp, received_at) {
            self.report.clock_skewed += 1;
        }
    }

    fn is_datable(&self, timestamp: Millis, received_at: Millis) -> bool {
        is_credible(
            timestamp,
            received_at,
            self.now,
            self.policy.clock_skew_tolerance_ms,
        )
    }

    fn quarantine(&mut self, op: &PulledOp, fault: SyncFault, tx: &Transaction<'_>) -> Result<()> {
        QuarantinedOp {
            op_id: op.op_id.clone(),
            entity: op.entity.clone(),
            entity_id: op.entity_id.clone(),
            direction: SyncDirection::Inbound.as_str().to_owned(),
            reason: fault.reason(),
            payload: canonical_json(&op.payload),
            quarantined_at: self.now,
            operation: None,
            created_at: None,
        }
        .save(tx)?;
        self.report.quarantined_inbound += 1;
        Ok(())
    }
}

fn decode<R: DeserializeOwned>(op: &PulledOp) -> std::result::Result<R, String> {
    serde_json::from_value(op.payload.clone()).map_err(|error| error.to_string())
}

/// Strictly greater, which is what makes the whole path idempotent: an op applied a
/// second time compares equal to the row it wrote and loses. `originDevice` breaks a
/// tie so every device reaches the same answer independently.
///
/// `existing_is_datable` is false only for a local row this device can no longer place
/// in time; the inbound copy at least has a time the server witnessed, so it takes the
/// row. That is what lets a device recover once its clock is fixed.
pub(crate) fn incoming_wins(
    incoming: (Millis, &str),
    existing: (Millis, &str),
    existing_is_datable: bool,
) -> bool {
    if !existing_is_datable {
        return true;
    }
    if incoming.0 == existing.0 {
        incoming.1 > existing.1
    } else {
        incoming.0 > existing.0
    }
}

/// Whether a timestamp is one this device can believe at all.
///
/// Two references, and both must agree that it is in the future. Checking only
/// against `receivedAt` cannot tell a fast client from a *slow server* — a NAS with no
/// RTC sits behind every device, and every inbound row would be dragged back older
/// than the local copy, splitting every device from every other. A client genuinely
/// stamping 2096 is ahead of both on every device that receives it.
pub(crate) fn is_credible(
    timestamp: Millis,
    received_at: Millis,
    now: Millis,
    tolerance: Millis,
) -> bool {
    !(is_ahead(timestamp, received_at, tolerance) && is_ahead(timestamp, now, tolerance))
}

/// Overflow is "not ahead": wrapping would invert the comparison.
fn is_ahead(timestamp: Millis, reference: Millis, tolerance: Millis) -> bool {
    reference
        .checked_add(tolerance)
        .is_some_and(|ceiling| timestamp > ceiling)
}

/// Whether `incoming` is a later stage of the same listen than `existing`. Equal is not
/// more: two copies that ended at the same instant are the same copy.
pub(crate) fn is_more_finished(incoming: &PlayEvent, existing: &PlayEvent) -> bool {
    match (incoming.ended_at, existing.ended_at) {
        (None, _) => false,
        (Some(_), None) => true,
        (Some(incoming_end), Some(existing_end)) => incoming_end > existing_end,
    }
}

impl Store {
    /// Applies a pulled page and advances the pull cursor, in **one** transaction.
    ///
    /// The contract asks for the cursor to be stored after the page is applied, so an
    /// interrupted sync replays rather than skips; doing both in one transaction closes
    /// the window between the two writes. A replayed page is harmless because every
    /// merge here is idempotent.
    ///
    /// # Errors
    ///
    /// Fails when the database does; the whole page is rolled back then, cursor included,
    /// and replays on the next sync. An op this build cannot apply is quarantined rather
    /// than failing the page.
    pub fn apply(
        &self,
        ops: &[PulledOp],
        cursor: PullCursor,
        policy: SyncPolicy,
        now: Millis,
    ) -> Result<SyncReport> {
        self.transaction(|tx| {
            let ops_before = Op::count(tx)?;

            let mut applier = InboundApplier::new(policy, now);
            for op in ops {
                applier.apply(op, tx)?;
            }
            write_pull_cursor(tx, cursor)?;

            // The ping-pong guard: applying inbound changes must queue nothing.
            debug_assert_eq!(
                Op::count(tx)?,
                ops_before,
                "applying {} inbound ops queued outbound ops",
                ops.len()
            );

            let mut report = applier.report;
            report.pulled = ops.len();
            report.cursor = Some(cursor);
            Ok(report)
        })
    }
}
