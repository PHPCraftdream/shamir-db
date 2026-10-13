//! Derivation tests: entry -> journal event (S5c-X I1).

use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::KvOp;
use shamir_storage::types::RecordKey;
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;
use shamir_wal::{WalEntryV2, WalEventHeader, WalOpV2};
use std::sync::Arc;

use crate::changefeed::{ChangeOp, RecordChange};
use crate::staging_store::StagingStore;
use crate::tx_context::{TxContext, TxId};
use crate::wal_event::{event_from_wal_entry, event_header_from_tx};

fn rid(n: u8) -> RecordId {
    let mut a = [0u8; 16];
    a[15] = n;
    RecordId(a)
}

fn staging(ops: Vec<KvOp>) -> StagingStore {
    let mut s = StagingStore::new(Arc::new(InMemoryStore::new()));
    for op in ops {
        match op {
            KvOp::Set(k, v) => s.set(k, v),
            KvOp::Remove(k) => s.remove(k),
        }
    }
    s
}

fn tx_with(table_token: u64, ops: Vec<KvOp>) -> TxContext {
    let mut tx = TxContext::new(TxId::new(77), 0, 0, crate::types::IsolationLevel::Snapshot);
    tx.actor = Actor::User(9);
    tx.table_tokens.insert(table_token, "users".to_string());
    tx.write_set.insert(table_token, staging(ops));
    tx
}

fn entry_from(tx: &TxContext, version: u64) -> WalEntryV2 {
    // Data ops exactly as the live `wal_ops_from_tx` emits them for a
    // write-set-only tx: RecordId-keyed Set/Remove become Put/Delete.
    let mut ops: Vec<WalOpV2> = Vec::new();
    for (token, st) in &tx.write_set {
        for kv in st.iter_ops() {
            match kv {
                KvOp::Set(k, v) => {
                    if let Some(r) = RecordId::try_from_bytes(&k) {
                        ops.push(WalOpV2::Put {
                            table_id_interned: *token,
                            rid: r,
                            body: v,
                        });
                    }
                }
                KvOp::Remove(k) => {
                    if let Some(r) = RecordId::try_from_bytes(&k) {
                        ops.push(WalOpV2::Delete {
                            table_id_interned: *token,
                            rid: r,
                        });
                    }
                }
            }
        }
    }
    WalEntryV2::new(tx.tx_id.0, 0, ops)
        .with_commit_version(version)
        .with_event_header(event_header_from_tx(tx, "main"))
}

fn kv_set(key: Vec<u8>, val: &[u8]) -> KvOp {
    KvOp::Set(RecordKey::from_slice(&key), Bytes::from_static(val))
}

#[test]
fn derived_event_matches_projection_shape_and_order() {
    let tx = tx_with(
        9,
        vec![
            kv_set(rid(1).as_bytes().to_vec(), b"v1"),
            kv_set(b"sys:key".to_vec(), b"sys"), // non-RecordId, spliced at index 1
            kv_set(rid(2).as_bytes().to_vec(), b"v2"),
        ],
    );
    let entry = entry_from(&tx, 5);
    let event = event_from_wal_entry(&entry).expect("data ops must yield an event");

    assert_eq!(event.repo, "main");
    assert_eq!(event.commit_version, 5);
    assert_eq!(event.tx_id, 77, "non-implicit tx keeps its public id");
    assert_eq!(event.actor, Actor::User(9));
    // Byte-for-byte projected order: rid, non-rid, rid.
    let keys: Vec<Vec<u8>> = event.changes.iter().map(|c| c.key.to_vec()).collect();
    assert_eq!(
        keys,
        vec![
            rid(1).as_bytes().to_vec(),
            b"sys:key".to_vec(),
            rid(2).as_bytes().to_vec()
        ]
    );
    assert_eq!(event.changes[0].op, ChangeOp::Put);
    assert_eq!(event.changes[0].value.as_deref(), Some(b"v1".as_slice()));
    assert_eq!(event.changes[1].table, "users");
    assert_eq!(event.changes[2].op, ChangeOp::Put);
}

#[test]
fn delete_yields_value_none() {
    let tx = tx_with(
        9,
        vec![KvOp::Remove(RecordKey::from_slice(rid(3).as_bytes()))],
    );
    let event = event_from_wal_entry(&entry_from(&tx, 1)).unwrap();
    assert_eq!(event.changes.len(), 1);
    assert_eq!(event.changes[0].op, ChangeOp::Delete);
    assert_eq!(event.changes[0].value, None);
}

#[test]
fn encode_decode_round_trip_preserves_the_event() {
    let tx = tx_with(
        9,
        vec![
            kv_set(rid(4).as_bytes().to_vec(), b"v"),
            kv_set(b"sys".to_vec(), b"s"),
        ],
    );
    let entry = entry_from(&tx, 8);
    let bytes = entry.encode().unwrap();
    let decoded = WalEntryV2::decode(&bytes).unwrap();
    assert_eq!(
        event_from_wal_entry(&decoded),
        event_from_wal_entry(&entry),
        "the event is a pure function of the encoded entry"
    );
}

#[test]
fn eventless_footprints_yield_no_event() {
    // Index-only / counter-only: header present, but no data ops.
    let mut entry = WalEntryV2::new(
        5,
        0,
        vec![WalOpV2::CounterDelta {
            table_id_interned: 9,
            delta: 1,
        }],
    )
    .with_commit_version(2);
    entry.event = Some(event_header_from_tx(&tx_with(9, vec![]), "main"));
    assert!(
        event_from_wal_entry(&entry).is_none(),
        "index-only commit is eventless"
    );

    // Empty ops, header present.
    let mut empty = WalEntryV2::new(6, 0, vec![]).with_commit_version(3);
    empty.event = Some(event_header_from_tx(&tx_with(9, vec![]), "main"));
    assert!(event_from_wal_entry(&empty).is_none());
}

/// NEGATIVE CONTROL (S5c-X I1): deriving from an entry WITHOUT the header
/// cannot reproduce the event. The unprotected path is the pre-S5c entry
/// format itself (version 2 wire bytes: no `event` field) - exercised
/// through encode/decode, not by editing code.
#[test]
fn entry_without_header_cannot_reproduce_the_event() {
    let tx = tx_with(9, vec![kv_set(rid(5).as_bytes().to_vec(), b"v")]);
    let protected = event_from_wal_entry(&entry_from(&tx, 4));
    assert!(
        protected.is_some(),
        "with the header the event is derivable"
    );

    // The same tx, written as a v2-format entry (no header field).
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
        txn_id: 77,
        repo_id_interned: 0,
        started_at_ns: 0,
        commit_version: 4,
        ops: vec![WalOpV2::Put {
            table_id_interned: 9,
            rid: rid(5),
            body: Bytes::from_static(b"v"),
        }],
        interner_delta: vec![],
    };
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&shamir_wal::WAL_V2_MAGIC);
    bytes.push(2); // pre-event format version
    bincode::serialize_into(&mut bytes, &body).unwrap();
    let headerless = WalEntryV2::decode(&bytes).unwrap();

    assert!(
        event_from_wal_entry(&headerless).is_none(),
        "without the header the SAME committed changes yield NO event - \
         the header is necessary, and recovery must treat such entries \
         conservatively"
    );
}

#[test]
fn unresolvable_table_token_falls_back_like_the_projection() {
    let mut tx = TxContext::new(TxId::new(1), 0, 0, crate::types::IsolationLevel::Snapshot);
    tx.write_set.insert(123, staging(ops_vec()));
    let mut entry = WalEntryV2::new(
        1,
        0,
        vec![WalOpV2::Delete {
            table_id_interned: 123,
            rid: rid(6),
        }],
    )
    .with_commit_version(1);
    entry.event = Some(event_header_from_tx(&tx, "main"));
    let event = event_from_wal_entry(&entry).unwrap();
    assert_eq!(event.changes[0].table, "token:123");
}

fn ops_vec() -> Vec<KvOp> {
    vec![]
}

#[test]
fn implicit_tx_reports_public_tx_id_zero() {
    let mut tx = TxContext::new(TxId::new(42), 0, 0, crate::types::IsolationLevel::Snapshot);
    tx.implicit = true;
    tx.write_set.insert(9, staging(ops_vec()));
    let header: WalEventHeader = event_header_from_tx(&tx, "main");
    assert_eq!(header.tx_id, 0, "implicit batch writes report 0");

    let mut tx2 = TxContext::new(TxId::new(42), 0, 0, crate::types::IsolationLevel::Snapshot);
    tx2.write_set.insert(9, staging(ops_vec()));
    let header2 = event_header_from_tx(&tx2, "main");
    assert_eq!(header2.tx_id, 42, "explicit txs keep their real id");
}

#[test]
fn extra_change_delete_carries_no_value() {
    let mut tx = TxContext::new(TxId::new(1), 0, 0, crate::types::IsolationLevel::Snapshot);
    tx.table_tokens.insert(9, "users".to_string());
    tx.write_set.insert(
        9,
        staging(vec![KvOp::Remove(RecordKey::from_slice(b"sys:gone"))]),
    );
    let header = event_header_from_tx(&tx, "main");
    assert_eq!(header.extra_changes.len(), 1);
    assert_eq!(header.extra_changes[0].value, None);
    assert_eq!(header.extra_changes[0].change_index, 0);

    let mut entry = WalEntryV2::new(1, 0, vec![]).with_commit_version(1);
    entry.event = Some(header);
    let event = event_from_wal_entry(&entry).unwrap();
    assert_eq!(
        event.changes,
        vec![RecordChange {
            table: "users".to_string(),
            key: Bytes::from_static(b"sys:gone"),
            op: ChangeOp::Delete,
            value: None,
        }]
    );
}
