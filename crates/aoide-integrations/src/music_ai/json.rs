//! Whole numbers in a rule set, read as the Electron app read them.

use serde_json::Value;

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
    fn whole_numbers_as_javascript_counts_them() {
        assert_eq!(whole_number(&json!(3)), Some(3));
        assert_eq!(whole_number(&json!(3.0)), Some(3));
        assert_eq!(whole_number(&json!(-2)), Some(-2));
        assert_eq!(whole_number(&json!(2.5)), None);
        assert_eq!(whole_number(&json!("3")), None);
        assert_eq!(whole_number(&json!(true)), None);
    }
}
