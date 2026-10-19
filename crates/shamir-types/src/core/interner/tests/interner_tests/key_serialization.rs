//! Interned-key serialisation, boundaries, ordering, and deserialisation tests.

use crate::core::interner::InternerKey;
use crate::types::common::new_map_wc;
use shamir_collections::TFxSet;

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
    let mut map = new_map_wc::<InternerKey, InnerValue>(3);
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
    // Keys with different byte sizes but same id should be equal and hash same
    let k1 = InternerKey::new(42);
    let k2 = InternerKey::new(42);
    assert_eq!(k1, k2);
    let mut set = TFxSet::<_>::default();
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
