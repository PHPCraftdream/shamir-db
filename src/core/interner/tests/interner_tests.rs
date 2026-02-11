use crate::core::interner::{InternedKey, Interner, TouchInd, UserKey};
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
        interner.get_str(&InternedKey::new(1, 1)),
        Some(UserKey::from_str("hello"))
    );
    assert_eq!(
        interner.get_str(&InternedKey::new(2, 1)),
        Some(UserKey::from_str("world"))
    );
    assert_eq!(interner.get_ind("world"), Some(InternedKey::new(2, 1)));
}

#[test]
fn test_with_state_initialization() {
    let initial_data = vec![
        (InternedKey::new(1, 1), UserKey::from_str("name")),
        (InternedKey::new(50, 1), UserKey::from_str("age")),
        (InternedKey::new(100, 1), UserKey::from_str("city")),
    ];
    let interner = Interner::with_state(initial_data);

    // Check that initial data is loaded correctly
    assert_eq!(interner.get_ind("name"), Some(InternedKey::new(1, 1)));
    assert_eq!(
        interner.get_str(&InternedKey::new(50, 1)),
        Some(UserKey::from_str("age"))
    );
    assert_eq!(interner.get_ind("city"), Some(InternedKey::new(100, 1)));

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
fn test_key_size_growth() {
    let interner = Interner::new();

    // Start with 1-byte keys
    assert_eq!(interner.key_size(), 1);

    // Add 255 keys (max for u8)
    for i in 0..255 {
        interner.touch_ind(format!("key_{}", i)).unwrap();
    }

    assert_eq!(interner.key_size(), 1);

    // Add 256th key - should migrate to 2-byte keys
    interner.touch_ind("key_255").unwrap();
    assert_eq!(interner.key_size(), 2);

    // Verify we can still access old keys
    assert!(interner.get_ind("key_0").is_some());
    assert!(interner.get_ind("key_255").is_some());

    // Add more keys to trigger 4-byte migration
    for i in 256..65536 {
        interner.touch_ind(format!("key_{}", i)).unwrap();
    }
    assert_eq!(interner.key_size(), 4);
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
    for i in 1..results.len() {
        let id_map_1: std::collections::HashMap<&str, u64> = first_result
            .iter()
            .zip(keys.iter())
            .map(|(result, key)| (*key, result.key().id()))
            .collect();
        let id_map_2: std::collections::HashMap<&str, u64> = results[i]
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
                let _ = interner_clone.get_str(&InternedKey::new(1, 1));
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
fn test_concurrent_same_key_determinism() {
    let interner = Arc::new(Interner::new());
    let num_threads = 100;
    let mut handles = vec![];

    // All threads touch same keys
    for _ in 0..num_threads {
        let interner_clone = Arc::clone(&interner);
        handles.push(thread::spawn(move || {
            let mut ids = vec![];
            for key in &["shared1", "shared2", "shared3", "shared1", "shared2"] {
                ids.push(interner_clone.touch_ind(key).unwrap().key().id());
            }
            ids
        }));
    }

    let results: Vec<Vec<u64>> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    // All threads should get same IDs for same keys
    let expected = vec![1, 2, 3, 1, 2];
    for result in results {
        assert_eq!(result, expected);
    }

    // Verify final state
    assert_eq!(interner.len(), 3);
}

#[test]
fn test_concurrent_reverse_lookup() {
    let interner = Arc::new(Interner::new());
    let num_threads = 20;
    let mut handles = vec![];

    // Populate first and collect actual IDs
    let mut key_to_id: Vec<(String, InternedKey)> = vec![];
    for i in 0..100 {
        let key = format!("key_{}", i);
        let touch_result = interner.touch_ind(key.clone()).unwrap();
        key_to_id.push((key, touch_result.key().clone()));
    }

    // Create a mapping for easy lookup
    let id_lookup: std::collections::HashMap<InternedKey, String> = key_to_id
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
                assert!(key.is_some(), "Failed to look up ID: {:?}", id.as_bytes());
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
    assert_eq!(interner.get_ind(""), Some(InternedKey::new(1, 1)));
    assert_eq!(
        interner.get_str(&InternedKey::new(1, 1)),
        Some(UserKey::from_str(""))
    );

    // Unicode strings
    let unicode_keys = vec!["привет", "🚀🎉🔥", "مرحبا", "مرحبا2", "😀😃😄😁"];

    for key in &unicode_keys {
        interner.touch_ind(key).unwrap();
    }

    // Verify unicode keys work
    assert_eq!(interner.get_ind("привет"), Some(InternedKey::new(2, 1)));
    assert_eq!(interner.get_ind("🚀🎉🔥"), Some(InternedKey::new(3, 1)));
    assert_eq!(interner.get_ind("مرحبا"), Some(InternedKey::new(4, 1)));
    assert_eq!(
        interner.get_str(&InternedKey::new(5, 1)),
        Some(UserKey::from_str("مرحبا2"))
    );
    assert_eq!(interner.get_ind("😀😃😄😁"), Some(InternedKey::new(6, 1)));
}

#[test]
fn test_edge_cases_very_long_keys() {
    let interner = Interner::new();

    // Very long key (10KB)
    let long_key = "a".repeat(10_000);
    let id = interner.touch_ind(&long_key).unwrap();
    assert_eq!(id.key().id(), 1);
    assert_eq!(interner.get_ind(&long_key), Some(InternedKey::new(1, 1)));
    assert_eq!(
        interner.get_str(&InternedKey::new(1, 1)),
        Some(UserKey::from_str(long_key.clone()))
    );
}

#[test]
fn test_concurrent_with_state() {
    let initial_data: Vec<(InternedKey, UserKey)> = (0..100)
        .map(|i| {
            (
                InternedKey::new(i + 1, 1),
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
    // At 1100 keys, we should have migrated to 2-byte keys
    assert_eq!(interner.key_size(), 2);

    // Verify initial data still accessible (now with 2-byte keys)
    assert_eq!(interner.get_ind("initial_0"), Some(InternedKey::new(1, 2)));
    assert_eq!(
        interner.get_ind("initial_99"),
        Some(InternedKey::new(100, 2))
    );
    assert_eq!(
        interner.get_str(&InternedKey::new(1, 2)),
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
    let key1 = InternedKey::new(42, 1);
    let bytes1 = rmp_serde::to_vec(&key1).unwrap();
    let decoded1: InternedKey = rmp_serde::from_slice(&bytes1).unwrap();
    assert_eq!(key1.id(), decoded1.id());
    assert_eq!(key1.as_bytes(), decoded1.as_bytes());

    let key2 = InternedKey::new(1000, 2);
    let bytes2 = rmp_serde::to_vec(&key2).unwrap();
    let decoded2: InternedKey = rmp_serde::from_slice(&bytes2).unwrap();
    assert_eq!(key2.id(), decoded2.id());
    assert_eq!(key2.as_bytes(), decoded2.as_bytes());

    let key3 = InternedKey::new(100000, 4);
    let bytes3 = rmp_serde::to_vec(&key3).unwrap();
    let decoded3: InternedKey = rmp_serde::from_slice(&bytes3).unwrap();
    assert_eq!(key3.id(), decoded3.id());
    assert_eq!(key3.as_bytes(), decoded3.as_bytes());
}

#[test]
fn test_interned_key_compact_messagepack_serialization() {
    // Test that InternedKey serializes compactly in MessagePack (not as full u64)
    println!("=== Testing InternedKey compact MessagePack serialization ===\n");

    // Create keys with different sizes
    let key_u8 = InternedKey::new(42, 1);
    let key_u16 = InternedKey::new(1000, 2);
    let key_u32 = InternedKey::new(70000, 4);
    let key_u64 = InternedKey::new(5000000000, 8);

    println!("Raw key sizes:");
    println!("  U8: {} bytes", key_u8.as_bytes().len());
    println!("  U16: {} bytes", key_u16.as_bytes().len());
    println!("  U32: {} bytes", key_u32.as_bytes().len());
    println!("  U64: {} bytes", key_u64.as_bytes().len());
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
    let recovered: InternedKey = rmp_serde::from_slice(&bytes_u8).expect("Failed to deserialize");
    assert_eq!(recovered.id(), 42, "Recovered ID should be 42");
    assert_eq!(recovered.as_bytes().len(), 1, "Recovered should be 1 byte");

    println!("✓ PASS: InternedKey serializes COMPACTLY!");
    println!("✓ 1-byte ID = 3 bytes (MessagePack overhead + 1 byte data)");
    println!("✓ 8-byte ID = 10 bytes (MessagePack overhead + 8 bytes data)");
}

#[test]
fn test_map_with_interned_keys_compact() {
    use crate::types::value::InnerValue;

    println!("\n=== Testing Map<InternedKey, InnerValue> compact serialization ===\n");

    // Create a map with InternedKey keys
    let mut map = crate::types::common::new_map_wc::<InternedKey, InnerValue>(3);
    let key1 = InternedKey::new(1, 1); // 1 byte
    let key2 = InternedKey::new(2, 1); // 1 byte
    let key3 = InternedKey::new(1000, 2); // 2 bytes

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

    println!("✓ PASS: Map with InternedKey keys is COMPACT!");
    println!("✓ InternedKeys stored as variable-size bytes (not full u64)!");
}
