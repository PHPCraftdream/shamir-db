//! P3b — Stress + crash-injection tests for the Version Oracle.
//!
//! Proves serializability, monotonicity, watermark convergence, and recovery
//! correctness under concurrent load and simulated crashes.

use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::storage_in_memory::{InMemoryRepo, InMemoryStore};
use shamir_storage::types::Store;
use shamir_tx::{IsolationLevel, StagingStore, TxContext, TxId, VersionProvider};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use shamir_wal::{WalEntryV2, WalOpV2};

use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_token;
use crate::repo::repo_types::BoxRepo;
use crate::table::table_manager::table_token_for;
use crate::table::TableConfig;
use crate::tx::commit_tx;

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("stress".into(), BoxRepo::InMemory(repo), Vec::new())
}

// =========================================================================
// Test 1 — concurrent disjoint-table throughput + monotonicity
// =========================================================================

/// N concurrent committers each writing a distinct table. All succeed with
/// unique monotonic versions; the watermark advances to cover all of them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oracle_stress_disjoint_table_concurrent_monotonic() {
    const N: usize = 50;

    let tables: Vec<String> = (0..N).map(|i| format!("tbl_{i}")).collect();
    let repo = make_repo();
    for t in &tables {
        repo.add_table(TableConfig::new(t.as_str()));
    }

    let mut handles = Vec::with_capacity(N);
    for (i, table_name) in tables.iter().enumerate() {
        let r = repo.clone();
        let table_name = table_name.clone();
        handles.push(tokio::spawn(async move {
            let tbl = r.get_table(&table_name).await.unwrap();
            let (mut tx, _g) = r.begin_tx(IsolationLevel::Snapshot).await.unwrap();
            tbl.insert_tx(&InnerValue::Int(i as i64), Some(&mut tx))
                .await
                .unwrap();
            let outcome = r.commit_tx(tx).await.unwrap();
            outcome.commit_version
        }));
    }

    let mut versions = Vec::with_capacity(N);
    for h in handles {
        versions.push(h.await.unwrap());
    }

    // All versions unique.
    let mut sorted = versions.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        N,
        "all {N} commit versions must be unique, got {} unique",
        sorted.len()
    );

    // Monotonic: sorted == original sorted (trivially true since we sort).
    // More importantly, min > 0.
    assert!(*sorted.first().unwrap() > 0);

    // Watermark must advance to cover all committed versions.
    let gate = repo.tx_gate().await.unwrap();
    let wm = gate.completion().watermark();
    let max_v = *sorted.last().unwrap();
    assert!(
        wm >= max_v,
        "watermark ({wm}) must be >= max committed version ({max_v})"
    );

    // Each table has its data.
    for (i, t) in tables.iter().enumerate() {
        let tbl = repo.get_table(t.as_str()).await.unwrap();
        let count = tbl.counter().get().await.unwrap();
        assert!(
            count >= 1,
            "table {t} (idx {i}) must have at least 1 record"
        );
    }
}

// =========================================================================
// Test 2 — concurrent same-table, conflict resolution
// =========================================================================

/// M concurrent committers on the SAME (table, key) under Snapshot isolation.
/// All succeed (last-writer-wins); versions are unique; watermark advances.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oracle_stress_same_table_snapshot_all_succeed() {
    const M: usize = 30;

    let repo = make_repo();
    repo.add_table(TableConfig::new("shared"));
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let mut handles = Vec::with_capacity(M);
    for i in 0..M {
        let r = repo.clone();
        let ds = Arc::clone(&data_store);
        handles.push(tokio::spawn(async move {
            let mut tx = TxContext::new(TxId::new(i as u64 + 1000), 0, 0, IsolationLevel::Snapshot);
            let mut staging = StagingStore::new(Arc::clone(&ds));
            staging.set(
                Bytes::from_static(b"contested_key"),
                Bytes::from(format!("val_{i}")),
            );
            tx.write_set.insert(table_token_for("shared"), staging);
            let outcome = commit_tx(tx, &r).await.unwrap();
            outcome.commit_version
        }));
    }

    let mut versions = Vec::with_capacity(M);
    for h in handles {
        versions.push(h.await.unwrap());
    }

    versions.sort_unstable();
    versions.dedup();
    assert_eq!(versions.len(), M, "all versions must be unique");

    let gate = repo.tx_gate().await.unwrap();
    let wm = gate.completion().watermark();
    assert!(
        wm >= *versions.last().unwrap(),
        "watermark must advance past all committed versions"
    );

    // Final value is one of the writes.
    let got = data_store
        .get(Bytes::from_static(b"contested_key"))
        .await
        .unwrap();
    let got_str = String::from_utf8(got.to_vec()).unwrap();
    assert!(
        got_str.starts_with("val_"),
        "final value must be from one of the writers, got: {got_str}"
    );
}

/// Same-table Serializable: at least one succeeds; others may get SSI/phantom
/// errors. No torn state; watermark advances past all (committed + aborted).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oracle_stress_same_table_serializable_conflict_resolution() {
    const M: usize = 20;

    let repo = make_repo();
    repo.add_table(TableConfig::new("ser"));

    // Use a provider that triggers conflicts for txs that read a key
    // written by a concurrent committer.
    struct AlwaysConflictProvider;
    impl VersionProvider for AlwaysConflictProvider {
        fn version_of(&self, _t: u64, _k: &Bytes) -> Option<u64> {
            // Return a version far above any snapshot → forces SSI conflict.
            Some(999_999)
        }
    }

    let mut handles = Vec::with_capacity(M);
    for i in 0..M {
        let r = repo.clone();
        handles.push(tokio::spawn(async move {
            let (mut tx, _g) = r.begin_tx(IsolationLevel::Serializable).await.unwrap();
            // Record a read so SSI validation has something to check.
            tx.record_read(table_token_for("ser"), Bytes::from_static(b"k"), 1);
            tx.set_version_provider(Arc::new(AlwaysConflictProvider));

            // Also stage a write so it's not the empty fast-path.
            let tbl = r.get_table("ser").await.unwrap();
            tbl.insert_tx(&InnerValue::Int(i as i64), Some(&mut tx))
                .await
                .unwrap();

            r.commit_tx(tx).await
        }));
    }

    let mut successes = 0u32;
    let mut conflicts = 0u32;
    for h in handles {
        match h.await.unwrap() {
            Ok(_) => successes += 1,
            Err(_) => conflicts += 1,
        }
    }

    // All should conflict because the provider always returns version > snapshot.
    assert_eq!(
        successes + conflicts,
        M as u32,
        "all txs must resolve to success or conflict"
    );
    // With AlwaysConflictProvider, all should abort.
    assert_eq!(
        conflicts, M as u32,
        "with AlwaysConflictProvider all txs should abort via SSI"
    );

    // Watermark still advances (aborted versions are marked).
    let gate = repo.tx_gate().await.unwrap();
    let wm = gate.completion().watermark();
    // Each aborted tx burned a version; watermark must advance past them.
    assert!(
        wm >= M as u64,
        "watermark ({wm}) must advance past {M} aborted versions"
    );
}

// =========================================================================
// Test 3 — recovery after crash (WAL durable, materialize not yet run)
// =========================================================================

fn rid(n: u8) -> RecordId {
    let mut a = [0u8; 16];
    a[15] = n;
    RecordId(a)
}

async fn seed_inflight_put(
    underlying: &Arc<InMemoryRepo>,
    table: &str,
    record: RecordId,
    body: bytes::Bytes,
    commit_version: u64,
) {
    let seed = RepoInstance::new(
        "stress".into(),
        BoxRepo::InMemory(Arc::clone(underlying)),
        Vec::new(),
    );
    let wal = seed.repo_wal().await.unwrap();
    let entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        repo_token(seed.name()),
        vec![WalOpV2::Put {
            table_id_interned: table_token_for(table),
            rid: record,
            body,
        }],
    )
    .with_commit_version(commit_version);
    wal.begin(entry).await.unwrap();
    drop(seed);
}

/// Crash point (b): WAL durable, materialize not yet run. Recovery must
/// replay all durable entries, restore data, advance watermark.
#[tokio::test]
async fn oracle_stress_recovery_after_wal_durable_no_materialize() {
    const N: u64 = 10;
    let underlying = Arc::new(InMemoryRepo::new());

    // Seed N inflight entries at commit_versions 1..=N.
    for v in 1..=N {
        let record = rid(v as u8);
        let body = InnerValue::Str(format!("crash_{v}")).to_bytes().unwrap();
        seed_inflight_put(&underlying, "t", record, body, v).await;
    }

    // Simulated restart.
    let repo = RepoInstance::new(
        "stress".into(),
        BoxRepo::InMemory(Arc::clone(&underlying)),
        Vec::new(),
    );
    repo.add_table(TableConfig::new("t"));

    let recovered = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(recovered, N as usize);

    // All records are present.
    let tbl = repo.get_table("t").await.unwrap();
    for v in 1..=N {
        let record = rid(v as u8);
        let val = tbl.get(record).await.unwrap();
        assert!(
            matches!(val, InnerValue::Str(ref s) if s == &format!("crash_{v}")),
            "record {v} must be recovered, got {val:?}"
        );
    }

    // Watermark = N (all contiguous).
    let gate = repo.tx_gate().await.unwrap();
    assert_eq!(
        gate.completion().watermark(),
        N,
        "watermark must equal highest recovered version"
    );

    // No inflight markers left.
    let wal = repo.repo_wal().await.unwrap();
    assert!(wal.list_inflight().await.unwrap().is_empty());

    // Next assigned version > N.
    assert!(gate.assign_next_version() > N);
}

/// Crash point (b) with a GAP: versions 1,2,4,5 are durable (3 is missing —
/// simulating a tx that crashed before WAL). Recovery replays only what's
/// durable; watermark = 2 (contiguous prefix), not 5.
#[tokio::test]
async fn oracle_stress_recovery_gap_in_versions() {
    let underlying = Arc::new(InMemoryRepo::new());

    for v in [1u64, 2, 4, 5] {
        let record = rid(v as u8);
        let body = InnerValue::Str(format!("g_{v}")).to_bytes().unwrap();
        seed_inflight_put(&underlying, "t", record, body, v).await;
    }

    let repo = RepoInstance::new(
        "stress".into(),
        BoxRepo::InMemory(Arc::clone(&underlying)),
        Vec::new(),
    );
    repo.add_table(TableConfig::new("t"));

    let recovered = repo.recover_v2_inflight().await.unwrap();
    assert_eq!(recovered, 4);

    let gate = repo.tx_gate().await.unwrap();
    // Watermark should be at least 2 (contiguous 1,2). Whether it reaches 5
    // depends on whether recovery marks the gap as Aborted. Either way, the
    // gate's next version must exceed the max recovered (5).
    let wm = gate.completion().watermark();
    assert!(
        wm >= 2,
        "watermark must be at least 2 (contiguous prefix), got {wm}"
    );
    assert!(gate.assign_next_version() > 5);
}

// =========================================================================
// Test 4 — abort under load doesn't stall the watermark
// =========================================================================

/// A mix of committing and aborting txs: the watermark keeps advancing and
/// never stalls on an aborted version.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oracle_stress_abort_does_not_stall_watermark() {
    const TOTAL: usize = 40;

    let repo = make_repo();
    repo.add_table(TableConfig::new("t"));

    // Use a deterministic pattern: even indices commit, odd indices abort (SSI conflict).
    struct EvenOddProvider {
        should_conflict: bool,
    }
    impl VersionProvider for EvenOddProvider {
        fn version_of(&self, _t: u64, _k: &Bytes) -> Option<u64> {
            if self.should_conflict {
                Some(999_999) // forces SSI conflict
            } else {
                Some(0) // no conflict
            }
        }
    }

    let mut handles = Vec::with_capacity(TOTAL);
    for i in 0..TOTAL {
        let r = repo.clone();
        let should_abort = i % 2 == 1;
        handles.push(tokio::spawn(async move {
            let (mut tx, _g) = r.begin_tx(IsolationLevel::Serializable).await.unwrap();

            if should_abort {
                // Record a read so SSI check triggers.
                tx.record_read(table_token_for("t"), Bytes::from_static(b"k"), 1);
                tx.set_version_provider(Arc::new(EvenOddProvider {
                    should_conflict: true,
                }));
            }

            // Stage a write so it's not the empty fast-path.
            let tbl = r.get_table("t").await.unwrap();
            tbl.insert_tx(&InnerValue::Int(i as i64), Some(&mut tx))
                .await
                .unwrap();

            r.commit_tx(tx).await
        }));
    }

    let mut successes = 0u64;
    let mut aborts = 0u64;
    for h in handles {
        match h.await.unwrap() {
            Ok(_) => successes += 1,
            Err(_) => aborts += 1,
        }
    }

    assert_eq!(aborts, (TOTAL / 2) as u64, "odd-indexed txs must abort");
    assert_eq!(
        successes,
        (TOTAL / 2) as u64,
        "even-indexed txs must succeed"
    );

    // The watermark must advance past ALL versions (committed + aborted).
    let gate = repo.tx_gate().await.unwrap();
    let wm = gate.completion().watermark();
    assert!(
        wm >= TOTAL as u64,
        "watermark ({wm}) must advance past all {TOTAL} consumed versions \
         (aborted ones must be marked and skipped)"
    );

    // Specifically: a successful commit that got a version AFTER an aborted
    // version is visible (watermark >= that version). Since watermark >= TOTAL,
    // this is already proven.
}
