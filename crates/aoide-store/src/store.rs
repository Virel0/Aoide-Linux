//! The store itself: one SQLite connection, and the single path syncable writes take.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, Transaction, params_from_iter};

use crate::error::Result;
use crate::field_stamps::changed_fields;
use crate::json;
use crate::models::{Op, OpKind, PlayEvent};
use crate::record::{Record, SyncRecord, placeholders};
use crate::{Millis, schema};

/// The local source of truth for playlists, likes, history and queue state.
///
/// One connection behind a mutex: SQLite serialises writers anyway, and every call
/// here is short. Safe to share between threads.
pub struct Store {
    conn: Mutex<Connection>,
    device_id: String,
}

impl Store {
    /// Opens the store at `path`, creating and migrating it as needed.
    pub fn open(path: impl AsRef<Path>, device_id: impl Into<String>) -> Result<Self> {
        let conn = Connection::open(path)?;
        // Better concurrency and crash-durability than the rollback journal, and the
        // difference matters: playback writes history while the UI reads it.
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        Self::prepare(conn, device_id.into())
    }

    /// A store that lives in memory, for tests and previews.
    pub fn in_memory(device_id: impl Into<String>) -> Result<Self> {
        Self::prepare(Connection::open_in_memory()?, device_id.into())
    }

    fn prepare(mut conn: Connection, device_id: String) -> Result<Self> {
        // Off by default in SQLite. The schema declares no relationships today, and
        // this keeps the phone's setting for the day one does.
        conn.execute_batch("PRAGMA foreign_keys = ON")?;
        schema::migrate(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            device_id,
        })
    }

    /// Stamped onto every row this device writes, and the tiebreak when two devices
    /// change the same row in the same millisecond.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the lock leaves the connection itself usable — every
        // write is a transaction that rolled back with the unwinding.
        self.conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Reads, with nothing written.
    pub fn read<T>(&self, body: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T> {
        let conn = self.lock();
        Ok(body(&conn)?)
    }

    /// A write that does **not** record an op, in one transaction. Only correct for the
    /// track cache and other purely local state; anything syncable goes through
    /// [`Store::record`].
    pub fn write_local<T>(
        &self,
        body: impl FnOnce(&Transaction<'_>) -> rusqlite::Result<T>,
    ) -> Result<T> {
        self.transaction(|tx| Ok(body(tx)?))
    }

    /// A transaction whose body may fail with any store error.
    pub(crate) fn transaction<T>(
        &self,
        body: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let value = body(&tx)?;
        tx.commit()?;
        Ok(value)
    }

    // MARK: - Recording changes

    /// Saves a syncable row and its op in one transaction, stamped with this device and
    /// `now`, and returns the row as stored.
    ///
    /// The single entry point for every syncable mutation. A row saved without its op
    /// is a change that silently never leaves this device, and nothing later can
    /// detect that it happened; making the two inseparable is cheaper than any
    /// reconciliation would be.
    ///
    /// For a row that merges per field, the fields that changed against the stored
    /// row are stamped with `now` — which can only be known here, against what is on
    /// disk. A delete touches no content field, so it stamps none: a playlist deleted
    /// on one device and renamed on another keeps the rename.
    pub fn record<R: SyncRecord>(&self, row: R, deleted: bool, now: Millis) -> Result<R> {
        self.transaction(|tx| self.record_in(tx, row, deleted, now))
    }

    pub(crate) fn record_in<R: SyncRecord>(
        &self,
        tx: &Transaction<'_>,
        row: R,
        deleted: bool,
        now: Millis,
    ) -> Result<R> {
        let mut row = row;
        if R::FIELD_STAMPED {
            let previous = R::fetch(tx, row.id())?;
            let mut stamps = previous
                .as_ref()
                .and_then(|p| p.field_stamps().cloned())
                .unwrap_or_default();
            if !deleted {
                let changed = changed_fields(&row, previous.as_ref())?;
                stamps.stamp(changed.iter(), now);
            }
            row.set_field_stamps(Some(stamps));
        }
        row.set_meta(now, deleted, &self.device_id);

        let op = Op::new(
            R::TABLE,
            row.id(),
            if deleted {
                OpKind::Delete
            } else {
                OpKind::Upsert
            },
            json::payload(&row)?,
            now,
        );
        row.save(tx)?;
        op.insert(tx)?;
        Ok(row)
    }

    /// Appends a play event and its op.
    ///
    /// Separate from `record` because history is append-only: events carry no
    /// `updatedAt`, `deleted` or conflict resolution, which also makes them the one
    /// thing here that cannot conflict during sync.
    pub fn append(&self, event: PlayEvent) -> Result<PlayEvent> {
        let op = Op::new(
            PlayEvent::TABLE,
            &event.id,
            OpKind::Upsert,
            json::payload(&event)?,
            event.started_at,
        );
        self.transaction(|tx| {
            event.insert(tx)?;
            op.insert(tx)?;
            Ok(())
        })?;
        Ok(event)
    }

    // MARK: - Sync plumbing

    /// Ops waiting to be pushed, oldest first.
    ///
    /// `held` names local tables whose ops stay in the queue untouched — neither
    /// pushed nor quarantined — and never block what is behind them.
    pub fn pending_ops(&self, limit: usize, held: &BTreeSet<String>) -> Result<Vec<Op>> {
        self.read(|conn| {
            let mut condition = "\"synced\" = 0".to_owned();
            if !held.is_empty() {
                condition.push_str(&format!(
                    " AND \"entity\" NOT IN ({})",
                    placeholders(held.len())
                ));
            }
            let mut params: Vec<rusqlite::types::Value> = held
                .iter()
                .map(|entity| rusqlite::types::Value::Text(entity.clone()))
                .collect();
            params.push(rusqlite::types::Value::Integer(
                i64::try_from(limit).unwrap_or(i64::MAX),
            ));
            // `rowid` breaks ties so a burst of ops in one millisecond pages in a
            // stable order rather than whichever order SQLite happens to read.
            Op::fetch_where(
                conn,
                &format!("{condition} ORDER BY \"createdAt\", rowid LIMIT ?"),
                params_from_iter(params),
            )
        })
    }

    /// Which of `op_ids` are still waiting to be pushed.
    pub fn unsynced_op_ids(&self, op_ids: &[String]) -> Result<BTreeSet<String>> {
        self.read(|conn| {
            let mut present = BTreeSet::new();
            for chunk in op_ids.chunks(crate::history::MAX_PARAMETERS) {
                let mut statement = conn.prepare(&format!(
                    "SELECT \"opId\" FROM \"op\" WHERE \"synced\" = 0 AND \"opId\" IN ({})",
                    placeholders(chunk.len())
                ))?;
                let rows = statement.query_map(params_from_iter(chunk), |row| row.get(0))?;
                for row in rows {
                    present.insert(row?);
                }
            }
            Ok(present)
        })
    }

    /// How many ops are waiting to be pushed.
    pub fn pending_op_count(&self) -> Result<i64> {
        self.read(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM \"op\" WHERE \"synced\" = 0",
                [],
                |row| row.get(0),
            )
        })
    }

    /// Marks ops the server acknowledged. By the id this device minted, so
    /// acknowledging an op twice, or one that is no longer here, is harmless.
    pub fn mark_synced(&self, op_ids: &[String]) -> Result<()> {
        if op_ids.is_empty() {
            return Ok(());
        }
        self.write_local(|tx| {
            for chunk in op_ids.chunks(crate::history::MAX_PARAMETERS) {
                tx.execute(
                    &format!(
                        "UPDATE \"op\" SET \"synced\" = 1 WHERE \"opId\" IN ({})",
                        placeholders(chunk.len())
                    ),
                    params_from_iter(chunk),
                )?;
            }
            Ok(())
        })
    }

    /// The pull cursor as stored, untyped. See [`Store::pull_cursor`].
    pub fn sync_cursor(&self) -> Result<Option<String>> {
        self.state("cursor")
    }

    /// Overwrites the pull cursor. The engine never calls this — it advances the cursor
    /// inside the transaction that applies a batch — but a person repairing a store
    /// may.
    pub fn set_sync_cursor(&self, cursor: &str) -> Result<()> {
        self.write_local(|tx| set_state(tx, "cursor", cursor))
    }

    pub(crate) fn state(&self, key: &str) -> Result<Option<String>> {
        self.read(|conn| get_state(conn, key))
    }
}

pub(crate) fn get_state(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT \"value\" FROM \"syncState\" WHERE \"key\" = ?",
        [key],
        |row| row.get(0),
    )
    .optional()
}

pub(crate) fn set_state(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO \"syncState\" (\"key\", \"value\") VALUES (?, ?) \
         ON CONFLICT(\"key\") DO UPDATE SET \"value\" = excluded.\"value\"",
        [key, value],
    )?;
    Ok(())
}

impl std::fmt::Debug for Store {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Store")
            .field("device_id", &self.device_id)
            .finish_non_exhaustive()
    }
}
