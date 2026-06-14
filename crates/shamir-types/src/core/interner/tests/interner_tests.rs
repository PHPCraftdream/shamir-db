use crate::core::interner::{Interner, InternerKey, TouchInd, UserKey};
use crate::types::common::TMap;
use std::sync::Arc;
use std::thread;

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
        Some(UserKey::from_str("hello"))
    );
    assert_eq!(
        interner.get_str(&InternerKey::new(2)),
        Some(UserKey::from_str("world"))
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
        Some(UserKey::from_str("age"))
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
    let unique: std::collections::HashSet<u64> = [reference[0], reference[1], reference[2]]
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
                assert_eq!(key, Some(UserKey::from_str(expected_key)));
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
                assert_eq!(reverse, Some(UserKey::from_str(key.as_str())));
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    // Total: 30 threads * 50 keys = 1500
    assert_eq!(interner.len(), 1500);
}

#[test]
fn test_edge_cases_empty_and_unicode() {
    let interner = Interner::new();

    // Empty string
    let id1 = interner.touch_ind("").unwrap();
    assert_eq!(id1.key().id(), 1);
    assert_eq!(interner.get_ind(""), Some(InternerKey::new(1)));
    assert_eq!(
        interner.get_str(&InternerKey::new(1)),
        Some(UserKey::from_str(""))
    );

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
        Some(UserKey::from_str("مرحبا2"))
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
        Some(UserKey::from_str(long_key.clone()))
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
        Some(UserKey::from_str("initial_0"))
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

#[test]
fn test_interned_key_serialization() {
    // Test that InternedKey serializes/deserializes correctly
    let key1 = InternerKey::new(42);
    let bytes1 = rmp_serde::to_vec(&key1).unwrap();
    let decoded1: InternerKey = rmp_serde::from_slice(&bytes1).unwrap();
    assert_eq!(key1.id(), decoded1.id());
    assert_eq!(key1, decoded1);

    let key2 = InternerKey::new(1000);
    let bytes2 = rmp_serde::to_vec(&key2).unwrap();
    let decoded2: InternerKey = rmp_serde::from_slice(&bytes2).unwrap();
    assert_eq!(key2.id(), decoded2.id());
    assert_eq!(key2, decoded2);

    let key3 = InternerKey::new(100000);
    let bytes3 = rmp_serde::to_vec(&key3).unwrap();
    let decoded3: InternerKey = rmp_serde::from_slice(&bytes3).unwrap();
    assert_eq!(key3.id(), decoded3.id());
    assert_eq!(key3, decoded3);
}

#[test]
fn test_interned_key_compact_messagepack_serialization() {
    // Test that InternedKey serializes compactly in MessagePack (not as full u64)
    println!("=== Testing InternedKey compact MessagePack serialization ===\n");

    // Create keys with different sizes (auto-determined by id value)
    let key_u8 = InternerKey::new(42);
    let key_u16 = InternerKey::new(1000);
    let key_u32 = InternerKey::new(70000);
    let key_u64 = InternerKey::new(5000000000);

    println!("Raw key sizes:");
    println!("  U8: {} bytes", key_u8.wire_len());
    println!("  U16: {} bytes", key_u16.wire_len());
    println!("  U32: {} bytes", key_u32.wire_len());
    println!("  U64: {} bytes", key_u64.wire_len());
    println!();

    // Serialize to MessagePack
    let bytes_u8 = rmp_serde::to_vec(&key_u8).expect("Failed to serialize");
    let bytes_u16 = rmp_serde::to_vec(&key_u16).expect("Failed to serialize");
    let bytes_u32 = rmp_serde::to_vec(&key_u32).expect("Failed to serialize");
    let bytes_u64 = rmp_serde::to_vec(&key_u64).expect("Failed to serialize");

    println!("MessagePack serialized sizes:");
    println!("  U8: {} bytes - {:?}", bytes_u8.len(), bytes_u8);
    println!("  U16: {} bytes - {:?}", bytes_u16.len(), bytes_u16);
    println!("  U32: {} bytes - {:?}", bytes_u32.len(), bytes_u32);
    println!("  U64: {} bytes - {:?}", bytes_u64.len(), bytes_u64);
    println!();

    // MessagePack bin8 format: 0xC4 (marker) + 1 byte length + data
    // U8 (1 byte data): 0xC4 + 0x01 + 0x2A = 3 bytes
    // U16 (2 bytes data): 0xC4 + 0x02 + data = 4 bytes
    // U32 (4 bytes data): 0xC4 + 0x04 + data = 6 bytes
    // U64 (8 bytes data): 0xC4 + 0x08 + data = 10 bytes

    assert_eq!(
        bytes_u8.len(),
        3,
        "U8 key should be 3 bytes (1 marker + 1 len + 1 data)"
    );
    assert_eq!(
        bytes_u16.len(),
        4,
        "U16 key should be 4 bytes (1 marker + 1 len + 2 data)"
    );
    assert_eq!(
        bytes_u32.len(),
        6,
        "U32 key should be 6 bytes (1 marker + 1 len + 4 data)"
    );
    assert_eq!(
        bytes_u64.len(),
        10,
        "U64 key should be 10 bytes (1 marker + 1 len + 8 data)"
    );

    // Test round-trip
    let recovered: InternerKey = rmp_serde::from_slice(&bytes_u8).expect("Failed to deserialize");
    assert_eq!(recovered.id(), 42, "Recovered ID should be 42");
    assert_eq!(recovered.wire_len(), 1, "Recovered should be 1 byte");

    println!("PASS: InternedKey serializes COMPACTLY!");
    println!("1-byte ID = 3 bytes (MessagePack overhead + 1 byte data)");
    println!("8-byte ID = 10 bytes (MessagePack overhead + 8 bytes data)");
}

#[test]
fn test_map_with_interned_keys_compact() {
    use crate::types::value::InnerValue;

    println!("\n=== Testing Map<InternedKey, InnerValue> compact serialization ===\n");

    // Create a map with InternedKey keys
    let mut map = crate::types::common::new_map_wc::<InternerKey, InnerValue>(3);
    let key1 = InternerKey::new(1);
    let key2 = InternerKey::new(2);
    let key3 = InternerKey::new(1000);

    map.insert(key1.clone(), InnerValue::Int(42));
    map.insert(key2.clone(), InnerValue::Int(100));
    map.insert(key3.clone(), InnerValue::Str("hello".to_string()));

    let val = InnerValue::Map(map);
    let bytes = rmp_serde::to_vec(&val).expect("Failed to serialize");

    println!(
        "Map with 3 InternedKey keys serialized: {} bytes",
        bytes.len()
    );
    println!("Bytes: {:?}", bytes);

    // Verify keys are compact in serialized data
    // Map format: marker (0x83 for fixmap with 3 entries) + [key, value] * 3
    // Each InternedKey: bin8 marker (0xC4) + length + data
    // U8 keys: 0xC4 + 0x01 + data = 3 bytes each
    // U16 key: 0xC4 + 0x02 + data = 4 bytes

    // Find 0xC4 markers (bin8) in output
    let bin8_count = bytes.iter().filter(|&&b| b == 0xC4).count();
    println!(
        "Found {} bin8 markers (0xC4) - each represents an InternedKey",
        bin8_count
    );

    assert_eq!(
        bin8_count, 3,
        "Should have 3 InternedKeys serialized with bin8 format"
    );

    // Round-trip test
    let recovered: InnerValue = rmp_serde::from_slice(&bytes).expect("Failed to deserialize");

    match recovered {
        InnerValue::Map(recovered_map) => {
            // Check we can retrieve values by InternedKey
            assert_eq!(recovered_map.len(), 3);

            let val1 = recovered_map.get(&key1);
            assert!(val1.is_some(), "Should find key1");

            let val3 = recovered_map.get(&key3);
            assert!(val3.is_some(), "Should find key3");
        }
        _ => panic!("Expected Map variant"),
    }

    println!("PASS: Map with InternedKey keys is COMPACT!");
    println!("InternedKeys stored as variable-size bytes (not full u64)!");
}

// ---------------------------------------------------------------------------
// InternerKey: all size classes + id round-trip
// ---------------------------------------------------------------------------

#[test]
fn test_interner_key_u8_boundary() {
    let key = InternerKey::new(u8::MAX as u64);
    assert_eq!(key.wire_len(), 1);
    assert_eq!(key.id(), u8::MAX as u64);
}

#[test]
fn test_interner_key_u16_boundary() {
    let key = InternerKey::new(u16::MAX as u64);
    assert_eq!(key.wire_len(), 2);
    assert_eq!(key.id(), u16::MAX as u64);
}

#[test]
fn test_interner_key_u32_boundary() {
    let key = InternerKey::new(u32::MAX as u64);
    assert_eq!(key.wire_len(), 4);
    assert_eq!(key.id(), u32::MAX as u64);
}

#[test]
fn test_interner_key_u64_max() {
    let key = InternerKey::new(u64::MAX);
    assert_eq!(key.wire_len(), 8);
    assert_eq!(key.id(), u64::MAX);
}

#[test]
fn test_interner_key_bytes_roundtrip() {
    for &id in &[1u64, 100, 300, 70_000, 5_000_000_000] {
        let key = InternerKey::new(id);
        assert_eq!(key.bytes().len(), key.wire_len());
        assert_eq!(key.id(), id);
        let consumed = key.into_bytes();
        assert_eq!(consumed.len(), InternerKey::new(id).wire_len());
    }
}

#[test]
fn test_interner_key_ordering() {
    let k1 = InternerKey::new(1);
    let k2 = InternerKey::new(2);
    let k_big = InternerKey::new(1_000_000);
    assert!(k1 < k2);
    assert!(k2 < k_big);
    assert_eq!(k1.cmp(&k1), std::cmp::Ordering::Equal);
}

#[test]
fn test_interner_key_hash_eq_different_sizes() {
    use std::collections::HashSet;
    // Keys with different byte sizes but same id should be equal and hash same
    let k1 = InternerKey::new(42);
    let k2 = InternerKey::new(42);
    assert_eq!(k1, k2);
    let mut set = HashSet::new();
    set.insert(k1);
    assert!(set.contains(&k2));
}

#[test]
fn test_interner_key_deserialization_invalid_length() {
    // 3 bytes is not a valid InternerKey length (must be 1/2/4/8)
    let bad_bytes = rmpv::Value::Binary(vec![1, 2, 3]);
    let mut buf = Vec::new();
    rmpv::encode::write_value(&mut buf, &bad_bytes).unwrap();
    let result: Result<InternerKey, _> = rmp_serde::from_slice(&buf);
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Interner: with_state edge cases + all_entries / entries_after / entries_in_id_range
// ---------------------------------------------------------------------------

#[test]
fn test_with_state_empty() {
    let interner = Interner::with_state(vec![]);
    assert!(interner.is_empty());
    assert_eq!(interner.len(), 0);
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
fn test_user_key_from_str_and_display() {
    let key = UserKey::from_str("hello");
    assert_eq!(key.as_str(), "hello");
    assert_eq!(key.to_string(), "hello");
}

#[test]
fn test_user_key_std_from_str() {
    let key: UserKey = "world".parse().unwrap();
    assert_eq!(key.as_str(), "world");
}

#[test]
fn test_user_key_as_ref() {
    let key = UserKey::from_str("test");
    let r: &str = key.as_ref();
    assert_eq!(r, "test");
}

#[test]
fn test_user_key_borrow_str() {
    use std::borrow::Borrow;
    let key = UserKey::from_str("borrowed");
    let b: &str = key.borrow();
    assert_eq!(b, "borrowed");
}

#[test]
fn test_user_key_equality() {
    let k1 = UserKey::from_str("abc");
    let k2 = UserKey::from_str("abc");
    let k3 = UserKey::from_str("xyz");
    assert_eq!(k1, k2);
    assert_ne!(k1, k3);
}

#[test]
fn test_user_key_serde_roundtrip() {
    let key = UserKey::from_str("serde_test");
    let bytes = rmp_serde::to_vec(&key).unwrap();
    let decoded: UserKey = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(key, decoded);
}

// ---------------------------------------------------------------------------
// TouchInd: AsRef<[u8]>, into_key
// ---------------------------------------------------------------------------

#[test]
fn test_touch_ind_wire_bytes() {
    let interner = Interner::new();
    let touch = interner.touch_ind("test").unwrap();
    let bytes = touch.key().to_wire_bytes();
    assert!(!bytes.is_empty());
}

#[test]
fn test_touch_ind_into_key() {
    let interner = Interner::new();
    let touch = interner.touch_ind("test").unwrap();
    let key = touch.into_key();
    assert_eq!(key.id(), 1);
}

// ---------------------------------------------------------------------------
// Interner::touch_with_id
// ---------------------------------------------------------------------------

#[test]
fn touch_with_id_fresh_insert() {
    let interner = Interner::new();
    interner.touch_with_id("email", 5).unwrap();
    assert_eq!(interner.get_ind("email"), Some(InternerKey::new(5)));
    assert_eq!(
        interner.get_str(&InternerKey::new(5)),
        Some(UserKey::from_str("email"))
    );
}

#[test]
fn touch_with_id_idempotent() {
    let interner = Interner::new();
    interner.touch_with_id("email", 5).unwrap();
    // Same call again — should be no-op.
    interner.touch_with_id("email", 5).unwrap();
    assert_eq!(interner.len(), 1);
}

#[test]
fn touch_with_id_conflict_different_id_for_known_name() {
    let interner = Interner::new();
    interner.touch_with_id("email", 5).unwrap();
    let err = interner.touch_with_id("email", 10).unwrap_err();
    assert!(err.contains("already mapped"), "unexpected error: {err}");
}

#[test]
fn touch_with_id_collision_id_used_by_different_name() {
    let interner = Interner::new();
    interner.touch_with_id("email", 5).unwrap();
    let err = interner.touch_with_id("score", 5).unwrap_err();
    assert!(err.contains("already used"), "unexpected error: {err}");
}

#[test]
fn touch_with_id_then_touch_ind_no_reuse() {
    let interner = Interner::new();
    // Pre-assign id 3 via touch_with_id.
    interner.touch_with_id("email", 3).unwrap();
    // touch_ind should allocate id > 3.
    let ti = interner.touch_ind("score").unwrap();
    assert!(
        ti.key().id() > 3,
        "touch_ind should not reuse id 3, got {}",
        ti.key().id()
    );
    // Both keys accessible.
    assert_eq!(interner.get_ind("email"), Some(InternerKey::new(3)));
    assert!(interner.get_ind("score").is_some());
}

/// Regression test for the silent data-loss bug in `entries_after`:
/// a `None` gap in the reverse vec caused the loop to `break`,
/// dropping every populated entry above the gap (never persisted →
/// missing after restart). The fix keeps scanning past gaps but
/// freezes the high-water mark so the gap is re-captured by the
/// next persist.
#[test]
fn entries_after_captures_entries_above_gap() {
    // with_state creates reverse vec with None holes for missing ids:
    //   idx 0 = None (sentinel)
    //   idx 1 = Some("a")
    //   idx 2 = None  (gap — id 2 was never interned)
    //   idx 3 = Some("c")
    let interner = Interner::with_state(vec![
        (InternerKey::new(1), UserKey::from_str("a")),
        (InternerKey::new(3), UserKey::from_str("c")),
    ]);

    let (entries, new_high) = interner.entries_after(0);

    // The entry above the gap (id 3) MUST be captured — was the
    // data-loss bug.
    assert!(
        entries.iter().any(|(k, _)| k.id() == 3),
        "entry above the gap must still be captured (was the data-loss bug)"
    );
    // The entry before the gap (id 1) must also be present.
    assert!(
        entries.iter().any(|(k, _)| k.id() == 1),
        "entry before the gap must be captured"
    );
    // The high-water mark must be frozen before the gap so it is
    // re-captured on the next persist.
    assert_eq!(new_high, 1, "high-water mark must freeze before the gap");
}
