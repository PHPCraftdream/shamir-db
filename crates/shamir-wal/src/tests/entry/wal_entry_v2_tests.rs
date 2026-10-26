use bincode::Options;
use bytes::Bytes;
use shamir_types::types::record_id::RecordId;

use crate::wal_entry_v2::{
    WalEntryV2, WalOpV2, WAL_V2_MAGIC, WAL_V2_VERSION, WAL_V2_VERSION_LEGACY,
};

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
        interner_delta: vec![],
        event: None,
    }
}

#[test]
fn frame_header_validation_is_cheap_and_version_aware() {
    assert!(WalEntryV2::validate_frame_header(b"").is_err());
    assert!(WalEntryV2::validate_frame_header(b"WAL2").is_err());
    assert!(WalEntryV2::validate_frame_header(b"NOPE\x01").is_err());
    assert!(WalEntryV2::validate_frame_header(b"WAL2\xff").is_err());
    for version in [WAL_V2_VERSION_LEGACY, 2, WAL_V2_VERSION] {
        let bytes = [WAL_V2_MAGIC.as_slice(), &[version], b"malformed body"].concat();
        assert!(WalEntryV2::validate_frame_header(&bytes).is_ok());
        assert!(WalEntryV2::decode(&bytes).is_err());
    }
}

#[test]
fn rv_wal_9cb60c5a_rejects_truly_ambiguous_legacy_shapes() {
    // At these overlapping offsets, the pre-HIGH-5 layout sees Delete then
    // IndexDel, while the post-HIGH-5 layout sees Put; both consume all bytes.
    let mut body = Vec::new();
    body.extend_from_slice(&1u64.to_le_bytes()); // txn id
    body.extend_from_slice(&2u64.to_le_bytes()); // repo id
    body.extend_from_slice(&3u64.to_le_bytes()); // started_at_ns
    body.extend_from_slice(&2u64.to_le_bytes()); // ops count / post commit version
    body.extend_from_slice(&1u32.to_le_bytes()); // Delete tag
    body.extend_from_slice(&0u64.to_le_bytes()); // table id / post ops count
    body.extend_from_slice(&16u64.to_le_bytes()); // rid length / post Put tag + table
    body.extend_from_slice(&16u64.to_le_bytes()); // first eight rid bytes / post rid length
    body.extend_from_slice(&0u64.to_le_bytes()); // remaining rid bytes
    body.extend_from_slice(&3u32.to_le_bytes()); // IndexDel tag
    body.extend_from_slice(&(16u64 << 32).to_le_bytes()); // table id
    body.extend_from_slice(&0u32.to_le_bytes()); // idx id
    body.extend_from_slice(&8u64.to_le_bytes()); // key length / post body length
    body.extend_from_slice(&0u64.to_le_bytes()); // key bytes / post body bytes
    assert_eq!(body.len(), 100);

    let options = || {
        bincode::DefaultOptions::new()
            .with_fixint_encoding()
            .reject_trailing_bytes()
    };
    let pre: Result<(u64, u64, u64, Vec<WalOpV2>), _> = options().deserialize(&body);
    let post: Result<(u64, u64, u64, u64, Vec<WalOpV2>), _> = options().deserialize(&body);
    assert_eq!(pre.unwrap().3.len(), 2);
    let decoded_post = post.unwrap();
    assert_eq!(decoded_post.3, 2);
    assert_eq!(decoded_post.4.len(), 1);

    let mut frame = WAL_V2_MAGIC.to_vec();
    frame.push(WAL_V2_VERSION_LEGACY);
    frame.extend_from_slice(&body);
    let error = WalEntryV2::decode(&frame).unwrap_err().to_string();
    assert!(error.contains("ambiguous legacy shape"), "{error}");
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

// ---------------------------------------------------------------------------
// WAL version 2 (with interner_delta) tests
// ---------------------------------------------------------------------------

#[test]
fn round_trip_v2_with_interner_delta() {
    let entry = WalEntryV2 {
        txn_id: 10,
        repo_id_interned: 3,
        started_at_ns: 999,
        commit_version: 7,
        ops: vec![WalOpV2::Put {
            table_id_interned: 3,
            rid: rid(1),
            body: Bytes::from_static(b"x"),
        }],
        interner_delta: vec![(3, "email".into(), 100), (3, "score".into(), 101)],
        event: None,
    };
    let encoded = entry.encode().unwrap();
    assert_eq!(encoded[4], WAL_V2_VERSION); // current version
    let decoded = WalEntryV2::decode(&encoded).unwrap();
    assert_eq!(entry, decoded);
    assert_eq!(decoded.interner_delta.len(), 2);
}

#[test]
fn round_trip_v2_empty_delta() {
    let entry = sample_entry(); // interner_delta defaults to vec![]
    assert!(entry.interner_delta.is_empty());
    let encoded = entry.encode().unwrap();
    let decoded = WalEntryV2::decode(&encoded).unwrap();
    assert_eq!(entry, decoded);
    assert!(decoded.interner_delta.is_empty());
}

#[test]
fn rv_wal_9cb60c5a_decodes_pre_high5_v1_shape_with_zero_version() {
    let mut bytes = WAL_V2_MAGIC.to_vec();
    bytes.push(WAL_V2_VERSION_LEGACY);
    bytes.extend_from_slice(&9u64.to_le_bytes());
    bytes.extend_from_slice(&3u64.to_le_bytes());
    bytes.extend_from_slice(&17u64.to_le_bytes());
    bytes.extend_from_slice(&1u64.to_le_bytes()); // one op
    bytes.extend_from_slice(&1u32.to_le_bytes()); // Delete variant
    bytes.extend_from_slice(&3u64.to_le_bytes()); // table id
    bytes.extend_from_slice(&16u64.to_le_bytes());
    bytes.extend_from_slice(&rid(1).0);
    let decoded = WalEntryV2::decode(&bytes).unwrap();
    assert_eq!(decoded.txn_id, 9);
    assert_eq!(decoded.repo_id_interned, 3);
    assert_eq!(decoded.started_at_ns, 17);
    assert_eq!(decoded.commit_version, 0);
    assert_eq!(
        decoded.ops,
        vec![WalOpV2::Delete {
            table_id_interned: 3,
            rid: rid(1)
        }]
    );
    assert!(decoded.interner_delta.is_empty());
    assert_eq!(decoded.event, None);
}

#[test]
fn rv_wal_9cb60c5a_pre_high5_v1_empty_and_exact_consumption() {
    let mut bytes = WAL_V2_MAGIC.to_vec();
    bytes.push(WAL_V2_VERSION_LEGACY);
    bytes.extend_from_slice(&9u64.to_le_bytes());
    bytes.extend_from_slice(&3u64.to_le_bytes());
    bytes.extend_from_slice(&17u64.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    let decoded = WalEntryV2::decode(&bytes).unwrap();
    assert_eq!(decoded.txn_id, 9);
    assert_eq!(decoded.repo_id_interned, 3);
    assert_eq!(decoded.started_at_ns, 17);
    assert_eq!(decoded.commit_version, 0);
    assert!(decoded.ops.is_empty());
    assert!(decoded.interner_delta.is_empty());
    assert_eq!(decoded.event, None);
    assert!(WalEntryV2::decode(&bytes[..bytes.len() - 1]).is_err());
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(WalEntryV2::decode(&trailing).is_err());
}

#[test]
fn decode_legacy_v1_produces_empty_delta() {
    // Build a v1-shaped entry by encoding with the legacy version byte.
    // We serialize the legacy struct (without interner_delta) via bincode
    // and prepend the v1 header.
    #[derive(serde::Serialize)]
    struct LegacyEntry {
        txn_id: u64,
        repo_id_interned: u64,
        started_at_ns: u64,
        commit_version: u64,
        ops: Vec<WalOpV2>,
    }
    let legacy = LegacyEntry {
        txn_id: 42,
        repo_id_interned: 7,
        started_at_ns: 1_234_567_890,
        commit_version: 123,
        ops: vec![WalOpV2::Put {
            table_id_interned: 7,
            rid: rid(1),
            body: Bytes::from_static(b"hello"),
        }],
    };
    let mut bytes = Vec::with_capacity(256);
    bytes.extend_from_slice(&WAL_V2_MAGIC);
    bytes.push(WAL_V2_VERSION_LEGACY);
    bincode::serialize_into(&mut bytes, &legacy).unwrap();

    let decoded = WalEntryV2::decode(&bytes).unwrap();
    assert_eq!(decoded.txn_id, 42);
    assert_eq!(decoded.repo_id_interned, 7);
    assert_eq!(decoded.commit_version, 123);
    assert!(decoded.interner_delta.is_empty());
    assert_eq!(decoded.ops.len(), 1);
}

// Red before the fix (RV-WAL-645DBB85): short input, bad magic, unsupported
// versions and malformed bodies were classified as DbError::Internal — a
// programmer-error variant — instead of a persisted-data decode failure.
#[test]
fn rv_wal_645dbb85_corrupt_decode_not_internal() {
    let mut unsupported = WAL_V2_MAGIC.to_vec();
    unsupported.push(9);
    let mut malformed_body = WAL_V2_MAGIC.to_vec();
    malformed_body.push(WAL_V2_VERSION);
    malformed_body.extend_from_slice(&[0xFF, 0xFF, 0xFF]);
    let cases: Vec<Vec<u8>> = vec![Vec::new(), b"NOPE!".to_vec(), unsupported, malformed_body];
    for bytes in &cases {
        let err = WalEntryV2::decode(bytes).unwrap_err();
        assert!(
            !matches!(err, shamir_storage::error::DbError::Internal(_)),
            "corruption misclassified as Internal: {err}"
        );
        assert!(
            matches!(err, shamir_storage::error::DbError::Codec(_)),
            "expected Codec classification, got: {err}"
        );
    }
}
