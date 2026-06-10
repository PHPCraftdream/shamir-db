use bytes::Bytes;
use shamir_types::types::record_id::RecordId;

use crate::wal_entry_v2::{WalEntryV2, WalOpV2, WAL_V2_MAGIC, WAL_V2_VERSION};

fn rid(n: u8) -> RecordId {
    let mut a = [0u8; 16];
    a[15] = n;
    RecordId(a)
}

fn sample_entry() -> WalEntryV2 {
    WalEntryV2 {
        txn_id: 42,
        repo_id_interned: 7,
        started_at_ns: 1_234_567_890,
        commit_version: 123,
        ops: vec![
            WalOpV2::Put {
                table_id_interned: 7,
                rid: rid(1),
                body: Bytes::from_static(b"hello"),
            },
            WalOpV2::Delete {
                table_id_interned: 7,
                rid: rid(2),
            },
            WalOpV2::IndexPut {
                table_id_interned: 7,
                idx_id: 11,
                key: Bytes::from_static(b"k"),
                value: Bytes::from_static(b"v"),
            },
            WalOpV2::IndexDel {
                table_id_interned: 7,
                idx_id: 11,
                key: Bytes::from_static(b"k2"),
            },
            WalOpV2::InternerOverlayMerge {
                entries: vec![(100, "email".into()), (101, "score".into())],
            },
            WalOpV2::CounterDelta {
                table_id_interned: 5,
                delta: -3,
            },
        ],
    }
}

#[test]
fn round_trip_all_op_variants() {
    let entry = sample_entry();
    let encoded = entry.encode().unwrap();
    let decoded = WalEntryV2::decode(&encoded).unwrap();
    assert_eq!(entry, decoded);
}

#[test]
fn encode_has_magic_and_version() {
    let bytes = sample_entry().encode().unwrap();
    assert_eq!(&bytes[..4], &WAL_V2_MAGIC);
    assert_eq!(bytes[4], WAL_V2_VERSION);
}

#[test]
fn decode_rejects_short_input() {
    assert!(WalEntryV2::decode(b"").is_err());
    assert!(WalEntryV2::decode(b"WAL").is_err());
    assert!(WalEntryV2::decode(b"WAL2").is_err());
}

#[test]
fn decode_rejects_bad_magic() {
    let mut bytes = sample_entry().encode().unwrap();
    bytes[0] = b'X';
    assert!(WalEntryV2::decode(&bytes).is_err());
}

#[test]
fn decode_rejects_unknown_version() {
    let mut bytes = sample_entry().encode().unwrap();
    bytes[4] = 99;
    assert!(WalEntryV2::decode(&bytes).is_err());
}

#[test]
fn looks_like_v2_sniff() {
    let bytes = sample_entry().encode().unwrap();
    assert!(WalEntryV2::looks_like_v2(&bytes));
    assert!(!WalEntryV2::looks_like_v2(b""));
    assert!(!WalEntryV2::looks_like_v2(b"SDB2\x01")); // wrong magic
    let v1_bytes = bincode::serialize(&"some v1 entry").unwrap();
    // V1 entries don't carry magic prefix — start with bincode varints.
    // Very unlikely to start with "WAL2".
    assert!(!WalEntryV2::looks_like_v2(&v1_bytes));
}

#[test]
fn size_bound_on_large_batch() {
    // 100 small Put ops, each 50 bytes body — roughly 5KB raw.
    // Bincode adds per-field overhead (variant tag + length prefix).
    // Acceptance: encoded fits in 10KB.
    let ops: Vec<_> = (0..100u8)
        .map(|i| WalOpV2::Put {
            table_id_interned: 0,
            rid: rid(i),
            body: Bytes::from(vec![b'x'; 50]),
        })
        .collect();
    let entry = WalEntryV2::new(1, 0, ops);
    let encoded = entry.encode().unwrap();
    assert!(
        encoded.len() < 10240,
        "encoded size {} should be < 10KB",
        encoded.len()
    );
}
