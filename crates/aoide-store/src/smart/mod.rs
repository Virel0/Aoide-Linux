//! Smart playlists: rules stored as JSON on the playlist, evaluated locally in SQL.
//!
//! An illegal field/operator pair cannot be constructed, from code or from JSON: the
//! cheap implementation of "unsupported" is a predicate that matches nothing, and a
//! smart playlist that is quietly empty looks exactly like one whose rules have no
//! hits. Every rule *value* is a bound parameter and every field and operator maps to
//! SQL through a `match` over a closed enum, so nothing typed reaches SQLite as source.

mod error;
mod evaluate;
mod interval;
mod rule;
mod rule_set;
mod sql;

pub use error::SmartPlaylistError;
pub(crate) use interval::resolve_local;
pub use interval::{Calendar, RelativeInterval, TimeUnit};
pub use rule::{Field, Operator, SmartRule, Value};
pub use rule_set::{Match, SmartRules, Sort, SortDirection};
