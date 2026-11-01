//! Entry-listing API and lookup-miss tests.

use crate::core::interner::{Interner, InternerKey};

#[test]
fn test_with_state_empty() {
    let interner = Interner::with_state(vec![]).unwrap();
    assert!(interner.is_empty());
    assert_eq!(interner.len(), 0);
}

#[test]
fn max_published_id_does_not_materialize_entries() {
    let interner = Interner::new();
    interner.touch_with_id("sparse", 100).unwrap();
    assert_eq!(interner.all_entries_call_count(), 0);
    assert_eq!(interner.max_published_id(), 100);
    assert_eq!(interner.max_published_id_reverse_slots_inspected(), 101);
    assert_eq!(interner.all_entries_call_count(), 0);
    assert_eq!(interner.all_entries().len(), 1);
    assert_eq!(interner.all_entries_call_count(), 1);
}

#[test]
fn max_published_id_handles_empty_gaps_explicit_ids_and_retouch() {
    let interner = Interner::new();
    assert_eq!(interner.max_published_id(), 0);
    interner.touch_with_id("high", 100).unwrap();
    assert_eq!(interner.max_published_id(), 100);
    interner.touch_with_id("low", 3).unwrap();
    interner.touch_with_id("high", 100).unwrap();
    assert_eq!(interner.max_published_id(), 100);
    // Three max_published_id() calls: on the fresh interner (reverse len 1),
    // after touching id 100 (len 101), and after the retouch (len still 101):
    // 1 + 101 + 101 = 203.
    assert_eq!(interner.max_published_id_reverse_slots_inspected(), 203);
}

#[test]
fn test_all_entries() {
    let interner = Interner::new();
    interner.touch_ind("alpha").unwrap();
    interner.touch_ind("beta").unwrap();
    let entries = interner.all_entries();
    assert_eq!(entries.len(), 2);
    // slot 0 is sentinel, entries start at 1
    let ids: Vec<u64> = entries.iter().map(|(k, _)| k.id()).collect();
    assert!(ids.contains(&1));
    assert!(ids.contains(&2));
}

#[test]
fn test_entries_in_id_range() {
    let interner = Interner::new();
    interner.touch_ind("a").unwrap();
    interner.touch_ind("b").unwrap();
    interner.touch_ind("c").unwrap();
    interner.touch_ind("d").unwrap();

    // Range: ids > 1, ≤ 3  → ids 2, 3
    let entries = interner.entries_in_id_range(1, 3);
    assert_eq!(entries.len(), 2);
    let ids: Vec<u64> = entries.iter().map(|(k, _)| k.id()).collect();
    assert!(ids.contains(&2));
    assert!(ids.contains(&3));
}

#[test]
fn test_entries_in_id_range_empty() {
    let interner = Interner::new();
    interner.touch_ind("x").unwrap();
    // lo > hi → empty
    let entries = interner.entries_in_id_range(10, 5);
    assert!(entries.is_empty());
}

#[test]
fn test_entries_after() {
    let interner = Interner::new();
    interner.touch_ind("p").unwrap();
    interner.touch_ind("q").unwrap();
    interner.touch_ind("r").unwrap();

    let (entries, high_water) = interner.entries_after(0);
    assert_eq!(entries.len(), 3);
    assert_eq!(high_water, 3);
}

#[test]
fn test_entries_after_with_gap() {
    let interner = Interner::new();
    interner.touch_ind("a").unwrap();
    // intern next one — ids are 1, 2
    let (entries, high_water) = interner.entries_after(5);
    // lo (6) > hi_full (2) → empty
    assert!(entries.is_empty());
    assert_eq!(high_water, 5);
}

#[test]
fn test_get_str_unknown_id() {
    let interner = Interner::new();
    let unknown = InternerKey::new(999);
    assert_eq!(interner.get_str(&unknown), None);
}

#[test]
fn test_get_ind_unknown_key() {
    let interner = Interner::new();
    assert_eq!(interner.get_ind("nonexistent"), None);
}

#[test]
fn test_with_str_callback() {
    let interner = Interner::new();
    let touch = interner.touch_ind("hello").unwrap();
    let key = touch.key().clone();
    let result = interner.with_str(&key, |s| s.to_uppercase());
    assert_eq!(result, Some("HELLO".to_string()));
}

#[test]
fn test_with_str_unknown_id() {
    let interner = Interner::new();
    let unknown = InternerKey::new(999);
    assert!(interner.with_str(&unknown, |_| "called").is_none());
}

#[test]
fn test_make_key() {
    let interner = Interner::new();
    let key = interner.make_key(42);
    assert_eq!(key.id(), 42);
}

// ---------------------------------------------------------------------------
// UserKey: construction, Display, Borrow<str>, FromStr, serde
// ---------------------------------------------------------------------------

#[test]
fn interner_touch_max_epoch_does_not_materialize_all_entries() {
    let interner = Interner::new();
    interner.touch_with_id("published", 12_345).unwrap();
    assert_eq!(interner.all_entries_call_count(), 0);
    assert_eq!(interner.max_published_id(), 12_345);
    assert_eq!(interner.max_published_id_reverse_slots_inspected(), 12_346);
    assert_eq!(interner.all_entries_call_count(), 0);
    interner.all_entries();
    assert_eq!(interner.all_entries_call_count(), 1);
}
