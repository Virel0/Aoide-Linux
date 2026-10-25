//! Reading a model's reply, which is JSON more often than not.

use serde_json::{Map, Value};

/// The JSON object inside a reply that may not be only JSON.
///
/// Models wrap answers in prose or a fenced code block however firmly they
/// are asked not to, and a reply that is 95% right should not be lost to its
/// packaging. Tries the whole reply, then the span from the first `{` to the
/// last `}`. An array or a scalar is not an answer.
pub(crate) fn extract_object(reply: &str) -> Option<Map<String, Value>> {
    let whole = std::iter::once(reply);
    let span = match (reply.find('{'), reply.rfind('}')) {
        (Some(start), Some(end)) if start <= end => Some(&reply[start..=end]),
        _ => None,
    };
    whole
        .chain(span)
        .find_map(|candidate| match serde_json::from_str(candidate) {
            Ok(Value::Object(object)) => Some(object),
            _ => None,
        })
}

/// A whole number from a JSON number, as JavaScript's `Number.isInteger`
/// sees it: `3` and `3.0` are, `2.5` is not.
pub(crate) fn whole_number(value: &Value) -> Option<i64> {
    let number = value.as_number()?;
    if let Some(n) = number.as_i64() {
        return Some(n);
    }
    let f = number.as_f64()?;
    // Within ±2^53 every whole f64 is exact, and so is the cast.
    (f.fract() == 0.0 && f.abs() <= 9_007_199_254_740_992.0).then_some(f as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finds_the_object_in_prose_and_fences() {
        let object = json!({"a": 1});
        for reply in [
            r#"{"a":1}"#,
            "```json\n{\"a\":1}\n```",
            "Sure! Here you go:\n{\"a\": 1}\nEnjoy.",
        ] {
            assert_eq!(
                extract_object(reply).map(Value::Object),
                Some(object.clone())
            );
        }
    }

    #[test]
    fn refuses_what_is_not_an_object() {
        for reply in ["", "no", "[1, 2]", "\"text\"", "}{", "{ broken"] {
            assert_eq!(extract_object(reply), None, "{reply:?}");
        }
    }

    #[test]
    fn whole_numbers_as_javascript_counts_them() {
        assert_eq!(whole_number(&json!(3)), Some(3));
        assert_eq!(whole_number(&json!(3.0)), Some(3));
        assert_eq!(whole_number(&json!(-2)), Some(-2));
        assert_eq!(whole_number(&json!(2.5)), None);
        assert_eq!(whole_number(&json!("3")), None);
        assert_eq!(whole_number(&json!(true)), None);
    }
}
