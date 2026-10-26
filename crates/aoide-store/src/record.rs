//! How a row maps to its table.

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, Row, params_from_iter};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::Millis;
use crate::field_stamps::FieldStamps;

/// A row of one table, keyed by one text column.
///
/// `save` is an upsert on the key and `insert` a plain insert, matching what the
/// phone's GRDB records do: a `save` that clashes with another row on a *unique*
/// column fails rather than silently replacing that row, which is what lets the merge
/// resolve such clashes deliberately.
pub trait Record: Sized {
    /// The table's name, unquoted.
    const TABLE: &'static str;
    /// The key column: unique, and what `fetch`, `save` and `delete` match on.
    const KEY: &'static str;
    /// Every column, in the order `values` and `from_row` use.
    const COLUMNS: &'static [&'static str];

    /// Reads a row selected as `COLUMNS`, in that order.
    ///
    /// # Errors
    ///
    /// Fails when a column is missing or of the wrong type.
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self>;
    /// Values in `COLUMNS` order.
    fn values(&self) -> Vec<SqlValue>;
    /// This row's value in the `KEY` column.
    fn key(&self) -> &str;

    /// The row whose key is `key`, if there is one, deleted or not.
    ///
    /// # Errors
    ///
    /// Fails when SQLite does.
    fn fetch(conn: &Connection, key: &str) -> rusqlite::Result<Option<Self>> {
        conn.query_row(
            &format!(
                "SELECT {} FROM \"{}\" WHERE \"{}\" = ?",
                column_list(Self::COLUMNS),
                Self::TABLE,
                Self::KEY
            ),
            [key],
            Self::from_row,
        )
        .optional()
    }

    /// Every row, deleted ones included, in no particular order.
    ///
    /// # Errors
    ///
    /// Fails when SQLite does.
    fn fetch_all(conn: &Connection) -> rusqlite::Result<Vec<Self>> {
        Self::fetch_where(conn, "1", [])
    }

    /// Rows matching a SQL condition over this table's own columns.
    ///
    /// # Errors
    ///
    /// Fails when SQLite does, including on a malformed `condition`.
    fn fetch_where<P: rusqlite::Params>(
        conn: &Connection,
        condition: &str,
        params: P,
    ) -> rusqlite::Result<Vec<Self>> {
        let mut statement = conn.prepare(&format!(
            "SELECT {} FROM \"{}\" WHERE {}",
            column_list(Self::COLUMNS),
            Self::TABLE,
            condition
        ))?;
        statement
            .query_map(params, Self::from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()
    }

    /// How many rows the table holds, deleted ones included.
    ///
    /// # Errors
    ///
    /// Fails when SQLite does.
    fn count(conn: &Connection) -> rusqlite::Result<i64> {
        conn.query_row(
            &format!("SELECT COUNT(*) FROM \"{}\"", Self::TABLE),
            [],
            |row| row.get(0),
        )
    }

    /// Inserts this row as new.
    ///
    /// # Errors
    ///
    /// Fails when SQLite does, including when the key or another unique column is taken.
    fn insert(&self, conn: &Connection) -> rusqlite::Result<()> {
        conn.execute(
            &format!(
                "INSERT INTO \"{}\" ({}) VALUES ({})",
                Self::TABLE,
                column_list(Self::COLUMNS),
                placeholders(Self::COLUMNS.len())
            ),
            params_from_iter(self.values()),
        )?;
        Ok(())
    }

    /// Inserts this row, or replaces every column of the row with its key.
    ///
    /// # Errors
    ///
    /// Fails when SQLite does, including when another row holds one of its unique columns.
    fn save(&self, conn: &Connection) -> rusqlite::Result<()> {
        let updates = Self::COLUMNS
            .iter()
            .filter(|column| **column != Self::KEY)
            .map(|column| format!("\"{column}\" = excluded.\"{column}\""))
            .collect::<Vec<_>>()
            .join(", ");
        conn.execute(
            &format!(
                "INSERT INTO \"{}\" ({}) VALUES ({}) ON CONFLICT(\"{}\") DO UPDATE SET {}",
                Self::TABLE,
                column_list(Self::COLUMNS),
                placeholders(Self::COLUMNS.len()),
                Self::KEY,
                updates
            ),
            params_from_iter(self.values()),
        )?;
        Ok(())
    }

    /// Removes the row with key `key` outright, leaving no tombstone; whether there
    /// was one.
    ///
    /// # Errors
    ///
    /// Fails when SQLite does.
    fn delete(conn: &Connection, key: &str) -> rusqlite::Result<bool> {
        let changed = conn.execute(
            &format!(
                "DELETE FROM \"{}\" WHERE \"{}\" = ?",
                Self::TABLE,
                Self::KEY
            ),
            [key],
        )?;
        Ok(changed > 0)
    }
}

/// A row that syncs: it carries the four columns that make it transmissible, and it
/// travels as its own JSON encoding.
///
/// `deleted` is a flag rather than a `DELETE` because a removed row has to be
/// transmissible: the absence of a row is indistinguishable from never having seen
/// it, so a hard delete would resurrect itself from any device that still had a copy.
pub trait SyncRecord: Record + Serialize + DeserializeOwned + Clone + PartialEq {
    /// Whether the row merges field by field. Only playlists, folders and taste flags
    /// do; see `FieldStamps`.
    const FIELD_STAMPED: bool = false;

    /// The row's id, minted by the device that created it.
    fn id(&self) -> &str;
    /// When the row last changed, in milliseconds since the epoch; the merge's clock.
    fn updated_at(&self) -> Millis;
    /// Whether the row is a tombstone.
    fn deleted(&self) -> bool;
    /// The device that made the last change.
    fn origin_device(&self) -> &str;
    /// Stamps a change: when, whether it deletes, and on which device.
    fn set_meta(&mut self, updated_at: Millis, deleted: bool, origin_device: &str);

    /// The per-field change times of a [`Self::FIELD_STAMPED`] row; `None` for the rest.
    fn field_stamps(&self) -> Option<&FieldStamps> {
        None
    }

    /// Replaces the per-field change times; does nothing on a row that has none.
    fn set_field_stamps(&mut self, _stamps: Option<FieldStamps>) {}
}

pub(crate) fn column_list(columns: &[&str]) -> String {
    columns
        .iter()
        .map(|column| format!("\"{column}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn placeholders(count: usize) -> String {
    vec!["?"; count].join(",")
}

pub(crate) fn text(value: &str) -> SqlValue {
    SqlValue::Text(value.to_owned())
}

pub(crate) fn opt_text(value: &Option<String>) -> SqlValue {
    value
        .as_ref()
        .map_or(SqlValue::Null, |text| SqlValue::Text(text.clone()))
}

pub(crate) fn int(value: i64) -> SqlValue {
    SqlValue::Integer(value)
}

pub(crate) fn opt_int(value: Option<i64>) -> SqlValue {
    value.map_or(SqlValue::Null, SqlValue::Integer)
}

pub(crate) fn flag(value: bool) -> SqlValue {
    SqlValue::Integer(i64::from(value))
}
