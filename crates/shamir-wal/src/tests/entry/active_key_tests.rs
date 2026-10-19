use crate::active_key::WalActiveKey;

#[test]
fn round_trip() {
    for txn_id in [0u64, 1, 42, u64::MAX / 2, u64::MAX] {
        let k = WalActiveKey::new(txn_id);
        let bytes = k.to_bytes();
        assert_eq!(WalActiveKey::parse(&bytes), Some(txn_id));
    }
}

#[test]
fn binary_layout_matches_legacy() {
    // Byte-for-byte compatibility with the previous inline
    // `ACTIVE_PREFIX || txn_id_be` encoding. Persisted WAL data
    // on disk before this refactor MUST still parse correctly.
    let k = WalActiveKey::new(0x1234_5678_9abc_def0).to_bytes();
    let expected: &[u8] = &[
        b'_', b'_', b'w', b'a', b'l', b'_', b'a', b'c', b't', b'i', b'v', b'e', b'_', 0x12, 0x34,
        0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0,
    ];
    assert_eq!(k.as_ref(), expected, "binary layout must not change");
    assert_eq!(k.len(), 21);
}

#[test]
fn parse_rejects_wrong_length() {
    assert_eq!(WalActiveKey::parse(b""), None);
    assert_eq!(WalActiveKey::parse(b"__wal_active_"), None);
    assert_eq!(WalActiveKey::parse(&[0u8; 22]), None);
}

#[test]
fn parse_rejects_wrong_prefix() {
    let bad: [u8; 21] = [
        b'_', b'_', b'd', b'a', b'l', b'_', b'a', b'c', b't', b'i', b'v', b'e', b'_', 0, 0, 0, 0,
        0, 0, 0, 0,
    ];
    assert_eq!(WalActiveKey::parse(&bad), None);
}

#[test]
fn scan_prefix_matches_legacy() {
    let prefix = WalActiveKey::scan_prefix();
    assert_eq!(prefix.as_ref(), b"__wal_active_");
}
