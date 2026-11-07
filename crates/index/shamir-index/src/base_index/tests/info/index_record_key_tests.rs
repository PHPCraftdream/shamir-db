//! `IndexRecordKey` unit tests — identity v2 layout (P34-12 S2 rewrite).
//!
//! The pre-S2 tests pinned the v1 25-byte `[ns][name][hash1][hash2]` FxHasher
//! layout; that layout is gone (breaking persisted-format change with an
//! automatic forward migration). These tests pin the v2 layout:
//! `[ns][name LE][I]` with namespace 0x02 (regular) / 0x03 (unique) and the
//! identity bytes `I`.

use bytes::Bytes;

use crate::base_index::index_record_key::{
    is_legacy_base_posting_key, posting_index_prefix, IndexRecordKey,
};
use crate::model::identity::markers::{REGULAR_V2, UNIQUE_V2};

#[test]
fn test_simple_index_key_creation() {
    let index_id = 12345u64;

    let key = IndexRecordKey::new(true, index_id).with_identity(vec![0x02, 0, 0, 0, 0, 0, 0, 0, 1]);

    assert_eq!(key.namespace, UNIQUE_V2);
    assert_eq!(key.name_interned, index_id);
    assert_eq!(key.identity, vec![0x02, 0, 0, 0, 0, 0, 0, 0, 1]);
}

#[test]
fn test_composite_index_key_creation() {
    // Составной индекс: (city, age) - используем строки для простоты
    let index_id = 67890u64;

    let key = IndexRecordKey::new(false, index_id).with_identity(vec![0x01, 0x02, 0x03]);

    assert_eq!(key.namespace, REGULAR_V2);
    assert_eq!(key.name_interned, index_id);
    assert_eq!(key.identity, vec![0x01, 0x02, 0x03]);
}

#[test]
fn test_simple_index_to_bytes() {
    let index_id = 111u64;
    let identity = vec![0x06, 0, 0, 0, 3, b'a', b'b', b'c'];
    let key = IndexRecordKey::new(false, index_id).with_identity(identity.clone());

    let bytes = key.to_bytes();

    // Размер: ns(1) + index_id(8) + |I| — variable length.
    assert_eq!(bytes.len(), 9 + identity.len());

    // namespace = 0x02 (regular v2)
    assert_eq!(bytes[0], REGULAR_V2);

    // index_id = 111
    assert_eq!(u64::from_le_bytes(bytes[1..9].try_into().unwrap()), 111);

    // identity bytes follow
    assert_eq!(&bytes[9..], identity.as_slice());
}

#[test]
fn test_composite_index_to_bytes() {
    let index_id = 222u64;
    let identity = vec![0x02, 0, 0, 0, 0, 0, 0, 0, 7];
    let key = IndexRecordKey::new(true, index_id).with_identity(identity.clone());

    let bytes = key.to_bytes();

    assert_eq!(bytes.len(), 9 + identity.len());

    // namespace = 0x03 (unique v2)
    assert_eq!(bytes[0], UNIQUE_V2);

    // index_id = 222
    assert_eq!(u64::from_le_bytes(bytes[1..9].try_into().unwrap()), 222);
}

#[test]
fn test_simple_index_from_bytes_roundtrip() {
    let index_id = 333u64;
    let identity = vec![0x06, 0, 0, 0, 3, b'x', b'y', b'z'];
    let original = IndexRecordKey::new(true, index_id).with_identity(identity);

    let bytes = original.to_bytes();
    let restored = IndexRecordKey::from_bytes(bytes).unwrap();

    assert_eq!(restored.namespace, original.namespace);
    assert_eq!(restored.name_interned, original.name_interned);
    assert_eq!(restored.identity, original.identity);
}

#[test]
fn test_composite_index_from_bytes_roundtrip() {
    let index_id = 444u64;
    let identity = vec![0x02, 0, 0, 0, 0, 0, 0, 0, 1, 0x02, 0, 0, 0, 0, 0, 0, 0, 2];
    let original = IndexRecordKey::new(false, index_id).with_identity(identity);

    let bytes = original.to_bytes();
    let restored = IndexRecordKey::from_bytes(bytes).unwrap();

    assert_eq!(restored.namespace, original.namespace);
    assert_eq!(restored.name_interned, original.name_interned);
    assert_eq!(restored.identity, original.identity);
}

#[test]
fn test_from_bytes_too_short() {
    let result = IndexRecordKey::from_bytes(Bytes::from(vec![1, 2, 3]));
    assert!(result.is_err());
}

#[test]
fn test_from_bytes_wrong_size() {
    let minimal = IndexRecordKey::new(true, 555).with_identity(vec![0x00]);
    let bytes = minimal.to_bytes();
    assert_eq!(bytes.len(), 10);
    assert!(IndexRecordKey::from_bytes(Bytes::copy_from_slice(&bytes[..9])).is_err());
    assert!(IndexRecordKey::from_bytes(Bytes::from(vec![1, 2, 3])).is_err());
}

#[test]
fn test_composite_index_deterministic() {
    let index_id = 666u64;
    let identity = vec![0x06, 0, 0, 0, 2, b'M', b'9'];

    let key1 = IndexRecordKey::new(true, index_id).with_identity(identity.clone());
    let key2 = IndexRecordKey::new(true, index_id).with_identity(identity);

    assert_eq!(key1, key2);

    let bytes1 = key1.to_bytes();
    let bytes2 = key2.to_bytes();
    assert_eq!(bytes1, bytes2);
}

#[test]
fn test_different_values_different_identities() {
    let index_id = 777u64;

    let key1 = IndexRecordKey::new(true, index_id)
        .with_identity(vec![0x06, 0, 0, 0, 5, b'M', b'i', b'n', b's', b'k']);
    let key2 = IndexRecordKey::new(true, index_id)
        .with_identity(vec![0x06, 0, 0, 0, 6, b'M', b'o', b's', b'c', b'o', b'w']);

    assert_ne!(key1.identity, key2.identity);
    assert_ne!(key1.to_bytes(), key2.to_bytes());
}

#[test]
fn test_matches_index() {
    let index_id = 888u64;
    let key = IndexRecordKey::new(true, index_id).with_identity(vec![0x00]);

    assert!(key.matches_index(index_id));
    assert!(key.matches_index(888));

    // Разный ID
    assert!(!key.matches_index(999));
}

#[test]
fn test_doc_new() {
    // Простой уникальный индекс
    let key = IndexRecordKey::new(true, 1001);
    assert_eq!(key.namespace, UNIQUE_V2);
    assert_eq!(key.name_interned, 1001);

    // Неуникальный составной индекс
    let key2 = IndexRecordKey::new(false, 1002);
    assert_eq!(key2.namespace, REGULAR_V2);
    assert_eq!(key2.name_interned, 1002);
}

#[test]
fn test_identity_carries_index_id() {
    // Two indexes with the SAME identity value produce DIFFERENT keys:
    // the name_interned is part of the key prefix.
    let identity = vec![0x06, 0, 0, 0, 4, b's', b'a', b'm', b'e'];
    let key1 = IndexRecordKey::new(true, 1000).with_identity(identity.clone());
    let key2 = IndexRecordKey::new(true, 2000).with_identity(identity);

    assert_ne!(key1.to_bytes(), key2.to_bytes());
}

#[test]
fn test_name_interned_getter() {
    let index_id = 12345u64;
    let key = IndexRecordKey::new(false, index_id);

    assert_eq!(key.name_interned(), index_id);
}

#[test]
fn test_posting_index_prefix_regular_and_unique() {
    let identity = vec![0x02, 0, 0, 0, 0, 0, 0, 0, 1];
    let index_key = IndexRecordKey::new(false, 42)
        .with_identity(identity.clone())
        .to_bytes();

    let mut posting = index_key.to_vec();
    posting.extend_from_slice(&[0xAA; 16]); // rid tail
    let prefix = posting_index_prefix(&posting).expect("regular posting prefix");
    assert_eq!(prefix, index_key.as_ref());

    let unique_key = IndexRecordKey::new(true, 43)
        .with_identity(identity)
        .to_bytes();
    let prefix = posting_index_prefix(unique_key.as_ref()).expect("unique key prefix");
    assert_eq!(prefix, unique_key.as_ref());

    // Too short / foreign namespace: None.
    assert!(posting_index_prefix(&posting[..10]).is_none());
    let mut legacy = vec![0x00u8];
    legacy.extend_from_slice(&[0u8; 24]);
    assert!(posting_index_prefix(&legacy).is_none());
}

#[test]
fn test_legacy_posting_key_detection() {
    let mut legacy = vec![0x01u8];
    legacy.extend_from_slice(&[0u8; 24]);
    assert!(is_legacy_base_posting_key(&legacy));
    assert!(!is_legacy_base_posting_key(&legacy[..10]));
    let v2 = IndexRecordKey::new(false, 1)
        .with_identity(vec![0x00])
        .to_bytes();
    assert!(!is_legacy_base_posting_key(v2.as_ref()));
}

#[test]
fn test_legacy_v1_compatibility_helper_writes_legacy_namespace() {
    let key = IndexRecordKey::legacy_v1(true, 42).with_identity(vec![0xAA; 16]);
    let bytes = key.to_bytes();
    assert_eq!(bytes[0], 0x01);
    assert_eq!(bytes.len(), 25);
}
