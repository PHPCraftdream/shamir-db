//! Randomized property: the derived event reproduces the projected change
//! order for arbitrary write-sets (S5c-X I1). Deterministic (seeded
//! xorshift; no wall clock, no network, no fs dependence beyond the entry
//! encode/decode round trip).

use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{KvOp, RecordKey};
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;
use shamir_wal::{WalEntryV2, WalOpV2};
use std::sync::Arc;

use crate::staging_store::StagingStore;
use crate::tx_context::TxContext;
use crate::types::TxId;
use crate::wal_event::{event_from_wal_entry, event_header_from_tx};

/// Deterministic xorshift64* PRNG (no external dependency).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn rid(n: u8) -> RecordId {
    let mut a = [0u8; 16];
    a[15] = n;
    RecordId(a)
}

#[test]
fn derived_event_matches_projection_for_random_write_sets() {
    let mut rng = Rng(0x5EED_5C5C_A111_0001);
    for case in 0..300u32 {
        let mut tx = TxContext::new(
            TxId::new(case as u64 + 1),
            0,
            0,
            crate::IsolationLevel::Snapshot,
        );
        tx.actor = match rng.below(3) {
            0 => Actor::System,
            1 => Actor::Admin(rng.below(100)),
            _ => Actor::User(rng.below(100)),
        };
        tx.implicit = rng.below(2) == 0;

        let n_tables = rng.below(4); // 0..=3 tables
        for t in 0..n_tables {
            let token = 100 + t;
            tx.table_tokens.insert(token, format!("table_{t}"));
            let mut staging = StagingStore::new(Arc::new(InMemoryStore::new()));
            let n_ops = rng.below(7); // 0..=6 ops
            for o in 0..n_ops {
                let non_record = rng.below(4) == 0; // ~25% non-RecordId keys
                let remove = rng.below(3) == 0;
                let key: Vec<u8> = if non_record {
                    format!("sys:{t}:{o}").into_bytes()
                } else {
                    rid((o as u8) ^ ((t as u8) * 7)).as_bytes().to_vec()
                };
                if remove {
                    staging.remove(RecordKey::from_slice(&key));
                } else {
                    staging.set(
                        RecordKey::from_slice(&key),
                        Bytes::from(format!("v{case}:{t}:{o}")),
                    );
                }
            }
            tx.write_set.insert(token, staging);
        }

        // Entry: header from the tx, data ops exactly as the live
        // `wal_ops_from_tx` emits them (RecordId-keyed Set/Remove only).
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
        let entry = WalEntryV2::new(tx.tx_id.0, 0, ops)
            .with_commit_version(case as u64 + 100)
            .with_event_header(event_header_from_tx(&tx, "prop"));

        // Expected: the live projection (the unprotected reference walk).
        let expected = crate::changefeed::project_event(&tx, "prop", case as u64 + 100);

        // Derived: from the entry, after a full encode/decode round trip.
        let decoded = WalEntryV2::decode(&entry.encode().unwrap()).unwrap();
        let derived = event_from_wal_entry(&decoded);

        match expected {
            None => {
                assert!(
                    derived.is_none(),
                    "case {case}: eventless commit must yield no event"
                );
            }
            Some(mut expected) => {
                // The header's timestamp is sampled at entry construction,
                // the projection's after validation: same clock, different
                // instant. Only the timestamp may differ; the header's value
                // must never run AHEAD of the projection's sample moment.
                let proj_ts = expected.timestamp_ns;
                expected.timestamp_ns = derived.as_ref().unwrap().timestamp_ns;
                assert_eq!(
                    derived.as_ref().unwrap().timestamp_ns,
                    decoded.event.as_ref().unwrap().timestamp_ns
                );
                assert!(
                    derived.as_ref().unwrap().timestamp_ns <= proj_ts + 1,
                    "case {case}: header timestamp must not drift ahead of the \
                     projection's sampling moment"
                );
                assert_eq!(
                    derived,
                    Some(expected),
                    "case {case}: derived event must equal the projected event \
                     (repo, version, tx_id, actor and full change order)"
                );
            }
        }
    }
}
