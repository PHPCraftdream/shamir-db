//! Concurrency tests: parallel interning, stress, and read/write races.

use crate::core::interner::{Interner, InternerKey, TouchInd};
use crate::types::common::TMap;
use shamir_collections::TFxSet;
use std::sync::Arc;
use std::thread;

#[test]
fn test_concurrent_interning() {
    let interner = Arc::new(Interner::new());
    let mut handles = vec![];
    let keys = vec!["a", "b", "c", "d", "a", "e", "b", "f", "g", "h"];
    for _ in 0..10 {
        let interner_clone = Arc::clone(&interner);
        let keys_clone = keys.clone();
        handles.push(thread::spawn(move || {
            let mut ids = vec![];
            for key in keys_clone {
                ids.push(interner_clone.touch_ind(key).unwrap());
            }
            ids
        }));
    }
    let results: Vec<Vec<TouchInd>> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    // Each thread should get consistent results - same keys get same IDs
    // across all threads (though not necessarily in insertion order)
    let first_result = &results[0];
    for result in results.iter().skip(1) {
        let id_map_1: TMap<&str, u64> = first_result
            .iter()
            .zip(keys.iter())
            .map(|(result, key)| (*key, result.key().id()))
            .collect();
        let id_map_2: TMap<&str, u64> = result
            .iter()
            .zip(keys.iter())
            .map(|(result, key)| (*key, result.key().id()))
            .collect();

        // Verify that same keys got same IDs
        for key in ["a", "b", "c", "d", "e", "f", "g", "h"] {
            let id1 = id_map_1.get(key);
            let id2 = id_map_2.get(key);
            assert_eq!(id1, id2, "Key '{}' got different IDs", key);
        }
    }

    // Verify all keys were interned
    assert!(interner.get_ind("a").is_some());
    assert!(interner.get_ind("b").is_some());
    assert!(interner.get_ind("c").is_some());
    assert!(interner.get_ind("d").is_some());
    assert!(interner.get_ind("e").is_some());
    assert!(interner.get_ind("f").is_some());
    assert!(interner.get_ind("g").is_some());
    assert!(interner.get_ind("h").is_some());
    assert_eq!(interner.len(), 8);
}

#[test]
fn test_concurrent_stress() {
    let interner = Arc::new(Interner::new());
    let num_threads = 50;
    let keys_per_thread = 100;
    let mut handles = vec![];

    for thread_id in 0..num_threads {
        let interner_clone = Arc::clone(&interner);
        handles.push(thread::spawn(move || {
            for i in 0..keys_per_thread {
                let key = format!("thread_{}_key_{}", thread_id, i);
                interner_clone.touch_ind(key).unwrap();
            }
        }));
    }

    // Wait for all threads
    for handle in handles {
        handle.join().unwrap();
    }

    // Verify all keys were interned correctly
    let final_count = interner.len();
    assert_eq!(final_count, num_threads * keys_per_thread);

    // Verify that keys from different threads were interned
    assert!(interner.get_ind("thread_0_key_0").is_some());
    assert!(interner.get_ind("thread_10_key_50").is_some());
    assert!(interner.get_ind("thread_25_key_75").is_some());
    assert!(interner.get_ind("thread_49_key_99").is_some());
}

#[test]
fn test_concurrent_read_while_write() {
    let interner = Arc::new(Interner::new());
    let mut handles = vec![];

    // Writer threads
    for i in 0..10 {
        let interner_clone = Arc::clone(&interner);
        handles.push(thread::spawn(move || {
            for j in 0..50 {
                let key = format!("write_{}_{}", i, j);
                interner_clone.touch_ind(key).unwrap();
            }
        }));
    }

    // Reader threads
    for _i in 0..10 {
        let interner_clone = Arc::clone(&interner);
        handles.push(thread::spawn(move || {
            for _ in 0..100 {
                let _ = interner_clone.get_ind("write_0_0");
                let _ = interner_clone.get_str(&InternerKey::new(1));
                let _ = interner_clone.get_ind("nonexistent");
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    // Should have 500 unique keys (10 writers * 50 keys)
    assert_eq!(interner.len(), 500);
}

#[test]
fn test_concurrent_same_key_consistency() {
    // Property under test: for any given key, *every* thread that touches
    // it observes the *same* ID — regardless of who races in first.
    //
    // The previous version of this test asserted a hard-coded sequence
    // [1,2,3,1,2], which only holds if "shared1" is the very first key
    // committed across all threads. Under real contention any of the
    // three keys can win the race, so that assertion was flaky (~40 %
    // failure rate). What actually matters is *consistency*, not the
    // specific numeric values.
    let interner = Arc::new(Interner::new());
    let num_threads = 100;
    let pattern = ["shared1", "shared2", "shared3", "shared1", "shared2"];
    let mut handles = vec![];

    for _ in 0..num_threads {
        let interner_clone = Arc::clone(&interner);
        handles.push(thread::spawn(move || {
            pattern
                .iter()
                .map(|k| interner_clone.touch_ind(k).unwrap().key().id())
                .collect::<Vec<u64>>()
        }));
    }

    let results: Vec<Vec<u64>> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    // Within every thread the repeated keys (shared1 at indices 0/3,
    // shared2 at indices 1/4) must collapse to the same ID — basic
    // interner contract.
    for r in &results {
        assert_eq!(r[0], r[3], "shared1 inconsistent within thread: {:?}", r);
        assert_eq!(r[1], r[4], "shared2 inconsistent within thread: {:?}", r);
    }

    // Across threads: each key must map to one and the same ID
    // everywhere. Use thread 0 as the reference.
    let reference = &results[0];
    for (i, r) in results.iter().enumerate().skip(1) {
        assert_eq!(r[0], reference[0], "shared1 ID differs in thread {i}");
        assert_eq!(r[1], reference[1], "shared2 ID differs in thread {i}");
        assert_eq!(r[2], reference[2], "shared3 ID differs in thread {i}");
    }

    // The three keys must be distinct.
    let unique: TFxSet<u64> = [reference[0], reference[1], reference[2]]
        .into_iter()
        .collect();
    assert_eq!(
        unique.len(),
        3,
        "three distinct keys must produce three IDs"
    );

    // Final state.
    assert_eq!(interner.len(), 3);
}

#[test]
fn test_concurrent_reverse_lookup() {
    let interner = Arc::new(Interner::new());
    let num_threads = 20;
    let mut handles = vec![];

    // Populate first and collect actual IDs
    let mut key_to_id: Vec<(String, InternerKey)> = vec![];
    for i in 0..100 {
        let key = format!("key_{}", i);
        let touch_result = interner.touch_ind(key.clone()).unwrap();
        key_to_id.push((key, touch_result.key().clone()));
    }

    // Create a mapping for easy lookup
    let id_lookup: TMap<InternerKey, String> = key_to_id
        .iter()
        .map(|(k, v)| (v.clone(), k.clone()))
        .collect();

    // Concurrent reverse lookups
    for _i in 0..num_threads {
        let interner_clone = Arc::clone(&interner);
        let id_lookup_clone = id_lookup.clone();
        handles.push(thread::spawn(move || {
            for (id, expected_key) in id_lookup_clone {
                let key = interner_clone.get_str(&id);
                assert!(key.is_some(), "Failed to look up ID: {}", id.id());
                assert_eq!(key.as_deref(), Some(expected_key.as_str()));
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }
}

#[test]
fn test_concurrent_touch_and_get() {
    let interner = Arc::new(Interner::new());
    let num_threads = 30;
    let mut handles = vec![];

    for i in 0..num_threads {
        let interner_clone = Arc::clone(&interner);
        handles.push(thread::spawn(move || {
            for j in 0..50 {
                let key = format!("key_{}_{}", i, j);

                // Touch key
                let touch_result = interner_clone.touch_ind(&key).unwrap();

                // Immediately verify with get_ind
                let get_result = interner_clone.get_ind(&key);

                assert_eq!(Some(touch_result.key().id()), get_result.map(|k| k.id()));

                // Also verify reverse lookup
                let reverse = interner_clone.get_str(touch_result.key());
                assert_eq!(reverse.as_deref(), Some(key.as_str()));
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    // Total: 30 threads * 50 keys = 1500
    assert_eq!(interner.len(), 1500);
}
