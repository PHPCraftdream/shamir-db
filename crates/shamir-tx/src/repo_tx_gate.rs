//! Per-repo transactional gate — serialises the commit phase.
//!
//! One `RepoTxGate` lives per repo. It owns:
//! - A `commit_mutex` that serialises the critical commit section
//!   (assign version → SSI validation → physical writes → publish).
//! - A monotonic `version_counter` (AtomicU64) for MVCC versioning.
//! - A durable `last_committed_version` (AtomicU64) that recovery
//!   reads on repo open to know which versions are visible.
//! - An `active_snapshots` set so GC knows the oldest open snapshot.
//! - A `next_tx_id` counter for allocating unique tx identifiers.
//!
//! Non-tx writes bypass the gate entirely — zero overhead on
//! non-transactional hot paths.

use bytes::Bytes;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::TxId;

// ── Phase C: commit-write log structs ──────────────────────────────

/// PROPOSED (Phase C). One captured commit's index-posting footprint for
/// the predicate-conflict check. Built engine-side from `TxContext` state
/// already collected at commit time (see `build_footprint_from_tx`).
///
/// Wire-shape mirror of the doc section 5.2 sketch. NOT durable: regenerated only
/// while a Serializable tx is alive — same lifetime discipline as
/// `MvccStore::version_cache` (mvcc_store.rs:446) and `active_snapshots`.
#[derive(Debug, Clone, Default)]
pub struct TableWriteFootprint {
    /// Set iff the tx wrote ANYTHING (data/index/counter) on this table.
    /// Drives the coarse `PredicateDep::TableScan` check.
    pub touched: bool,
    /// `SetPosting.key`s the tx staged for this table (already collected
    /// at `tx.index_write_set` — tx_context.rs:72 ; SetPosting variant at
    /// index_write_op.rs:15). `Bytes` is refcounted: this clone is cheap.
    /// Sorted ascending so future `binary_search` optimisations are free.
    pub inserted_index_keys: Vec<Bytes>,
}

/// PROPOSED (Phase C). One committed tx's write footprint, stored in the
/// commit-write log for predicate-conflict checks.
#[derive(Debug, Clone)]
pub struct CommitWriteRecord {
    pub commit_version: u64,
    /// Keyed by table_token (engine's `table_token()`).
    pub per_table: HashMap<u64, TableWriteFootprint>,
}

impl CommitWriteRecord {
    pub fn is_empty(&self) -> bool {
        self.per_table.is_empty()
    }
}

/// Per-repo transactional synchronisation point.
pub struct RepoTxGate {
    /// Serialises the commit phase. Held across the whole critical
    /// section: pre-commit (interner-overlay merge → SSI validation →
    /// unique re-check → WAL begin), then data + index materialization
    /// (Phase 5a data writes + Phase 5c index writes), publish, durable
    /// markers, and WAL cleanup — it is released only after
    /// `materialize` returns. The per-vector HNSW promote runs OUTSIDE
    /// the lock (post-`materialize`). So the section is O(rows + index
    /// postings) of storage work, not a fixed wall-clock budget.
    /// `tokio::sync::Mutex` because the guard lives across `.await`.
    commit_mutex: tokio::sync::Mutex<()>,

    /// Monotonic MVCC version counter. Incremented on every commit.
    version_counter: AtomicU64,

    /// The highest version that has been fully published. Readers
    /// see only versions ≤ this value.
    last_committed_version: AtomicU64,

    /// Monotonic tx-id allocator.
    next_tx_id: AtomicU64,

    /// Set of open snapshot versions. GC must not delete any version
    /// ≥ `min(active_snapshots)`.
    active_snapshots: Arc<scc::HashMap<u64, ()>>,

    /// PROPOSED (Phase C). Ring of recently-committed write footprints.
    /// Keyed by `commit_version` (monotonic), so a `range(snapshot..]`
    /// scan visits exactly the tx's conflict window. `scc::TreeIndex`
    /// is lock-free CAS-based — no `RwLock` poisoning and no contention
    /// on the gate's `commit_lock`. Empty unless at least one
    /// Serializable tx has committed.
    commit_write_log: scc::TreeIndex<u64, Arc<CommitWriteRecord>>,
}

/// RAII guard that removes a snapshot from `active_snapshots` on drop.
pub struct SnapshotGuard {
    version: u64,
    snapshots: Arc<scc::HashMap<u64, ()>>,
}

impl SnapshotGuard {
    /// The snapshot version this guard protects.
    pub fn version(&self) -> u64 {
        self.version
    }
}

impl Drop for SnapshotGuard {
    fn drop(&mut self) {
        let _ = self.snapshots.remove(&self.version);
    }
}

impl RepoTxGate {
    /// Create a new gate, optionally seeded from durable markers read
    /// on repo open.
    pub fn new(last_committed: u64, next_tx_id_seed: u64) -> Self {
        Self {
            commit_mutex: tokio::sync::Mutex::new(()),
            version_counter: AtomicU64::new(last_committed),
            last_committed_version: AtomicU64::new(last_committed),
            next_tx_id: AtomicU64::new(next_tx_id_seed),
            active_snapshots: Arc::new(scc::HashMap::new()),
            commit_write_log: scc::TreeIndex::new(),
        }
    }

    /// Fresh gate for a new repo (no history).
    pub fn fresh() -> Self {
        Self::new(0, 1)
    }

    /// Allocate a unique tx-id. Lock-free.
    pub fn fresh_tx_id(&self) -> TxId {
        TxId::new(self.next_tx_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Current committed version visible to readers.
    pub fn last_committed(&self) -> u64 {
        self.last_committed_version.load(Ordering::Acquire)
    }

    /// cancel-safe: yes — single `scc::HashMap::insert_async` is CAS-
    /// based and either completes or leaves the map unchanged on
    /// cancellation. If the future is dropped after insertion but before
    /// returning the guard, the snapshot entry leaks (no Drop runs);
    /// since callers always await this to completion or never call it,
    /// in practice this is cancel-safe.
    ///
    /// Register a snapshot at the current `last_committed` version and
    /// return a RAII guard. On drop the snapshot is removed.
    pub async fn open_snapshot(&self) -> SnapshotGuard {
        let version = self.last_committed();
        let _ = self.active_snapshots.insert_async(version, ()).await;
        SnapshotGuard {
            version,
            snapshots: Arc::clone(&self.active_snapshots),
        }
    }

    /// cancel-safe: yes — single `tokio::sync::Mutex::lock().await`,
    /// which is documented cancel-safe (`drop` of the future releases
    /// the wait without acquiring the lock).
    ///
    /// Lock the commit gate. Returns a tokio `MutexGuard`. The guard is
    /// held across the entire critical section — pre-commit
    /// (interner-overlay merge / SSI validation / unique re-check / WAL
    /// begin) + data and index materialization + publish + durable
    /// markers + WAL cleanup — and dropped only after `materialize`, so
    /// the post-lock per-vector HNSW promote runs unserialised.
    pub async fn commit_lock(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.commit_mutex.lock().await
    }

    /// Allocate the next MVCC version. Called under `commit_lock`.
    pub fn assign_next_version(&self) -> u64 {
        self.version_counter.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Publish: update `last_committed_version` atomically.
    pub fn publish_committed(&self, version: u64) {
        self.last_committed_version
            .store(version, Ordering::Release);
    }

    /// Minimum alive snapshot version — for GC. Returns
    /// `last_committed()` if no snapshots are open.
    pub fn min_alive(&self) -> u64 {
        let mut min = u64::MAX;
        self.active_snapshots.scan(|k, _| {
            if *k < min {
                min = *k;
            }
        });
        if min == u64::MAX {
            self.last_committed()
        } else {
            min
        }
    }

    /// True if no transaction has an open snapshot.
    pub fn active_snapshots_empty(&self) -> bool {
        self.active_snapshots.is_empty()
    }

    /// Peek at the `next_tx_id` counter (for durable snapshot).
    pub fn peek_next_tx_id(&self) -> u64 {
        self.next_tx_id.load(Ordering::Relaxed)
    }

    // ── Phase C: commit-write log ────────────────────────────────────

    /// PROPOSED (Phase C). Append one footprint at publish time.
    ///
    /// Called by `commit_tx::materialize` UNDER `commit_lock`, immediately
    /// after `gate.publish_committed(commit_version)`. The caller already
    /// holds `commit_lock`, so the append is serialised against every other
    /// publish. We use `TreeIndex::insert` (CAS-based, lock-free).
    ///
    /// Zero-overhead invariant: NO-OP off Serializable AND no-op when the
    /// footprint has no touched tables. The caller MUST construct the
    /// footprint via `build_footprint_from_tx` which short-circuits to
    /// empty under Snapshot — but we re-gate here defensively, so this
    /// function is safe to call unconditionally.
    pub fn record_commit_writes(&self, rec: CommitWriteRecord) {
        if rec.per_table.is_empty() {
            return;
        }
        let _ = self
            .commit_write_log
            .insert(rec.commit_version, Arc::new(rec));
    }

    /// PROPOSED (Phase C). True iff some committer in the window
    /// `(snapshot, last_committed()]` wrote a footprint that conflicts
    /// with `dep`. Called by `pre_commit` Phase 2-bis UNDER `commit_lock`.
    ///
    /// Walks ONLY records with `commit_version > snapshot` thanks to
    /// `TreeIndex::range`. Uses `(snapshot + 1)..` to express the
    /// exclusive lower bound (snapshot is always far from `u64::MAX`
    /// in practice — versions are monotonic from a small seed).
    pub fn predicate_conflicts(
        &self,
        dep: &crate::predicate_set::PredicateDep,
        snapshot: u64,
    ) -> bool {
        let guard = scc::ebr::Guard::new();
        // `snapshot + 1` is safe: version 0 is the initial seed, and
        // a tx at snapshot u64::MAX cannot exist (versions are monotonic
        // from a small seed and would exhaust before reaching MAX).
        let start = snapshot.saturating_add(1);
        for (_v, rec) in self.commit_write_log.range(start.., &guard) {
            if record_conflicts(rec, dep) {
                return true;
            }
        }
        false
    }

    /// PROPOSED (Phase C). Drop records with `commit_version <= floor`.
    ///
    /// Called by the same GC tick that prunes `MvccStore::version_cache`.
    /// The caller MUST pass `gate.min_alive()` — never anything higher.
    pub fn prune_commit_log_below(&self, floor: u64) -> usize {
        // Use `remove_range` for efficiency.
        let guard = scc::ebr::Guard::new();
        let count = self.commit_write_log.range(..=floor, &guard).count();
        self.commit_write_log.remove_range(..=floor);
        count
    }

    /// Number of records currently in the commit-write log.
    /// Useful for telemetry and engine-side tests (GC prune assertions).
    /// Not on any hot path — walks the tree under an EBR guard.
    pub fn commit_log_len(&self) -> usize {
        let guard = scc::ebr::Guard::new();
        self.commit_write_log.range::<u64, _>(.., &guard).count()
    }
}

/// PROPOSED (Phase C). Conflict-check for ONE committed record.
/// Free function so it stays testable without a `RepoTxGate` instance.
fn record_conflicts(rec: &CommitWriteRecord, dep: &crate::predicate_set::PredicateDep) -> bool {
    match dep {
        crate::predicate_set::PredicateDep::TableScan { table_token } => {
            rec.per_table.get(table_token).is_some_and(|f| f.touched)
        }
        crate::predicate_set::PredicateDep::IndexRange {
            table_token,
            index_id,
            lo,
            hi,
        } => rec.per_table.get(table_token).is_some_and(|f| {
            f.inserted_index_keys
                .iter()
                .any(|k| crate::predicate_set::key_in_interval(k, *index_id, lo, hi))
        }),
    }
}

/// PROPOSED (Phase C). Project the on-commit `TxContext` state into a
/// `CommitWriteRecord`. Zero-cost off Serializable: returns an empty
/// record whose `per_table` map is empty (no allocations except the
/// stack-resident empty `HashMap`), and the caller's `record_commit_writes`
/// no-ops on it.
///
/// Called by `commit_tx::materialize` AT publish time.
/// Inputs already collected by the existing pipeline:
///   - `tx.index_write_set` (tx_context.rs:72) — SetPosting keys per table.
///   - `tx.write_set`       (tx_context.rs:67) — touched tables (data).
///   - `tx.counter_deltas`  (tx_context.rs:93) — touched tables (counter).
pub fn build_footprint_from_tx(tx: &crate::TxContext, commit_version: u64) -> CommitWriteRecord {
    let mut rec = CommitWriteRecord {
        commit_version,
        per_table: HashMap::new(),
    };
    if tx.isolation != crate::IsolationLevel::Serializable {
        // Zero-overhead: Snapshot/non-tx publishes nothing.
        return rec;
    }

    // Touched-bit: any table that ANY non-empty staging touched.
    for token in tx.write_set.keys() {
        rec.per_table.entry(*token).or_default().touched = true;
    }
    for token in tx.counter_deltas.keys() {
        rec.per_table.entry(*token).or_default().touched = true;
    }

    // Precise index-posting keys (only SetPosting; RemovePosting does not
    // introduce a phantom in a predicate — coarse `touched` covers it).
    for (token, op) in &tx.index_write_set {
        // Index ops always touch their table even if SetPosting/Remove split.
        rec.per_table.entry(*token).or_default().touched = true;
        if let crate::IndexWriteOp::SetPosting { key, .. } = op {
            rec.per_table
                .entry(*token)
                .or_default()
                .inserted_index_keys
                .push(key.clone());
        }
    }

    // Sort each table's keys ascending — frees a future binary_search
    // optimisation (doc section 5.4) and makes test assertions stable.
    for f in rec.per_table.values_mut() {
        f.inserted_index_keys.sort_unstable();
    }

    rec
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut versions = std::collections::HashSet::new();
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
        let mut ids = std::collections::HashSet::new();
        for h in handles {
            let id = h.await.unwrap();
            assert!(ids.insert(id.raw()), "duplicate tx_id");
        }
        assert_eq!(ids.len(), n);
    }

    // ── Phase C: commit-write log tests ──────────────────────────────

    #[test]
    fn commit_log_window_scan_excludes_at_or_below_snapshot() {
        use crate::predicate_set::PredicateDep;
        let gate = RepoTxGate::new(0, 1);
        let mk = |v: u64, token: u64| CommitWriteRecord {
            commit_version: v,
            per_table: HashMap::from([(
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

        // snapshot=10 -> only records with v>10 (i.e. v=15) are visible.
        assert!(gate.predicate_conflicts(&PredicateDep::TableScan { table_token: 42 }, 10));
        // snapshot=15 -> window is empty.
        assert!(!gate.predicate_conflicts(&PredicateDep::TableScan { table_token: 42 }, 15));
        // Disjoint table -> no conflict at any snapshot.
        assert!(!gate.predicate_conflicts(&PredicateDep::TableScan { table_token: 99 }, 0));
    }

    #[test]
    fn commit_log_index_range_intersects_posting_key() {
        use crate::predicate_set::{PredicateDep, SORTED_PREFIX_LEN, SORTED_TAG};
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
            per_table: HashMap::from([(
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
            per_table: HashMap::from([(
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
                per_table: HashMap::from([(
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
            per_table: HashMap::from([(
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
        assert_eq!(gate.commit_log_len(), 5);

        // Prune with floor=3: entries 1,2,3 dropped, 4,5 remain.
        let removed = gate.prune_commit_log_below(3);
        assert_eq!(removed, 3);
        assert_eq!(gate.commit_log_len(), 2);

        // Verify 4 and 5 survive: predicate_conflicts at snapshot=3 sees v>3.
        use crate::predicate_set::PredicateDep;
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
            per_table: HashMap::from([(
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
        use crate::types::{IsolationLevel, TxId};
        let tx = crate::TxContext::new(TxId::new(1), 0, 10, IsolationLevel::Snapshot);
        let rec = crate::repo_tx_gate::build_footprint_from_tx(&tx, 99);
        assert!(rec.is_empty(), "Snapshot must produce empty footprint");

        // And `record_commit_writes` on an empty record is a no-op.
        let gate = RepoTxGate::fresh();
        gate.record_commit_writes(rec);
        assert_eq!(gate.commit_log_len(), 0);
    }

    #[test]
    fn build_footprint_projects_index_set_postings_only() {
        use crate::types::{IsolationLevel, TxId};
        use crate::IndexWriteOp;
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

        let rec = crate::repo_tx_gate::build_footprint_from_tx(&tx, 11);
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
                    per_table: HashMap::from([(
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
}
