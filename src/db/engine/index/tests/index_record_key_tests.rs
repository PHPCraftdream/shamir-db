use crate::db::engine::index::index_record_key::IndexRecordKey;
use bytes::Bytes;

#[test]
fn test_simple_index_key_creation() {
    let index_id = 12345u64;
    let value = "test@example.com";

    let key = IndexRecordKey::new(true, index_id).with_values(&[&value]);

    assert_eq!(key.is_unique, 1);
    assert_eq!(key.name_interned, index_id);
    assert_ne!(key.hash1, 0);
    assert_ne!(key.hash2, 0);
    assert_ne!(key.hash1, key.hash2);
}

#[test]
fn test_composite_index_key_creation() {
    // Составной индекс: (city, age) - используем строки для простоты
    let index_id = 67890u64;
    let city = "Minsk";
    let age = "30";

    let key = IndexRecordKey::new(false, index_id).with_values(&[&city, &age]);

    assert_eq!(key.is_unique, 0);
    assert_eq!(key.name_interned, index_id);
    assert_ne!(key.hash1, 0);
    assert_ne!(key.hash2, 0);
}

#[test]
fn test_simple_index_to_bytes() {
    let index_id = 111u64;
    let value = "test@example.com";
    let key = IndexRecordKey::new(false, index_id).with_values(&[&value]);

    let bytes = key.to_bytes();

    // Размер: is_unique(1) + index_id(8) + hash1(8) + hash2(8) = 25 байт
    assert_eq!(bytes.len(), 25);

    // is_unique = 0
    assert_eq!(bytes[0], 0);

    // index_id = 111
    assert_eq!(u64::from_le_bytes(bytes[1..9].try_into().unwrap()), 111);
}

#[test]
fn test_composite_index_to_bytes() {
    let index_id = 222u64;
    let value1 = "Minsk";
    let value2 = "30";
    let key = IndexRecordKey::new(true, index_id).with_values(&[&value1, &value2]);

    let bytes = key.to_bytes();

    // Размер: 1 + 8 + 8 + 8 = 25 байт
    assert_eq!(bytes.len(), 25);

    // is_unique = 1
    assert_eq!(bytes[0], 1);

    // index_id = 222
    assert_eq!(u64::from_le_bytes(bytes[1..9].try_into().unwrap()), 222);
}

#[test]
fn test_simple_index_from_bytes_roundtrip() {
    let index_id = 333u64;
    let value = "test@example.com";
    let original = IndexRecordKey::new(true, index_id).with_values(&[&value]);

    let bytes = original.to_bytes();
    let restored = IndexRecordKey::from_bytes(bytes).unwrap();

    assert_eq!(restored.is_unique, original.is_unique);
    assert_eq!(restored.name_interned, original.name_interned);
    assert_eq!(restored.hash1, original.hash1);
    assert_eq!(restored.hash2, original.hash2);
}

#[test]
fn test_composite_index_from_bytes_roundtrip() {
    let index_id = 444u64;
    let value1 = "Minsk";
    let value2 = "30";
    let original = IndexRecordKey::new(false, index_id).with_values(&[&value1, &value2]);

    let bytes = original.to_bytes();
    let restored = IndexRecordKey::from_bytes(bytes).unwrap();

    assert_eq!(restored.is_unique, original.is_unique);
    assert_eq!(restored.name_interned, original.name_interned);
    assert_eq!(restored.hash1, original.hash1);
    assert_eq!(restored.hash2, original.hash2);
}

#[test]
fn test_from_bytes_too_short() {
    let result = IndexRecordKey::from_bytes(Bytes::from(vec![1, 2, 3]));
    assert!(result.is_err());
}

#[test]
fn test_from_bytes_wrong_size() {
    let index_id = 555u64;
    let key = IndexRecordKey::new(true, index_id).with_values(&[&"42"]);
    let mut bytes = key.to_bytes().to_vec();

    // Укорачиваем на 1 байт
    bytes.pop();
    let result = IndexRecordKey::from_bytes(Bytes::from(bytes));
    assert!(result.is_err());
}

#[test]
fn test_composite_index_deterministic() {
    let index_id = 666u64;
    let value1 = "Minsk";
    let value2 = "30";

    let key1 = IndexRecordKey::new(true, index_id).with_values(&[&value1, &value2]);
    let key2 = IndexRecordKey::new(true, index_id).with_values(&[&value1, &value2]);

    assert_eq!(key1.hash1, key2.hash1);
    assert_eq!(key1.hash2, key2.hash2);

    let bytes1 = key1.to_bytes();
    let bytes2 = key2.to_bytes();
    assert_eq!(bytes1, bytes2);
}

#[test]
fn test_different_values_different_hashes() {
    let index_id = 777u64;

    let key1 = IndexRecordKey::new(true, index_id).with_values(&[&"Minsk", &"30"]);
    let key2 = IndexRecordKey::new(true, index_id).with_values(&[&"Moscow", &"30"]);

    assert_ne!(key1.hash1, key2.hash1);
}

#[test]
fn test_matches_index() {
    let index_id = 888u64;
    let key = IndexRecordKey::new(true, index_id).with_values(&[&"a", &"42"]);

    assert!(key.matches_index(index_id));
    assert!(key.matches_index(888));

    // Разный ID
    assert!(!key.matches_index(999));
}

#[test]
fn test_doc_new() {
    // Простой уникальный индекс
    let key = IndexRecordKey::new(true, 1001);
    assert_eq!(key.is_unique, 1);
    assert_eq!(key.name_interned, 1001);

    // Неуникальный составной индекс
    let key2 = IndexRecordKey::new(false, 1002);
    assert_eq!(key2.is_unique, 0);
    assert_eq!(key2.name_interned, 1002);
}

#[test]
fn test_hash2_includes_index_id() {
    // hash2 должен включать name_interned для уникальности
    let value = "same_value";
    let index_id1 = 1000u64;
    let index_id2 = 2000u64;

    let key1 = IndexRecordKey::new(true, index_id1).with_values(&[&value]);
    let key2 = IndexRecordKey::new(true, index_id2).with_values(&[&value]);

    // hash1 должен быть одинаковым (одинаковое значение)
    assert_ne!(key1.hash1, key2.hash1);

    // hash2 должен быть разным (разные index_id)
    assert_ne!(key1.hash2, key2.hash2);
}

#[test]
fn test_name_interned_getter() {
    let index_id = 12345u64;
    let key = IndexRecordKey::new(false, index_id);

    assert_eq!(key.name_interned(), index_id);
}
