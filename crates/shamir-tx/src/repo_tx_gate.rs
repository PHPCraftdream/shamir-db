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

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::TxId;

/// Per-repo transactional synchronisation point.
pub struct RepoTxGate {
    /// Serialises the commit phase. Only held for the ~5ms critical
    /// section (assign version → validate → write → publish).
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

    /// Lock the commit gate. Returns a tokio `MutexGuard`.
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
}
