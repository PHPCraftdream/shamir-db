//! Basic interner lifecycle, state initialisation, and edge-case tests.

use crate::core::interner::{Interner, InternerKey, UserKey};
use std::sync::Arc;
use std::thread;

/// Helper: make an `Arc<str>` for comparison with `get_str` results.
pub(super) fn arc_str(s: &str) -> Arc<str> {
    Arc::from(s)
}

#[test]
fn test_basic_interning() {
    let interner = Interner::new();
    let id1 = interner.touch_ind("hello").unwrap();
    let id2 = interner.touch_ind("world").unwrap();
    let id3 = interner.touch_ind("hello").unwrap();

    assert!(id1.is_new());
    assert!(id2.is_new());
    assert!(!id3.is_new());

    // IDs are now 1, 2, 1 (starting from 1, not 0)
    assert_eq!(id1.key().id(), 1);
    assert_eq!(id2.key().id(), 2);
    assert_eq!(id3.key().id(), 1); // Same as id1

    assert_eq!(
        interner.get_str(&InternerKey::new(1)),
        Some(arc_str("hello"))
    );
    assert_eq!(
        interner.get_str(&InternerKey::new(2)),
        Some(arc_str("world"))
    );
    assert_eq!(interner.get_ind("world"), Some(InternerKey::new(2)));
}

#[test]
fn test_with_state_initialization() {
    let initial_data = vec![
        (InternerKey::new(1), UserKey::from_str("name")),
        (InternerKey::new(50), UserKey::from_str("age")),
        (InternerKey::new(100), UserKey::from_str("city")),
    ];
    let interner = Interner::with_state(initial_data);

    // Check that initial data is loaded correctly
    assert_eq!(interner.get_ind("name"), Some(InternerKey::new(1)));
    assert_eq!(
        interner.get_str(&InternerKey::new(50)),
        Some(arc_str("age"))
    );
    assert_eq!(interner.get_ind("city"), Some(InternerKey::new(100)));

    // Check that touching an existing key returns correct ID
    let touch_existing = interner.touch_ind("name").unwrap();
    assert!(!touch_existing.is_new());
    assert_eq!(touch_existing.key().id(), 1);

    // Check that next ID is correctly assigned
    let next_id = interner.touch_ind("new_key").unwrap();
    assert!(next_id.is_new());
    assert_eq!(next_id.key().id(), 101);
}

#[test]
fn test_interned_key_size() {
    // Test that InternedKey uses minimal size based on id value
    let key_u8 = InternerKey::new(42);
    assert_eq!(key_u8.wire_len(), 1);

    let key_u16 = InternerKey::new(256);
    assert_eq!(key_u16.wire_len(), 2);

    let key_u32 = InternerKey::new(65536);
    assert_eq!(key_u32.wire_len(), 4);

    let key_u64 = InternerKey::new(4294967296);
    assert_eq!(key_u64.wire_len(), 8);
}

#[test]
fn test_interned_key_equality_by_id() {
    // Keys with different byte sizes but same id should be equal
    let key1 = InternerKey::new(42);
    let key2 = InternerKey::new(42);
    assert_eq!(key1, key2);
    assert_eq!(key1.id(), key2.id());
}

#[test]
fn test_edge_cases_empty_and_unicode() {
    let interner = Interner::new();

    // Empty string
    let id1 = interner.touch_ind("").unwrap();
    assert_eq!(id1.key().id(), 1);
    assert_eq!(interner.get_ind(""), Some(InternerKey::new(1)));
    assert_eq!(interner.get_str(&InternerKey::new(1)), Some(arc_str("")));

    // Unicode strings
    let unicode_keys = vec!["привет", "🚀🎉🔥", "مرحبا", "مرحبا2", "😀😃😄😁"];

    for key in &unicode_keys {
        interner.touch_ind(key).unwrap();
    }

    // Verify unicode keys work
    assert_eq!(interner.get_ind("привет"), Some(InternerKey::new(2)));
    assert_eq!(interner.get_ind("🚀🎉🔥"), Some(InternerKey::new(3)));
    assert_eq!(interner.get_ind("مرحبا"), Some(InternerKey::new(4)));
    assert_eq!(
        interner.get_str(&InternerKey::new(5)),
        Some(arc_str("مرحبا2"))
    );
    assert_eq!(interner.get_ind("😀😃😄😁"), Some(InternerKey::new(6)));
}

#[test]
fn test_edge_cases_very_long_keys() {
    let interner = Interner::new();

    // Very long key (10KB)
    let long_key = "a".repeat(10_000);
    let id = interner.touch_ind(&long_key).unwrap();
    assert_eq!(id.key().id(), 1);
    assert_eq!(interner.get_ind(&long_key), Some(InternerKey::new(1)));
    assert_eq!(
        interner.get_str(&InternerKey::new(1)),
        Some(arc_str(&long_key))
    );
}

#[test]
fn test_concurrent_with_state() {
    let initial_data: Vec<(InternerKey, UserKey)> = (0..100)
        .map(|i| {
            (
                InternerKey::new(i + 1),
                UserKey::from_str(format!("initial_{}", i)),
            )
        })
        .collect();

    let interner = Arc::new(Interner::with_state(initial_data));
    let num_threads = 20;
    let mut handles = vec![];

    for i in 0..num_threads {
        let interner_clone = Arc::clone(&interner);
        handles.push(thread::spawn(move || {
            for j in 0..50 {
                let key = format!("thread_{}", i * 50 + j);
                interner_clone.touch_ind(key).unwrap();
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    // Initial 100 + 20*50 new = 1100
    assert_eq!(interner.len(), 1100);

    // Verify initial data still accessible
    assert_eq!(interner.get_ind("initial_0"), Some(InternerKey::new(1)));
    assert_eq!(interner.get_ind("initial_99"), Some(InternerKey::new(100)));
    assert_eq!(
        interner.get_str(&InternerKey::new(1)),
        Some(arc_str("initial_0"))
    );
}

#[test]
fn test_len_and_is_empty() {
    let interner = Interner::new();
    assert_eq!(interner.len(), 0);
    assert!(interner.is_empty());

    interner.touch_ind("a").unwrap();
    interner.touch_ind("b").unwrap();
    assert_eq!(interner.len(), 2);
    assert!(!interner.is_empty());
}
