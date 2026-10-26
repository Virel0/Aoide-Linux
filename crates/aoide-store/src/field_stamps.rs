//! When each field of a row was last changed.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ValueRef};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::Millis;
use crate::json;
use crate::record::SyncRecord;

/// When each field of a row was last changed.
///
/// Row-level last-writer-wins throws away one of two edits whenever they touch the
/// same row, however unrelated: rename a playlist on the phone while adding a
/// description on the laptop, and whichever synced second erases the other. Stamping
/// fields individually lets both survive.
///
/// Carried *inside* the payload, which is why this needed no server change: the
/// sidecar stores payloads verbatim. Rows written before this existed carry no stamps
/// and fall back to the row's `updatedAt`, so older ops keep working.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FieldStamps(pub BTreeMap<String, Millis>);

impl FieldStamps {
    /// No stamps: every field falls back to the row's `updatedAt`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Stamps from `(field, time)` pairs, for tests and fixtures.
    pub fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, Millis)>) -> Self {
        Self(
            pairs
                .into_iter()
                .map(|(field, time)| (field.to_owned(), time))
                .collect(),
        )
    }

    /// When `field` last changed, or `fallback` for a row written before stamps existed.
    pub fn time(&self, field: &str, fallback: Millis) -> Millis {
        self.0.get(field).copied().unwrap_or(fallback)
    }

    /// When `field` last changed, if it has been stamped.
    pub fn get(&self, field: &str) -> Option<Millis> {
        self.0.get(field).copied()
    }

    /// Stamps each of `fields` with `now`.
    pub fn stamp<'a>(&mut self, fields: impl IntoIterator<Item = &'a String>, now: Millis) {
        for field in fields {
            self.0.insert(field.clone(), now);
        }
    }

    /// Whether no field has been stamped.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Drags any stamp that fails `credible` back to a time the server witnessed.
    ///
    /// These come from the same clock as the row's `updatedAt` and need the same
    /// correction; correcting only the row timestamp would let a device stamping 2096
    /// lose the row comparison and then win every field anyway.
    pub(crate) fn correcting_skew(
        &mut self,
        credible: impl Fn(Millis) -> bool,
        to: Millis,
    ) -> bool {
        let mut corrected = false;
        for time in self.0.values_mut() {
            if !credible(*time) {
                *time = to;
                corrected = true;
            }
        }
        corrected
    }

    pub(crate) fn to_column(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_owned())
    }
}

impl FromSql for FieldStamps {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let text = value.as_str()?;
        serde_json::from_str(text).map_err(|error| FromSqlError::Other(Box::new(error)))
    }
}

/// Columns that carry no content and are never merged on their own: they come from
/// whichever side wins the row as a whole. A soft delete is a fact about the row, and
/// merging it per field invites a half-deleted row.
pub const METADATA_COLUMNS: [&str; 5] = [
    "id",
    "updatedAt",
    "deleted",
    "originDevice",
    "fieldUpdatedAt",
];

/// Which fields differ between two versions of a row, read off their encoded form.
///
/// Every key of the new row is compared, metadata included, because that is what the
/// phone stamps — a row's stamps carry `updatedAt` and friends too, and a row written
/// here has to stamp exactly what the phone's would.
pub(crate) fn changed_fields<R: SyncRecord>(
    new: &R,
    old: Option<&R>,
) -> serde_json::Result<Vec<String>> {
    let new_fields = json::fields(new)?;
    let Some(old) = old else {
        return Ok(new_fields.keys().cloned().collect());
    };
    let old_fields = json::fields(old)?;
    Ok(new_fields
        .iter()
        .filter(|(key, value)| old_fields.get(*key) != Some(*value))
        .map(|(key, _)| key.clone())
        .collect())
}

/// Builds one row from two, taking each content field from whichever side changed it
/// more recently and the metadata from `winner`.
///
/// Works through the encoded form rather than a hand-written field list, so a column
/// added later is merged without anybody remembering to add it here.
pub(crate) fn merged<R: SyncRecord>(
    incoming: &R,
    existing: &R,
    winner: &R,
) -> serde_json::Result<R> {
    let incoming_fields = json::fields(incoming)?;
    let existing_fields = json::fields(existing)?;
    let mut result: Map<String, Value> = json::fields(winner)?;

    let mut stamps = winner.field_stamps().cloned().unwrap_or_default();
    let incoming_stamps = incoming.field_stamps().cloned().unwrap_or_default();
    let existing_stamps = existing.field_stamps().cloned().unwrap_or_default();

    let keys: BTreeSet<&String> = incoming_fields
        .keys()
        .chain(existing_fields.keys())
        .collect();
    for key in keys {
        if METADATA_COLUMNS.contains(&key.as_str()) {
            continue;
        }
        let incoming_stamp = incoming_stamps.get(key);
        let existing_stamp = existing_stamps.get(key);
        let incoming_time = incoming_stamp.unwrap_or(incoming.updated_at());
        let existing_time = existing_stamp.unwrap_or(existing.updated_at());

        // Ties are left alone: `result` already holds the row-level winner's value, and
        // every device computes that winner identically.
        let source = if incoming_time > existing_time {
            Some(&incoming_fields)
        } else if existing_time > incoming_time {
            Some(&existing_fields)
        } else {
            None
        };
        if let Some(source) = source {
            match source.get(key) {
                Some(value) => {
                    result.insert(key.clone(), value.clone());
                }
                None => {
                    result.remove(key);
                }
            }
        }

        // Only where a stamp genuinely exists. Synthesising one for a field neither
        // side stamped would make re-applying an op write every time and never settle.
        if incoming_stamp.is_some() || existing_stamp.is_some() {
            stamps
                .0
                .insert(key.clone(), incoming_time.max(existing_time));
        }
    }

    let mut merged: R = serde_json::from_value(Value::Object(result))?;
    // Left absent when nothing was ever stamped: an empty set is not equal to none, and
    // merging two pre-stamp rows must produce a row equal to one of them.
    merged.set_field_stamps(if stamps.is_empty() {
        None
    } else {
        Some(stamps)
    });
    Ok(merged)
}
