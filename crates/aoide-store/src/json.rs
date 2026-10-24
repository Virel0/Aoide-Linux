//! The one JSON encoding every payload in this crate uses.

use serde::Serialize;
use serde_json::{Map, Value};

/// Compact JSON with every object's keys sorted, at every depth.
///
/// Sorted by hand rather than by trusting `serde_json::Map` to be a `BTreeMap`: that is
/// only true while no crate in the build turns on `preserve_order`, and feature
/// unification would make it false without a compile error. Payloads are compared and
/// logged, so the same row has to produce the same bytes every time.
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write(value, &mut out);
    out
}

fn write(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            out.push('{');
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String((*key).clone()).to_string());
                out.push(':');
                write(&map[key.as_str()], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

/// A row as the op payload stores it.
pub(crate) fn payload<T: Serialize>(row: &T) -> serde_json::Result<String> {
    Ok(canonical_json(&serde_json::to_value(row)?))
}

/// A row as a field map, for the per-field merge.
pub(crate) fn fields<T: Serialize>(row: &T) -> serde_json::Result<Map<String, Value>> {
    match serde_json::to_value(row)? {
        Value::Object(map) => Ok(map),
        _ => Ok(Map::new()),
    }
}

/// Whether a flag column reads as set.
///
/// Both spellings, because the two ends disagree in writing: the phone encodes
/// `deleted` from a `Bool`, so it goes out as `true`, while the sync contract writes
/// the invariant as `deleted = 1` after the SQLite column.
pub(crate) fn is_flag_set(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_i64().is_some_and(|n| n != 0),
        _ => false,
    }
}

/// How deeply a document nests: a scalar is 0, `{}` is 1, `{"a":{}}` is 2.
pub(crate) fn depth(value: &Value) -> usize {
    match value {
        Value::Object(map) => 1 + map.values().map(depth).max().unwrap_or(0),
        Value::Array(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// Serde helpers for flag columns that may arrive as `true` or as `1`.
///
/// The tolerance is owed rather than generous: a peer that serialises its rows straight
/// out of SQLite writes `"deleted":1`, which clears the delete gate and would otherwise
/// fail to decode — with the pull cursor already past it, so the deletion is lost for
/// good. Integers only; a string or a float is still a type error.
pub(crate) mod flag {
    use serde::de::{self, Deserializer, Visitor};
    use std::fmt;

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
        deserializer.deserialize_any(FlagVisitor)
    }

    struct FlagVisitor;

    impl Visitor<'_> for FlagVisitor {
        type Value = bool;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a boolean, or an integer standing for one")
        }

        fn visit_bool<E: de::Error>(self, value: bool) -> Result<bool, E> {
            Ok(value)
        }

        fn visit_i64<E: de::Error>(self, value: i64) -> Result<bool, E> {
            Ok(value != 0)
        }

        fn visit_u64<E: de::Error>(self, value: u64) -> Result<bool, E> {
            Ok(value != 0)
        }
    }
}
