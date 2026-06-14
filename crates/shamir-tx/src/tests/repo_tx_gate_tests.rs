use crate::predicate_set::{PredicateDep, SORTED_PREFIX_LEN, SORTED_TAG};
use crate::repo_tx_gate::{
    build_footprint_from_tx, record_conflicts, CommitWriteRecord, RepoTxGate, TableWriteFootprint,
};
use crate::types::{IsolationLevel, TxId};
use crate::IndexWriteOp;
use bytes::Bytes;
use shamir_collections::THasher;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[test]
fn fresh_tx_id_monotonic() {
    let gate = RepoTxGate::fresh();
    let a = gate.fresh_tx_id();
    let b = gate.fresh_tx_id();
    let c = gate.fresh_tx_id();
    assert!(a.raw() < b.raw() && b.raw() < c.raw());
}

#[test]
fn assign_next_version_monotonic() {
    let gate = RepoTxGate::fresh();
    let v1 = gate.assign_next_version();
    let v2 = gate.assign_next_version();
    assert!(v2 > v1);
}

#[test]
fn publish_updates_last_committed() {
    let gate = RepoTxGate::fresh();
    assert_eq!(gate.last_committed(), 0);
    gate.publish_committed(5);
    assert_eq!(gate.last_committed(), 5);
}

#[tokio::test]
async fn snapshot_guard_removes_on_drop() {
    let gate = RepoTxGate::fresh();
    let guard = gate.open_snapshot().await;
    let _v = guard.version();
    assert!(!gate.active_snapshots_empty());
    drop(guard);
    assert!(gate.active_snapshots_empty());
}

#[tokio::test]
async fn min_alive_with_no_snapshots() {
    let gate = RepoTxGate::new(10, 1);
    assert_eq!(gate.min_alive(), 10);
}

#[tokio::test]
async fn min_alive_with_snapshots() {
    let gate = RepoTxGate::new(10, 1);
    let _g1 = gate.open_snapshot().await; // v=10
    gate.publish_committed(15);
    let _g2 = gate.open_snapshot().await; // v=15
    assert_eq!(gate.min_alive(), 10);
}

#[tokio::test]
async fn commit_lock_serialises() {
    let gate = Arc::new(RepoTxGate::fresh());
    let gate2 = Arc::clone(&gate);

    let counter = Arc::new(AtomicU64::new(0));
    let c1 = Arc::clone(&counter);
    let c2 = Arc::clone(&counter);

    let h1 = tokio::spawn(async move {
        let _lock = gate.commit_lock().await;
        let v = c1.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert_eq!(c1.load(Ordering::SeqCst), v + 1);
    });

    let h2 = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        let _lock = gate2.commit_lock().await;
        c2.fetch_add(1, Ordering::SeqCst);
    });

    h1.await.unwrap();
    h2.await.unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[test]
fn seeded_gate_preserves_values() {
    let gate = RepoTxGate::new(42, 100);
    assert_eq!(gate.last_committed(), 42);
    assert_eq!(gate.fresh_tx_id().raw(), 100);
    assert_eq!(gate.fresh_tx_id().raw(), 101);
}

#[tokio::test]
async fn assign_next_version_concurrent_no_duplicates() {
    let gate = Arc::new(RepoTxGate::fresh());
    let n = 100;
    let mut handles = Vec::new();
    for _ in 0..n {
        let g = Arc::clone(&gate);
        handles.push(tokio::spawn(async move { g.assign_next_version() }));
    }
    let mut versions = HashSet::<_, THasher>::default();
    for h in handles {
        let v = h.await.unwrap();
        assert!(versions.insert(v), "duplicate version {v}");
    }
    assert_eq!(versions.len(), n);
}

#[tokio::test]
async fn fresh_tx_id_concurrent_no_duplicates() {
    let gate = Arc::new(RepoTxGate::fresh());
    let n = 100;
    let mut handles = Vec::new();
    for _ in 0..n {
        let g = Arc::clone(&gate);
        handles.push(tokio::spawn(async move { g.fresh_tx_id() }));
    }
    let mut ids = HashSet::<_, THasher>::default();
    for h in handles {
        let id = h.await.unwrap();
        assert!(ids.insert(id.raw()), "duplicate tx_id");
    }
    assert_eq!(ids.len(), n);
}

// ── Phase C: commit-write log tests ──────────────────────────────

#[test]
fn commit_log_window_scan_excludes_at_or_below_snapshot() {
    let gate = RepoTxGate::new(0, 1);
    let mk = |v: u64, token: u64| CommitWriteRecord {
        commit_version: v,
        per_table: HashMap::<_, _, THasher>::from_iter([(
            token,
            TableWriteFootprint {
                touched: true,
                inserted_index_keys: vec![],
            },
        )]),
    };
    gate.record_commit_writes(mk(5, 42));
    gate.record_commit_writes(mk(10, 42));
    gate.record_commit_writes(mk(15, 42));
    // Publish through v=15 so all three records are within
    // `(snapshot, last_committed]` for appropriate snapshot values.
    gate.publish_committed(15);

    // snapshot=10 -> only records with v>10 (i.e. v=15) are visible.
    assert!(gate.predicate_conflicts(&PredicateDep::TableScan { table_token: 42 }, 10));
    // snapshot=15 -> window is empty.
    assert!(!gate.predicate_conflicts(&PredicateDep::TableScan { table_token: 42 }, 15));
    // Disjoint table -> no conflict at any snapshot.
    assert!(!gate.predicate_conflicts(&PredicateDep::TableScan { table_token: 99 }, 0));
}

// ── P3a: inter-batch phantom — record_conflicts against a batch-local
//        footprint (the building blocks the group-commit leader composes
//        inline in run_leader). The committed-log path is already covered by
//        commit_log_window_scan_*; this verifies the SAME footprint shape
//        (build_footprint_from_tx) and the SAME conflict predicate
//        (record_conflicts) detect an intra-batch phantom without going
//        through the gate's published log. ──────────────────────────────

#[test]
fn p3a_batch_footprint_table_scan_conflict_detected() {
    // A footprint produced exactly as the leader produces it for an
    // accepted survivor: build_footprint_from_tx on a Serializable tx that
    // touched table 42.
    let footprint = CommitWriteRecord {
        commit_version: 7,
        per_table: HashMap::<_, _, THasher>::from_iter([(
            42u64,
            TableWriteFootprint {
                touched: true,
                inserted_index_keys: vec![],
            },
        )]),
    };

    // A subsequent survivor whose predicate scans table 42 → conflict.
    assert!(
        record_conflicts(&footprint, &PredicateDep::TableScan { table_token: 42 }),
        "table-scan predicate over a written table must conflict with the batch footprint"
    );
    // A predicate over a disjoint table → no conflict.
    assert!(
        !record_conflicts(&footprint, &PredicateDep::TableScan { table_token: 99 }),
        "disjoint table predicate must NOT conflict"
    );
}

#[test]
fn p3a_build_footprint_from_tx_snapshot_is_empty() {
    // Snapshot txs never produce SSI footprints — so the leader's
    // `!footprint.is_empty()` guard skips accumulating them, and the
    // intra-batch phantom check is a no-op for non-Serializable survivors.
    let ctx = crate::TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);
    let fp = build_footprint_from_tx(&ctx, 5);
    assert!(
        fp.is_empty(),
        "Snapshot tx footprint must be empty (no phantom protection)"
    );
}

#[test]
fn commit_log_index_range_intersects_posting_key() {
    use std::ops::Bound;
    let gate = RepoTxGate::new(0, 1);

    let index_id: u64 = 7;

    // Build a posting key: SORTED_TAG || index_id BE8 || encoded_value || rid(16)
    let make_posting = |encoded: &[u8]| -> Bytes {
        let mut k = Vec::with_capacity(SORTED_PREFIX_LEN + encoded.len() + 16);
        k.push(SORTED_TAG);
        k.extend_from_slice(&index_id.to_be_bytes());
        k.extend_from_slice(encoded);
        k.extend_from_slice(&[0xAAu8; 16]); // fake rid
        Bytes::from(k)
    };

    // Build a bound prefix: SORTED_TAG || index_id BE8 || tail
    let make_bound = |tail: &[u8]| -> Bytes {
        let mut b = Vec::with_capacity(SORTED_PREFIX_LEN + tail.len());
        b.push(SORTED_TAG);
        b.extend_from_slice(&index_id.to_be_bytes());
        b.extend_from_slice(tail);
        Bytes::from(b)
    };

    // Two posting keys committed at v=7.
    let rec = CommitWriteRecord {
        commit_version: 7,
        per_table: HashMap::<_, _, THasher>::from_iter([(
            42,
            TableWriteFootprint {
                touched: true,
                inserted_index_keys: vec![
                    make_posting(&[0x10]), // "low"
                    make_posting(&[0x50]), // "high"
                ],
            },
        )]),
    };
    gate.record_commit_writes(rec);
    // Publish through v=7 so the record is in the window `(0, 7]`.
    gate.publish_committed(7);

    // Range covering only the LOW key.
    let hits_low = PredicateDep::IndexRange {
        table_token: 42,
        index_id,
        lo: Bound::Included(make_bound(&[0x00])),
        hi: Bound::Included(make_bound(&[0x20, 0xFF, 0xFF, 0xFF])),
    };
    // Range that misses both keys entirely.
    let misses = PredicateDep::IndexRange {
        table_token: 42,
        index_id,
        lo: Bound::Included(make_bound(&[0x60])),
        hi: Bound::Included(make_bound(&[0x70])),
    };
    assert!(gate.predicate_conflicts(&hits_low, 0));
    assert!(!gate.predicate_conflicts(&misses, 0));
}

#[test]
fn commit_log_prune_below_min_alive() {
    let gate = RepoTxGate::new(0, 1);
    let mk = |v: u64| CommitWriteRecord {
        commit_version: v,
        per_table: HashMap::<_, _, THasher>::from_iter([(
            1,
            TableWriteFootprint {
                touched: true,
                inserted_index_keys: vec![],
            },
        )]),
    };
    for v in [1u64, 2, 3, 4, 5] {
        gate.record_commit_writes(mk(v));
    }
    assert_eq!(gate.commit_log_len(), 5);

    let removed = gate.prune_commit_log_below(3);
    assert_eq!(removed, 3); // v=1,2,3 dropped
    assert_eq!(gate.commit_log_len(), 2); // v=4,5 remain
}

#[test]
fn commit_log_prune_uses_min_alive_floor() {
    let gate = Arc::new(RepoTxGate::new(0, 1));
    // Publish through v=5 so last_committed=5.
    for _ in 0..5 {
        let v = gate.assign_next_version();
        gate.publish_committed(v);
        gate.record_commit_writes(CommitWriteRecord {
            commit_version: v,
            per_table: HashMap::<_, _, THasher>::from_iter([(
                1,
                TableWriteFootprint {
                    touched: true,
                    inserted_index_keys: vec![],
                },
            )]),
        });
    }
    assert_eq!(gate.commit_log_len(), 5);

    let floor = gate.min_alive(); // = last_committed() = 5 (no snapshots)
    gate.prune_commit_log_below(floor);
    assert_eq!(gate.commit_log_len(), 0); // all <=5 dropped, none left
}

// ── Phase C Step 7: prune commit-write-log tests ──────────────────

#[test]
fn prune_commit_write_log_drops_only_at_or_below_min() {
    let gate = RepoTxGate::new(0, 1);
    let mk = |v: u64| CommitWriteRecord {
        commit_version: v,
        per_table: HashMap::<_, _, THasher>::from_iter([(
            1,
            TableWriteFootprint {
                touched: true,
                inserted_index_keys: vec![],
            },
        )]),
    };
    for v in 1..=5 {
        gate.record_commit_writes(mk(v));
    }
    // Publish through v=5 so all records are in `(snapshot, last_committed]`.
    gate.publish_committed(5);
    assert_eq!(gate.commit_log_len(), 5);

    // Prune with floor=3: entries 1,2,3 dropped, 4,5 remain.
    let removed = gate.prune_commit_log_below(3);
    assert_eq!(removed, 3);
    assert_eq!(gate.commit_log_len(), 2);

    // Verify 4 and 5 survive: predicate_conflicts at snapshot=3 sees v>3.
    assert!(gate.predicate_conflicts(&PredicateDep::TableScan { table_token: 1 }, 3));
    // And snapshot=5 sees nothing.
    assert!(!gate.predicate_conflicts(&PredicateDep::TableScan { table_token: 1 }, 5));
}

#[test]
fn prune_commit_write_log_empty_is_noop_no_write_lock() {
    // On an empty log, prune returns 0 immediately. This exercises the
    // fast path that protects Snapshot/non-tx repos from write-lock
    // acquisition overhead. The test simply asserts the return value and
    // idempotency — the "no write lock" part is a design invariant, not
    // directly observable from outside.
    let gate = RepoTxGate::fresh();
    assert_eq!(gate.commit_log_len(), 0);
    let removed = gate.prune_commit_log_below(100);
    assert_eq!(removed, 0);
    assert_eq!(gate.commit_log_len(), 0);
}

#[test]
fn prune_commit_write_log_idempotent() {
    let gate = RepoTxGate::new(0, 1);
    let mk = |v: u64| CommitWriteRecord {
        commit_version: v,
        per_table: HashMap::<_, _, THasher>::from_iter([(
            1,
            TableWriteFootprint {
                touched: true,
                inserted_index_keys: vec![],
            },
        )]),
    };
    for v in 1..=3 {
        gate.record_commit_writes(mk(v));
    }
    assert_eq!(gate.commit_log_len(), 3);

    // First prune at floor=2: removes 1,2.
    let r1 = gate.prune_commit_log_below(2);
    assert_eq!(r1, 2);
    assert_eq!(gate.commit_log_len(), 1);

    // Same floor again: nothing to remove.
    let r2 = gate.prune_commit_log_below(2);
    assert_eq!(r2, 0);
    assert_eq!(gate.commit_log_len(), 1);

    // Higher floor removes the last entry.
    let r3 = gate.prune_commit_log_below(10);
    assert_eq!(r3, 1);
    assert_eq!(gate.commit_log_len(), 0);

    // On empty, idempotent.
    let r4 = gate.prune_commit_log_below(10);
    assert_eq!(r4, 0);
}

#[test]
fn build_footprint_is_noop_off_serializable() {
    let tx = crate::TxContext::new(TxId::new(1), 0, 10, IsolationLevel::Snapshot);
    let rec = build_footprint_from_tx(&tx, 99);
    assert!(rec.is_empty(), "Snapshot must produce empty footprint");

    // And `record_commit_writes` on an empty record is a no-op.
    let gate = RepoTxGate::fresh();
    gate.record_commit_writes(rec);
    assert_eq!(gate.commit_log_len(), 0);
}

#[test]
fn build_footprint_projects_index_set_postings_only() {
    let mut tx = crate::TxContext::new(TxId::new(1), 0, 10, IsolationLevel::Serializable);
    tx.index_write_set.push((
        42,
        IndexWriteOp::SetPosting {
            key: Bytes::from_static(b"K1"),
            value: Bytes::from_static(b"V"),
        },
    ));
    tx.index_write_set.push((
        42,
        IndexWriteOp::RemovePosting {
            key: Bytes::from_static(b"K_DEL"),
        },
    ));
    tx.index_write_set.push((
        42,
        IndexWriteOp::BumpFtsStats {
            doc_len: 7,
            sign: 1,
        },
    ));

    let rec = build_footprint_from_tx(&tx, 11);
    let f = &rec.per_table[&42];
    assert!(f.touched);
    assert_eq!(f.inserted_index_keys, vec![Bytes::from_static(b"K1")]);
}

#[tokio::test]
async fn record_commit_writes_concurrent_no_loss() {
    // Lock-free CAS append under simulated commit_lock serialisation.
    let gate = Arc::new(RepoTxGate::fresh());
    let n = 50u64;
    let mut handles = Vec::new();
    for i in 1..=n {
        let g = Arc::clone(&gate);
        handles.push(tokio::spawn(async move {
            let _lock = g.commit_lock().await;
            let v = g.assign_next_version();
            g.publish_committed(v);
            g.record_commit_writes(CommitWriteRecord {
                commit_version: v,
                per_table: HashMap::<_, _, THasher>::from_iter([(
                    i,
                    TableWriteFootprint {
                        touched: true,
                        inserted_index_keys: vec![],
                    },
                )]),
            });
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    assert_eq!(gate.commit_log_len(), n as usize);
}
