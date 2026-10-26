//! Why a rule or rule set was refused.

use super::rule::{Field, Operator};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SmartPlaylistError {
    #[error("no such rule field: {0}")]
    UnknownField(String),
    #[error("no such rule operator: {0}")]
    UnknownOperator(String),
    #[error("{} cannot apply to {}", op.wire_name(), field.wire_name())]
    OperatorNotApplicable { field: Field, op: Operator },
    #[error("the value does not suit {} {}", field.wire_name(), op.wire_name())]
    ValueTypeMismatch { field: Field, op: Operator },
    #[error("{0} is not a relative interval; expected something like -30d, -6m or -1y")]
    MalformedInterval(String),
    #[error("a rule value must be a string, a whole number or a boolean")]
    MalformedValue,
    #[error("matching any of no rules can never match anything")]
    NoRules,
    #[error("a limit of {0} can never return anything")]
    NonPositiveLimit(i64),
    #[error("{} has no single value to sort by", .0.wire_name())]
    FieldNotSortable(Field),
    #[error("playlist {playlist_id} carries no smart rules")]
    NotSmart { playlist_id: String },
    /// The document is not shaped like a rule set at all.
    #[error("not a rule set: {0}")]
    MalformedJson(String),
}
