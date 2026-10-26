//! Why a rule or rule set was refused.

use super::rule::{Field, Operator};

/// Why a rule or rule set was refused, or a playlist's rules could not be run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SmartPlaylistError {
    /// A field name that is neither a wire name nor an accepted alias.
    #[error("no such rule field: {0}")]
    UnknownField(String),
    /// An operator name that is neither a wire name nor a comparison symbol.
    #[error("no such rule operator: {0}")]
    UnknownOperator(String),
    /// The operator does not compare that kind of field (`contains` on a year).
    #[error("{} cannot apply to {}", op.wire_name(), field.wire_name())]
    OperatorNotApplicable {
        /// The rule's field.
        field: Field,
        /// The operator it was given.
        op: Operator,
    },
    /// The value is the wrong type for the field and operator (text for a year).
    #[error("the value does not suit {} {}", field.wire_name(), op.wire_name())]
    ValueTypeMismatch {
        /// The rule's field.
        field: Field,
        /// The rule's operator.
        op: Operator,
    },
    /// A date value that does not parse as a [`super::RelativeInterval`].
    #[error("{0} is not a relative interval; expected something like -30d, -6m or -1y")]
    MalformedInterval(String),
    /// A JSON value that is not a string, a whole number or a boolean.
    #[error("a rule value must be a string, a whole number or a boolean")]
    MalformedValue,
    /// `any` of no rules, which could only ever be empty.
    #[error("matching any of no rules can never match anything")]
    NoRules,
    /// A limit of zero or less.
    #[error("a limit of {0} can never return anything")]
    NonPositiveLimit(i64),
    /// A sort on a field with several values per track (genre).
    #[error("{} has no single value to sort by", .0.wire_name())]
    FieldNotSortable(Field),
    /// The playlist is not smart, or its rules are empty.
    #[error("playlist {playlist_id} carries no smart rules")]
    NotSmart {
        /// The playlist asked about.
        playlist_id: String,
    },
    /// The document is not shaped like a rule set at all.
    #[error("not a rule set: {0}")]
    MalformedJson(String),
}
