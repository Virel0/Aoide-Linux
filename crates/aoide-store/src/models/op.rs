//! The outbound op log: one row per local change waiting to be pushed.

use rusqlite::Row;
use rusqlite::types::Value as SqlValue;

use crate::Millis;
use crate::new_id;
use crate::record::{Record, flag, int, text};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpKind {
    Upsert,
    Delete,
}

impl OpKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Upsert => "upsert",
            Self::Delete => "delete",
        }
    }

    /// Case-sensitive, as the server is: `UPSERT` is not an operation.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "upsert" => Some(Self::Upsert),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }
}

/// One outbound change, written in the same transaction as the row it describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Op {
    pub op_id: String,
    /// The local table name; translated to the server's spelling at the wire.
    pub entity: String,
    pub entity_id: String,
    /// Stored as text, so a row written by anything else can hold anything.
    pub operation: String,
    /// The row as canonical JSON.
    pub payload: String,
    pub created_at: Millis,
    pub synced: bool,
}

impl Op {
    pub fn new(
        entity: impl Into<String>,
        entity_id: impl Into<String>,
        operation: OpKind,
        payload: impl Into<String>,
        created_at: Millis,
    ) -> Self {
        Self {
            op_id: new_id(),
            entity: entity.into(),
            entity_id: entity_id.into(),
            operation: operation.as_str().to_owned(),
            payload: payload.into(),
            created_at,
            synced: false,
        }
    }

    pub fn with_id(mut self, op_id: impl Into<String>) -> Self {
        self.op_id = op_id.into();
        self
    }
}

impl Record for Op {
    const TABLE: &'static str = "op";
    const KEY: &'static str = "opId";
    const COLUMNS: &'static [&'static str] = &[
        "opId",
        "entity",
        "entityId",
        "operation",
        "payload",
        "createdAt",
        "synced",
    ];

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            op_id: row.get("opId")?,
            entity: row.get("entity")?,
            entity_id: row.get("entityId")?,
            operation: row.get("operation")?,
            payload: row.get("payload")?,
            created_at: row.get("createdAt")?,
            synced: row.get("synced")?,
        })
    }

    fn values(&self) -> Vec<SqlValue> {
        vec![
            text(&self.op_id),
            text(&self.entity),
            text(&self.entity_id),
            text(&self.operation),
            text(&self.payload),
            int(self.created_at),
            flag(self.synced),
        ]
    }

    fn key(&self) -> &str {
        &self.op_id
    }
}
