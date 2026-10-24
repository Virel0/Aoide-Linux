//! Where ops go when they can never succeed.

use rusqlite::Row;
use rusqlite::types::Value as SqlValue;

use crate::Millis;
use crate::error::Result;
use crate::models::{Op, OpKind};
use crate::record::{Record, int, opt_int, opt_text, text};
use crate::store::{Store, set_state};
use crate::sync::{PullCursor, SyncFault};

/// Which way an op was travelling when it died.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyncDirection {
    /// Ours, and either invalid before it left or refused by the server.
    Outbound,
    /// Somebody else's, and unapplicable here.
    Inbound,
}

impl SyncDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Outbound => "outbound",
            Self::Inbound => "inbound",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "outbound" => Some(Self::Outbound),
            "inbound" => Some(Self::Inbound),
            _ => None,
        }
    }
}

/// An op that can never succeed, kept out of the way but not thrown away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuarantinedOp {
    pub op_id: String,
    /// Spelled as it arrived — the local table name outbound, the wire name inbound.
    /// For the case this table exists to explain, the unrecognised spelling is the
    /// finding.
    pub entity: String,
    pub entity_id: String,
    pub direction: String,
    pub reason: String,
    /// Kept whole: for an inbound op it is the only copy once the cursor has moved on.
    pub payload: String,
    pub quarantined_at: Millis,
    /// Absent on rows quarantined before v5, which therefore cannot be re-queued.
    pub operation: Option<String>,
    /// When the change was originally made, not when it was quarantined.
    pub created_at: Option<Millis>,
}

impl QuarantinedOp {
    pub fn sync_direction(&self) -> Option<SyncDirection> {
        SyncDirection::parse(&self.direction)
    }

    /// Whether this can be handed back to the outbound queue: only a server fault, and
    /// only outbound. An inbound op has no queue to return to.
    pub fn is_retryable(&self) -> bool {
        self.sync_direction() == Some(SyncDirection::Outbound)
            && self.operation.is_some()
            && self.created_at.is_some()
            && self.reason.contains("the server failed")
    }
}

impl Record for QuarantinedOp {
    const TABLE: &'static str = "quarantinedOp";
    const KEY: &'static str = "opId";
    const COLUMNS: &'static [&'static str] = &[
        "opId",
        "entity",
        "entityId",
        "direction",
        "reason",
        "payload",
        "quarantinedAt",
        "operation",
        "createdAt",
    ];

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            op_id: row.get("opId")?,
            entity: row.get("entity")?,
            entity_id: row.get("entityId")?,
            direction: row.get("direction")?,
            reason: row.get("reason")?,
            payload: row.get("payload")?,
            quarantined_at: row.get("quarantinedAt")?,
            operation: row.get("operation")?,
            created_at: row.get("createdAt")?,
        })
    }

    fn values(&self) -> Vec<SqlValue> {
        vec![
            text(&self.op_id),
            text(&self.entity),
            text(&self.entity_id),
            text(&self.direction),
            text(&self.reason),
            text(&self.payload),
            int(self.quarantined_at),
            opt_text(&self.operation),
            opt_int(self.created_at),
        ]
    }

    fn key(&self) -> &str {
        &self.op_id
    }
}

impl Store {
    /// Everything quarantined, newest first.
    pub fn quarantined_ops(&self, limit: usize) -> Result<Vec<QuarantinedOp>> {
        self.read(|conn| {
            QuarantinedOp::fetch_where(
                conn,
                "1 ORDER BY \"quarantinedAt\" DESC, rowid DESC LIMIT ?",
                [i64::try_from(limit).unwrap_or(i64::MAX)],
            )
        })
    }

    pub fn quarantine_count(&self) -> Result<i64> {
        self.read(QuarantinedOp::count)
    }

    /// Forgets quarantined ops. Nothing re-queues them.
    pub fn discard_quarantined(&self, op_ids: &[String]) -> Result<()> {
        self.write_local(|tx| {
            for op_id in op_ids {
                QuarantinedOp::delete(tx, op_id)?;
            }
            Ok(())
        })
    }

    pub fn discard_all_quarantined(&self) -> Result<()> {
        self.write_local(|tx| tx.execute("DELETE FROM \"quarantinedOp\"", []).map(|_| ()))
    }

    /// The pull cursor, typed — the only thing between this row and a push cursor
    /// being written into it.
    pub fn pull_cursor(&self) -> Result<Option<PullCursor>> {
        Ok(self
            .sync_cursor()?
            .and_then(|stored| stored.parse().ok())
            .map(PullCursor))
    }

    /// Moves an outbound op out of the queue and into quarantine, in one transaction.
    ///
    /// Moved, not flagged: a dead op that stays pending is retried forever and blocks
    /// everything queued behind it. Marking it synced would be a lie about what the
    /// server holds.
    pub fn quarantine(&self, op: &Op, fault: &SyncFault, now: Millis) -> Result<()> {
        self.write_local(|tx| {
            QuarantinedOp {
                op_id: op.op_id.clone(),
                entity: op.entity.clone(),
                entity_id: op.entity_id.clone(),
                direction: SyncDirection::Outbound.as_str().to_owned(),
                reason: fault.reason(),
                payload: op.payload.clone(),
                quarantined_at: now,
                operation: Some(op.operation.clone()),
                created_at: Some(op.created_at),
            }
            .save(tx)?;
            Op::delete(tx, &op.op_id)?;
            Ok(())
        })
    }

    /// Puts server-faulted ops back in the outbound queue, all of them or the ones
    /// named. Returns how many went back: rows from before v5 cannot be rebuilt.
    pub fn requeue_quarantined(&self, op_ids: Option<&[String]>) -> Result<usize> {
        self.write_local(|tx| {
            let mut candidates = QuarantinedOp::fetch_all(tx)?;
            if let Some(op_ids) = op_ids {
                candidates.retain(|held| op_ids.contains(&held.op_id));
            }
            let mut moved = 0;
            for held in candidates.into_iter().filter(QuarantinedOp::is_retryable) {
                let (Some(operation), Some(created_at)) = (
                    held.operation.as_deref().and_then(OpKind::parse),
                    held.created_at,
                ) else {
                    continue;
                };
                Op::new(
                    &held.entity,
                    &held.entity_id,
                    operation,
                    &held.payload,
                    created_at,
                )
                .with_id(&held.op_id)
                .insert(tx)?;
                QuarantinedOp::delete(tx, &held.op_id)?;
                moved += 1;
            }
            Ok(moved)
        })
    }

    /// How many are worth offering a retry for.
    pub fn retryable_quarantine_count(&self) -> Result<usize> {
        Ok(self
            .read(QuarantinedOp::fetch_all)?
            .iter()
            .filter(|held| held.is_retryable())
            .count())
    }
}

pub(crate) fn write_pull_cursor(
    conn: &rusqlite::Connection,
    cursor: PullCursor,
) -> rusqlite::Result<()> {
    set_state(conn, "cursor", &cursor.0.to_string())
}
