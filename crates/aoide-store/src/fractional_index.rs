/// Sortable position keys that can be inserted between without renumbering.
///
/// With integer positions, inserting at row 3 rewrites every row below it, so two
/// devices inserting at once produce dozens of conflicting updates and a merge drops
/// one of the inserts. A fractional key means an insert writes exactly **one** row.
/// Keys compare as plain strings, so the database can `ORDER BY position`.
///
/// The same algorithm, and so the same keys, as the phone and the desktop.
pub struct FractionalIndex;

/// Digits before uppercase before lowercase, which is already ASCII order.
const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

impl FractionalIndex {
    pub const ALPHABET: &'static str =
        "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

    fn base() -> usize {
        ALPHABET.len()
    }

    /// A key for the first item in an empty list.
    pub fn initial() -> String {
        Self::between(None, None)
    }

    /// A key that sorts strictly between `lower` and `upper`; `None` is unbounded.
    /// Callers must pass `lower < upper` (the store checks this before calling).
    pub fn between(lower: Option<&str>, upper: Option<&str>) -> String {
        debug_assert!(
            !matches!((lower, upper), (Some(l), Some(u)) if l >= u),
            "FractionalIndex::between requires lower < upper"
        );

        // Appending is the common case, and bisecting towards the ceiling makes keys
        // grow a character every few appends; incrementing makes it behave like
        // counting.
        let Some(upper) = upper else {
            return Self::increment(lower);
        };

        let mut result: Vec<u8> = Vec::new();
        let mut bounded_above = Some(upper);
        let mut index = 0;
        loop {
            let low = digit(lower, index).unwrap_or(0);
            let high = digit(bounded_above, index).unwrap_or(Self::base());

            if high - low > 1 {
                // The midpoint is never digit zero here, so a generated key never ends
                // in the smallest digit and there is always room to insert before it.
                result.push(ALPHABET[(low + high) / 2]);
                return String::from_utf8(result).unwrap_or_default();
            }

            // Equal or adjacent: keep the lower digit and go deeper.
            result.push(ALPHABET[low]);
            // Having taken a digit strictly below `upper`'s, every extension already
            // sorts below it, so it stops constraining the search.
            if low < high {
                bounded_above = None;
            }
            index += 1;
        }
    }

    /// Keys for `count` items appended after `lower`, in order.
    pub fn sequence(after: Option<&str>, count: usize) -> Vec<String> {
        let mut keys = Vec::with_capacity(count);
        let mut previous: Option<String> = after.map(str::to_owned);
        for _ in 0..count {
            let key = Self::between(previous.as_deref(), None);
            keys.push(key.clone());
            previous = Some(key);
        }
        keys
    }

    /// The next key after `key`, and the shortest one available.
    fn increment(key: Option<&str>) -> String {
        let middle = ALPHABET[Self::base() / 2] as char;
        let Some(key) = key.filter(|key| !key.is_empty()) else {
            return middle.to_string();
        };

        let mut characters: Vec<u8> = key.bytes().collect();
        let mut index = characters.len();
        while index > 0 {
            index -= 1;
            let value = ALPHABET
                .iter()
                .position(|c| *c == characters[index])
                .unwrap_or(0);
            if value + 1 < Self::base() {
                characters[index] = ALPHABET[value + 1];
                // Truncated: "Vz" becomes "W" rather than "Wz".
                characters.truncate(index + 1);
                return String::from_utf8(characters).unwrap_or_default();
            }
        }
        // Every digit is already the largest; the key has to get longer.
        format!("{key}{middle}")
    }
}

fn digit(key: Option<&str>, index: usize) -> Option<usize> {
    let byte = *key?.as_bytes().get(index)?;
    ALPHABET.iter().position(|c| *c == byte)
}
