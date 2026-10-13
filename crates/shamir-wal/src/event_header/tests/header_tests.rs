//! Format-version tests for the journal-event header (S5c-X I1).

use crate::event_header::{WalEventHeader, WalExtraChange};
use crate::wal_entry_v2::{
    WalEntryV2, WalOpV2, WAL_V2_MAGIC, WAL_V2_VERSION, WAL_V2_VERSION_LEGACY,
    WAL_V2_VERSION_PRE_EVENT,
};

fn rid(n: u8) -> shamir_types::types::record_id::RecordId {
    let mut a = [0u8; 16];
    a[15] = n;
    shamir_types::types::record_id::RecordId(a)
}

fn sample_header() -> WalEventHeader {
    WalEventHeader {
        repo: "main".to_string(),
        actor: shamir_types::access::Actor::User(42),
        tx_id: 7,
        timestamp_ns: 1_000,
        table_names: vec![(9, "users".to_string())],
        extra_changes: vec![WalExtraChange {
            table_id_interned: 9,
            change_index: 1,
            key: bytes::Bytes::from_static(b"sys:key"),
            value: Some(bytes::Bytes::from_static(b"v")),
        }],
    }
}

fn v3_entry() -> WalEntryV2 {
    WalEntryV2::new(
        11,
        7,
        vec![
            WalOpV2::Put {
                table_id_interned: 9,
                rid: rid(1),
                body: bytes::Bytes::from_static(b"body"),
            },
            WalOpV2::CounterDelta {
                table_id_interned: 9,
                delta: 1,
            },
        ],
    )
    .with_commit_version(5)
    .with_event_header(sample_header())
}

#[test]
fn v3_round_trip_preserves_header_and_ops() {
    let entry = v3_entry();
    let bytes = entry.encode().unwrap();
    assert_eq!(
        bytes[4], WAL_V2_VERSION,
        "encode writes the current version"
    );
    let decoded = WalEntryV2::decode(&bytes).unwrap();
    assert_eq!(decoded, entry);
    assert!(decoded.event.is_some());
}

#[test]
fn v2_decode_yields_no_event() {
    // Build genuine version-2 bytes: same body shape, no `event` field.
    #[derive(serde::Serialize)]
    struct V2Shape {
        txn_id: u64,
        repo_id_interned: u64,
        started_at_ns: u64,
        commit_version: u64,
        ops: Vec<WalOpV2>,
        interner_delta: Vec<(u64, String, u64)>,
    }
    let body = V2Shape {
        txn_id: 11,
        repo_id_interned: 7,
        started_at_ns: 0,
        commit_version: 5,
        ops: vec![WalOpV2::Put {
            table_id_interned: 9,
            rid: rid(1),
            body: bytes::Bytes::from_static(b"body"),
        }],
        interner_delta: vec![],
    };
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&WAL_V2_MAGIC);
    bytes.push(WAL_V2_VERSION_PRE_EVENT);
    bincode::serialize_into(&mut bytes, &body).unwrap();

    let decoded = WalEntryV2::decode(&bytes).unwrap();
    assert_eq!(decoded.txn_id, 11);
    assert_eq!(decoded.commit_version, 5);
    assert_eq!(decoded.ops.len(), 1);
    // "unknown": no journal event can be derived from a pre-header entry.
    assert!(decoded.event.is_none());
}

#[test]
fn v1_decode_yields_no_event() {
    // Version 1: no interner_delta, no event.
    #[derive(serde::Serialize)]
    struct V1Shape {
        txn_id: u64,
        repo_id_interned: u64,
        started_at_ns: u64,
        commit_version: u64,
        ops: Vec<WalOpV2>,
    }
    let body = V1Shape {
        txn_id: 3,
        repo_id_interned: 7,
        started_at_ns: 0,
        commit_version: 0,
        ops: vec![],
    };
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&WAL_V2_MAGIC);
    bytes.push(WAL_V2_VERSION_LEGACY);
    bincode::serialize_into(&mut bytes, &body).unwrap();

    let decoded = WalEntryV2::decode(&bytes).unwrap();
    assert_eq!(decoded.txn_id, 3);
    assert!(decoded.interner_delta.is_empty());
    assert!(decoded.event.is_none());
}

#[test]
fn unknown_future_version_fails_closed() {
    let mut bytes = v3_entry().encode().unwrap();
    bytes[4] = WAL_V2_VERSION + 1; // a hypothetical future format
    let err = WalEntryV2::decode(&bytes).unwrap_err();
    assert!(
        err.to_string().contains("unsupported version"),
        "unknown version must be a decode error, got: {err}"
    );
}

#[test]
fn torn_and_truncated_frames_never_decode() {
    let bytes = v3_entry().encode().unwrap();
    for cut in 0..bytes.len() {
        let res = WalEntryV2::decode(&bytes[..cut]);
        assert!(
            res.is_err(),
            "a frame truncated to {cut} bytes must not decode"
        );
    }
    // Corrupt magic: also a decode error, never a silent skip.
    let mut corrupt = bytes.clone();
    corrupt[0] = b'X';
    assert!(WalEntryV2::decode(&corrupt).is_err());
}

#[tokio::test]
async fn segment_mixing_old_and_new_entries_replays_both() {
    let dir = tempfile::TempDir::new().unwrap();
    let set = crate::segment_set::SegmentSet::open(dir.path().to_path_buf(), 1 << 20)
        .await
        .unwrap();

    // One v1-shaped, one v2-shaped, one v3 entry (append order).
    let old_v1 = {
        #[derive(serde::Serialize)]
        struct V1Shape {
            txn_id: u64,
            repo_id_interned: u64,
            started_at_ns: u64,
            commit_version: u64,
            ops: Vec<WalOpV2>,
        }
        let body = V1Shape {
            txn_id: 1,
            repo_id_interned: 7,
            started_at_ns: 0,
            commit_version: 10,
            ops: vec![WalOpV2::Delete {
                table_id_interned: 9,
                rid: rid(2),
            }],
        };
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&WAL_V2_MAGIC);
        bytes.push(WAL_V2_VERSION_LEGACY);
        bincode::serialize_into(&mut bytes, &body).unwrap();
        bytes
    };
    let old_v2 = {
        #[derive(serde::Serialize)]
        struct V2Shape {
            txn_id: u64,
            repo_id_interned: u64,
            started_at_ns: u64,
            commit_version: u64,
            ops: Vec<WalOpV2>,
            interner_delta: Vec<(u64, String, u64)>,
        }
        let body = V2Shape {
            txn_id: 2,
            repo_id_interned: 7,
            started_at_ns: 0,
            commit_version: 20,
            ops: vec![WalOpV2::Delete {
                table_id_interned: 9,
                rid: rid(3),
            }],
            interner_delta: vec![],
        };
        let mut bytes2 = Vec::new();
        bytes2.extend_from_slice(&WAL_V2_MAGIC);
        bytes2.push(WAL_V2_VERSION_PRE_EVENT);
        bincode::serialize_into(&mut bytes2, &body).unwrap();
        bytes2
    };
    let new_v3 = v3_entry().encode().unwrap();

    set.append_batch(vec![old_v1], 10).await.unwrap();
    set.append_batch(vec![old_v2], 20).await.unwrap();
    set.append_batch(vec![new_v3], 30).await.unwrap();

    let replayed = set.replay().await.unwrap();
    assert_eq!(replayed.len(), 3, "all three formats must replay");
    assert!(replayed[0].event.is_none(), "v1 entry has no event header");
    assert!(replayed[1].event.is_none(), "v2 entry has no event header");
    assert!(replayed[2].event.is_some(), "v3 entry carries the header");
    assert_eq!(replayed[2].commit_version, 5);
}
