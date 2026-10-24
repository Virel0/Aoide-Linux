//! Port of `FractionalIndexTests.swift` ("Fractional index").

use std::collections::HashSet;

use crate::FractionalIndex;

#[test]
fn a_key_lands_strictly_between_its_neighbours() {
    let a = FractionalIndex::initial();
    let b = FractionalIndex::between(Some(&a), None);
    let mid = FractionalIndex::between(Some(&a), Some(&b));
    assert!(a < mid);
    assert!(mid < b);
}

#[test]
fn appending_stays_ordered() {
    let keys = FractionalIndex::sequence(None, 50);
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
    assert_eq!(keys.iter().collect::<HashSet<_>>().len(), 50);
}

/// Nothing sorts between "" and "0", so a key of "0" could never be preceded.
#[test]
fn there_is_always_room_to_insert_before_the_first_key() {
    let mut first = FractionalIndex::initial();
    for _ in 0..200 {
        let earlier = FractionalIndex::between(None, Some(&first));
        assert!(earlier < first, "no room left before {first}");
        first = earlier;
    }
}

#[test]
fn no_generated_key_ends_in_the_smallest_digit() {
    let mut keys = vec![FractionalIndex::initial()];
    for _ in 0..200 {
        let before = FractionalIndex::between(None, Some(keys.last().unwrap()));
        keys.push(before);
        let after = FractionalIndex::between(Some(&keys[0]), None);
        keys.push(after);
    }
    let smallest = FractionalIndex::ALPHABET.chars().next().unwrap();
    for key in keys {
        assert_ne!(
            key.chars().last(),
            Some(smallest),
            "{key} ends in the smallest digit"
        );
    }
}

#[test]
fn repeated_insertion_into_one_gap_keeps_working() {
    let mut low = FractionalIndex::initial();
    let high = FractionalIndex::between(Some(&low), None);
    for step in 0..200 {
        let mid = FractionalIndex::between(Some(&low), Some(&high));
        assert!(low < mid, "step {step}: {mid} not above {low}");
        assert!(mid < high, "step {step}: {mid} not below {high}");
        low = mid;
    }
}

/// Integer positions are exactly what this would fail on.
#[test]
fn concurrent_inserts_at_the_same_spot_both_survive() {
    let a = FractionalIndex::initial();
    let b = FractionalIndex::between(Some(&a), None);
    let one = FractionalIndex::between(Some(&a), Some(&b));
    let two = FractionalIndex::between(Some(&a), Some(&b));
    assert_eq!(one, two);
    assert!(a < one && one < b);
}

#[test]
fn a_whole_list_stays_sorted_under_random_insertion() {
    let mut keys = FractionalIndex::sequence(None, 10);
    for step in 0..100 {
        let at = (step * 7) % (keys.len() + 1);
        let lower = if at > 0 {
            Some(keys[at - 1].clone())
        } else {
            None
        };
        let upper = keys.get(at).cloned();
        let key = FractionalIndex::between(lower.as_deref(), upper.as_deref());
        keys.insert(at, key);
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "step {step} broke the ordering");
    }
    assert_eq!(keys.iter().collect::<HashSet<_>>().len(), keys.len());
}

/// An earlier version bisected towards the ceiling and reached 167 characters.
#[test]
fn keys_stay_short_enough_to_store_cheaply() {
    let longest = FractionalIndex::sequence(None, 1000)
        .iter()
        .map(String::len)
        .max()
        .unwrap_or(0);
    assert!(
        longest <= 40,
        "append-only keys grew to {longest} characters"
    );
}

/// Not in the Swift suite: the keys themselves, worked by hand from the Swift
/// algorithm, so a drift in this port shows up as a value rather than an ordering.
#[test]
fn the_first_keys_are_the_phones() {
    assert_eq!(FractionalIndex::initial(), "V");
    assert_eq!(FractionalIndex::sequence(None, 3), ["V", "W", "X"]);
    assert_eq!(FractionalIndex::between(Some("V"), Some("W")), "VV");
    assert_eq!(FractionalIndex::between(None, Some("V")), "F");
    assert_eq!(FractionalIndex::between(Some("Vz"), None), "W");
}
