//! Versioned KV layer over two dumb-KV stores (main + history).
//!
//! [`MvccStore`] wraps two [`Store`] instances:
//! - `main` — the current version of every key (identical to today's
//!   non-tx writes).
//! - `history` — old versions stored under `<key>::0xFF::<version_be>`
//!   keys (see [`version_codec`]).
//!
//! Zero-overhead when no transaction is active: [`MvccStore::set_versioned`]
//! checks `gate.active_snapshots_empty()` and skips history archival
//! entirely, writing directly to `main`.
//!
//! Snapshot reads via [`MvccStore::get_at`] use a fast path (version cache
//! check → main read) and fall back to a history range scan.

use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use scc::HashMap as SccHashMap;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::KvOp;
use shamir_storage::types::Store;

use crate::repo_tx_gate::RepoTxGate;
use crate::version_codec::encode_version_key;

/// Versioned layer over two dumb-KV stores.
///
/// See [module-level documentation](self) for the design rationale.
pub struct MvccStore {
    main: Arc<dyn Store>,
    history: Arc<dyn Store>,
    gate: Arc<RepoTxGate>,
    /// In-memory cache: key → latest committed version.
    /// Cold start: first `get_at` for a key does a range scan, populates cache.
    version_cache: SccHashMap<Bytes, u64>,
}

impl MvccStore {
    /// Create a new MVCC store from two backing stores and a gate.
    pub fn new(main: Arc<dyn Store>, history: Arc<dyn Store>, gate: Arc<RepoTxGate>) -> Self {
        Self {
            main,
            history,
            gate,
            version_cache: SccHashMap::new(),
        }
    }

    /// cancel-safe: NO — multi-step state mutation. When snapshots are
    /// active the sequence is: read old from `main`, archive to `history`,
    /// write new to `main`, allocate version and update `version_cache`.
    /// Cancellation between archive and main-write can leave history
    /// containing a value while main still has the old one (or a stale
    /// version_cache). Recovery is by caller-side retry / WAL replay.
    ///
    /// Non-tx write. If active snapshots exist, saves the old value
    /// in history before overwriting main.
    pub async fn set_versioned(&self, key: Bytes, value: Bytes) -> DbResult<()> {
        if self.gate.active_snapshots_empty() {
            self.main.set(key, value).await?;
            return Ok(());
        }
        // Archive old value if it exists.
        if let Ok(old) = self.main.get(key.clone()).await {
            let cur_v = self.current_version(&key);
            let h_key = encode_version_key(&key, cur_v);
            self.history.set(h_key, old).await?;
        }
        self.main.set(key.clone(), value).await?;
        let new_v = self.gate.assign_next_version();
        // CRIT-2: `entry().insert_entry()` is a NO-OP on Occupied —
        // it returns the existing entry unchanged. Use `upsert_async`
        // so repeated writes to the same key advance the cached
        // version monotonically.
        self.version_cache.upsert_async(key, new_v).await;
        Ok(())
    }

    /// cancel-safe: NO — multi-step state mutation; same reasoning as
    /// `set_versioned`. Sequence is archive-old → remove from main →
    /// allocate version and update `version_cache`. Cancellation mid-
    /// sequence leaves the store in a partial state; caller-side retry
    /// / WAL replay is the recovery path.
    ///
    /// Non-tx delete. Similar to `set_versioned` — archives old value
    /// if snapshots are active.
    pub async fn delete_versioned(&self, key: Bytes) -> DbResult<()> {
        if !self.gate.active_snapshots_empty() {
            if let Ok(old) = self.main.get(key.clone()).await {
                let cur_v = self.current_version(&key);
                let h_key = encode_version_key(&key, cur_v);
                self.history.set(h_key, old).await?;
            }
        }
        let _ = self.main.remove(key.clone()).await;
        if !self.gate.active_snapshots_empty() {
            let new_v = self.gate.assign_next_version();
            // CRIT-2: see `set_versioned` for rationale.
            self.version_cache.upsert_async(key, new_v).await;
        }
        Ok(())
    }

    /// cancel-safe: yes — read-only. Fast path is a single `main.get`;
    /// slow path is a read-only history range scan. Cancellation drops
    /// the future with no state mutation.
    ///
    /// Snapshot read: return the value visible at `snapshot_version`.
    ///
    /// Fast path: if version_cache says current version ≤ snapshot →
    /// return `main.get(key)`.
    /// Slow path: range scan history `[key::0, key::snapshot]`, take last.
    pub async fn get_at(&self, key: &[u8], snapshot_version: u64) -> DbResult<Option<Bytes>> {
        let cur_v = self.current_version(key);
        if cur_v <= snapshot_version {
            return match self.main.get(Bytes::copy_from_slice(key)).await {
                Ok(v) => Ok(Some(v)),
                Err(DbError::NotFound(_)) => Ok(None),
                Err(e) => Err(e),
            };
        }
        self.scan_history_for_version(key, snapshot_version).await
    }

    /// Direct access to main store (for non-tx reads).
    pub fn main_store(&self) -> &Arc<dyn Store> {
        &self.main
    }

    /// Direct access to history store (for GC, recovery).
    pub fn history_store(&self) -> &Arc<dyn Store> {
        &self.history
    }

    /// Look up the latest committed version for `key` in the in-memory cache.
    /// Returns 0 if the key is not cached (meaning "initial / no version tracked").
    fn current_version(&self, key: &[u8]) -> u64 {
        let key_bytes = Bytes::copy_from_slice(key);
        self.version_cache.read(&key_bytes, |_, v| *v).unwrap_or(0)
    }

    /// Public accessor: current committed version for `key`, or `0` if
    /// the key has never been written through this store.
    ///
    /// Used by SSI read-set validation (Stage 4.D.5+) — the caller
    /// captures this value when reading inside a tx, then commit re-
    /// queries it to detect "another tx wrote this key since I read".
    pub fn version_of(&self, key: &[u8]) -> u64 {
        self.current_version(key)
    }

    /// cancel-safe: yes — a single `version_cache.upsert_async`, which is
    /// CAS-based and either lands or leaves the map unchanged on cancel.
    ///
    /// Seed the in-memory version cache for a recovered key.
    ///
    /// V2 WAL recovery (`crate`-external; see
    /// `shamir_engine::tx::recovery`) replays a committed tx by writing
    /// the record body directly to the backing `main` store, bypassing
    /// [`apply_committed_ops`]. That keeps `main` correct but leaves
    /// `version_cache` empty, so a later `get_at(key, snap)` for a
    /// snapshot *below* `commit_version` would take the fast path
    /// (`current_version == 0 ≤ snap`) and return the recovered (latest)
    /// value instead of scanning history.
    ///
    /// In the bootstrap-recovery scenario this is harmless (no snapshot
    /// survives a restart and every fresh snapshot opens at
    /// `≥ last_committed ≥ commit_version`), but seeding the cache keeps
    /// `version_of`/`get_at` consistent for any post-recovery reader and
    /// for SSI conflict detection if the recovered key is immediately
    /// re-written inside a new transaction.
    ///
    /// `upsert_async` (not `insert`) so a re-replay of the same key
    /// advances monotonically rather than silently keeping a stale value.
    pub async fn seed_version(&self, key: Bytes, version: u64) {
        self.version_cache.upsert_async(key, version).await;
    }

    /// cancel-safe: NO — applies a batch of `KvOp` via multi-step
    /// sequences (Phase 1 pre-reads, Phase 2 batched history transact,
    /// Phase 3 batched main transact, Phase 4 version_cache updates).
    /// Cancellation mid-batch leaves some phases applied, others not.
    /// Recovery relies on WAL replay (commit_tx invariant).
    pub async fn apply_committed_ops(&self, ops: Vec<KvOp>, commit_version: u64) -> DbResult<()> {
        // HIGH-2: re-sample `snapshots_active` per-op inside the loop
        // rather than once at function entry. A snapshot opening
        // mid-apply must still see archived old values for ops that
        // run after the snapshot's gate-insertion happens. Sampling
        // per-op keeps the existing "no snapshots ⇒ no archive"
        // contract without races for late-arriving snapshots.
        //
        // HIGH-3: batch the physical writes through `Store::transact`.
        // Per-op `set`/`remove` collapses to a single atomic write-tx
        // on backends that override `transact` (redb, sled, fjall,
        // persy, nebari, canopy) — one fsync instead of N.

        // Phase 1: pre-read old values for archive. Per-op snapshot
        // check + per-op `main.get` (can't batch — depends on per-key
        // old value).
        let mut history_ops: Vec<KvOp> = Vec::new();
        for op in &ops {
            // Re-sample per iteration so a snapshot that opens
            // mid-loop is honoured by the remaining ops.
            if self.gate.active_snapshots_empty() {
                continue;
            }
            let key = match op {
                KvOp::Set(k, _) => k,
                KvOp::Remove(k) => k,
            };
            if let Ok(old) = self.main.get(key.clone()).await {
                let cur_v = self.current_version(key);
                let h_key = encode_version_key(key, cur_v);
                history_ops.push(KvOp::Set(h_key, old));
            }
        }

        // Phase 2: one batched write to history.
        if !history_ops.is_empty() {
            self.history.transact(history_ops).await?;
        }

        // Phase 3: one batched write to main.
        self.main.transact(ops.clone()).await?;

        // Phase 4: update the in-memory version cache for every
        // touched key. Uses `upsert_async` (CRIT-2): `entry().insert_entry()`
        // is a NO-OP when the key already exists, so repeated writes
        // to the same key silently kept the FIRST commit_version —
        // breaking SSI conflict detection. `upsert_async` overwrites.
        for op in ops {
            let key = match op {
                KvOp::Set(k, _) => k,
                KvOp::Remove(k) => k,
            };
            self.version_cache.upsert_async(key, commit_version).await;
        }
        Ok(())
    }

    /// cancel-safe: NO — Phase 1 scans the history stream; Phase 2
    /// deletes per-key residuals. Cancellation during Phase 2 leaves
    /// some entries deleted and others not. GC is idempotent — a later
    /// `gc_below` resumes from current history state — so eventual
    /// convergence is fine, but a single call is not atomic.
    ///
    /// Garbage-collect history entries with version < `min_version`.
    ///
    /// For each original key, keeps the LATEST version that is still
    /// < `min_version` (the "anchor" — needed so `get_at(snapshot)`
    /// can still find it for snapshots between anchor and min_version).
    /// All older versions of that key are removed.
    ///
    /// Returns the number of history entries deleted.
    pub async fn gc_below(&self, min_version: u64) -> DbResult<usize> {
        use crate::version_codec::decode_version_key;

        // Phase 1: scan all history entries, group by original key.
        let stream = self.history.iter_stream(256);
        futures::pin_mut!(stream);

        // Collect: original_key → Vec<(version, physical_key)>
        let mut per_key: std::collections::HashMap<Vec<u8>, Vec<(u64, Bytes)>> =
            std::collections::HashMap::new();

        while let Some(batch) = stream.next().await {
            for (phys_key, _value) in batch? {
                if let Some((orig, version)) = decode_version_key(&phys_key) {
                    if version < min_version {
                        per_key
                            .entry(orig.to_vec())
                            .or_default()
                            .push((version, phys_key));
                    }
                }
            }
        }

        // Phase 2: for each key, sort by version, keep the latest (anchor),
        // delete the rest.
        let mut deleted = 0usize;
        for (_orig_key, mut entries) in per_key {
            if entries.len() <= 1 {
                // Only one entry — it's the anchor, keep it.
                continue;
            }
            entries.sort_by_key(|(v, _)| *v);
            // Keep the last (highest version < min_version), delete the rest.
            let to_delete = &entries[..entries.len() - 1];
            for (_, phys_key) in to_delete {
                let _ = self.history.remove(phys_key.clone()).await;
                deleted += 1;
            }
        }

        Ok(deleted)
    }

    /// cancel-safe: NO — delegates to `gc_below`, which is non-cancel-
    /// safe. Idempotent on retry.
    ///
    /// Run GC using the gate's `min_alive()` as the threshold.
    pub async fn gc(&self) -> DbResult<usize> {
        let min = self.gate.min_alive();
        self.gc_below(min).await
    }

    /// Slow path: range scan history for the latest version ≤ `snapshot`.
    async fn scan_history_for_version(&self, key: &[u8], snapshot: u64) -> DbResult<Option<Bytes>> {
        let lo = encode_version_key(key, 0);
        let hi = encode_version_key(key, snapshot);
        let stream = self.history.iter_range_stream(Some(lo), Some(hi), 64);
        futures::pin_mut!(stream);
        let mut latest: Option<Bytes> = None;
        while let Some(batch) = stream.next().await {
            for (_, val) in batch? {
                latest = Some(val);
            }
        }
        Ok(latest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shamir_storage::storage_in_memory::InMemoryStore;

    fn make_gate() -> Arc<RepoTxGate> {
        Arc::new(RepoTxGate::fresh())
    }

    fn make_mvcc() -> MvccStore {
        let gate = make_gate();
        MvccStore::new(
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryStore::new()),
            gate,
        )
    }

    fn make_mvcc_with_gate(gate: Arc<RepoTxGate>) -> MvccStore {
        MvccStore::new(
            Arc::new(InMemoryStore::new()),
            Arc::new(InMemoryStore::new()),
            gate,
        )
    }

    #[tokio::test]
    async fn set_without_snapshots_skips_history() {
        let mvcc = make_mvcc();
        let key = Bytes::from("k1");
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();

        // main has the value
        let val = mvcc.main.get(key.clone()).await.unwrap();
        assert_eq!(val, Bytes::from("v1"));

        // history is empty
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            let batch = batch.unwrap();
            assert!(batch.is_empty(), "history should be empty");
        }
    }

    #[tokio::test]
    async fn set_with_snapshot_archives_old_value() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        // Open a snapshot so active_snapshots is non-empty.
        let _guard = gate.open_snapshot().await;

        let key = Bytes::from("k1");
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        mvcc.set_versioned(key.clone(), Bytes::from("v2"))
            .await
            .unwrap();

        // main has v2
        let val = mvcc.main.get(key.clone()).await.unwrap();
        assert_eq!(val, Bytes::from("v2"));

        // history contains v1 under a version key
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut found = false;
        while let Some(batch) = stream.next().await {
            for (hk, hv) in batch.unwrap() {
                let (orig_key, _ver) = crate::version_codec::decode_version_key(&hk).unwrap();
                assert_eq!(orig_key, &b"k1"[..]);
                assert_eq!(hv, Bytes::from("v1"));
                found = true;
            }
        }
        assert!(found, "history should contain archived v1");
    }

    #[tokio::test]
    async fn get_at_current_version() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        let _guard = gate.open_snapshot().await;
        let key = Bytes::from("k1");
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();

        let v = gate.assign_next_version();
        // snapshot_version >> v → fast path returns from main
        let result = mvcc.get_at(b"k1", v + 100).await.unwrap();
        assert_eq!(result, Some(Bytes::from("v1")));
    }

    #[tokio::test]
    async fn get_at_old_version() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        let _guard = gate.open_snapshot().await;
        let key = Bytes::from("k1");

        // v1 written at version 1
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        // v2 written at version 2
        mvcc.set_versioned(key.clone(), Bytes::from("v2"))
            .await
            .unwrap();

        // Query at snapshot between v1 and v2 — should find v1 in history.
        // After two set_versioned calls, version_cache[k1] = 2.
        // get_at(snapshot=1) → cur_v(2) > 1 → slow path → scan history.
        let result = mvcc.get_at(b"k1", 1).await.unwrap();
        assert_eq!(result, Some(Bytes::from("v1")));
    }

    #[tokio::test]
    async fn get_at_missing_key() {
        let mvcc = make_mvcc();
        let result = mvcc.get_at(b"nonexistent", 999).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn delete_versioned_archives() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        let _guard = gate.open_snapshot().await;
        let key = Bytes::from("k1");

        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        mvcc.delete_versioned(key.clone()).await.unwrap();

        // main no longer has the key
        assert!(mvcc.main.get(key).await.is_err());

        // history contains v1
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut found = false;
        while let Some(batch) = stream.next().await {
            for (_, hv) in batch.unwrap() {
                assert_eq!(hv, Bytes::from("v1"));
                found = true;
            }
        }
        assert!(found, "history should contain archived v1 after delete");
    }

    #[tokio::test]
    async fn get_at_after_delete() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        let _guard = gate.open_snapshot().await;
        let key = Bytes::from("k1");

        // v1 at version 1
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        // delete at version 2
        mvcc.delete_versioned(key.clone()).await.unwrap();

        // get_at between v1 and delete → slow path → v1 from history
        let result = mvcc.get_at(b"k1", 1).await.unwrap();
        assert_eq!(result, Some(Bytes::from("v1")));

        // get_at after delete → fast path (cur_v=2 <= 15) → main.get → NotFound → None
        let result = mvcc.get_at(b"k1", 15).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn zero_overhead_no_snapshots() {
        let mvcc = make_mvcc();
        for i in 0..100u32 {
            let key = Bytes::copy_from_slice(&i.to_be_bytes());
            mvcc.set_versioned(key, Bytes::from("val")).await.unwrap();
        }

        // history should be empty — no snapshots were open
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut count = 0usize;
        while let Some(batch) = stream.next().await {
            count += batch.unwrap().len();
        }
        assert_eq!(
            count, 0,
            "history should have zero records without snapshots"
        );
    }

    #[tokio::test]
    async fn version_cache_populated_on_set() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        let _guard = gate.open_snapshot().await;
        let key = Bytes::from("k1");
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();

        let cached = mvcc.version_cache.read(&key, |_, v| *v);
        assert!(
            cached.is_some(),
            "version_cache should contain key after set"
        );
        assert!(cached.unwrap() > 0, "version should be > 0");
    }

    #[tokio::test]
    async fn main_store_accessor() {
        let main: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let gate = make_gate();

        let mvcc = MvccStore::new(Arc::clone(&main), Arc::clone(&history), gate);

        // Verify that main_store() returns the same Arc
        assert!(Arc::ptr_eq(&main, mvcc.main_store()));
        assert!(Arc::ptr_eq(&history, mvcc.history_store()));
    }

    #[tokio::test]
    async fn version_of_returns_zero_for_unknown_key() {
        let mvcc = make_mvcc();
        let v = mvcc.version_of(b"never_written");
        assert_eq!(v, 0);
    }

    #[tokio::test]
    async fn version_of_returns_cached_version_after_versioned_set() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let _guard = gate.open_snapshot().await;
        let key = Bytes::from("kx");
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        let v = mvcc.version_of(&key);
        assert!(v > 0, "version_of must reflect the assigned version");
    }

    #[tokio::test]
    async fn apply_committed_ops_updates_version_cache() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let _guard = gate.open_snapshot().await;

        let key = Bytes::from("k_commit");
        let ops = vec![KvOp::Set(key.clone(), Bytes::from("val"))];
        mvcc.apply_committed_ops(ops, 42).await.unwrap();

        assert_eq!(mvcc.version_of(&key), 42);

        let val = mvcc.main.get(key).await.unwrap();
        assert_eq!(val, Bytes::from("val"));
    }

    #[tokio::test]
    async fn apply_committed_ops_archives_old_value() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let _guard = gate.open_snapshot().await;

        let key = Bytes::from("k_archive");
        mvcc.main
            .set(key.clone(), Bytes::from("old"))
            .await
            .unwrap();

        let ops = vec![KvOp::Set(key.clone(), Bytes::from("new"))];
        mvcc.apply_committed_ops(ops, 10).await.unwrap();

        assert_eq!(
            mvcc.main.get(key.clone()).await.unwrap(),
            Bytes::from("new")
        );

        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut found = false;
        while let Some(batch) = stream.next().await {
            for (_, hv) in batch.unwrap() {
                assert_eq!(hv, Bytes::from("old"));
                found = true;
            }
        }
        assert!(found, "old value must be archived in history");
    }

    #[tokio::test]
    async fn apply_committed_ops_remove_archives_and_deletes() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let _guard = gate.open_snapshot().await;

        let key = Bytes::from("k_del");
        mvcc.main
            .set(key.clone(), Bytes::from("before"))
            .await
            .unwrap();

        let ops = vec![KvOp::Remove(key.clone())];
        mvcc.apply_committed_ops(ops, 20).await.unwrap();

        assert!(mvcc.main.get(key.clone()).await.is_err());

        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut found = false;
        while let Some(batch) = stream.next().await {
            for (_, hv) in batch.unwrap() {
                assert_eq!(hv, Bytes::from("before"));
                found = true;
            }
        }
        assert!(found);
    }

    #[tokio::test]
    async fn get_at_busy_history_five_versions() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let _guard = gate.open_snapshot().await;

        let key = Bytes::from("busy");
        let mut version_at = Vec::new();

        for i in 1..=5u32 {
            mvcc.set_versioned(key.clone(), Bytes::from(format!("v{i}")))
                .await
                .unwrap();
            let v = mvcc.version_of(&key);
            version_at.push(v);
        }

        // Query at each historical version and verify we get the right value.
        // version_at[0] was assigned when v1 was written, so get_at(version_at[0]) → v1
        for (idx, &snap) in version_at.iter().enumerate() {
            let result = mvcc.get_at(key.as_ref(), snap).await.unwrap();
            let expected = format!("v{}", idx + 1);
            assert_eq!(
                result,
                Some(Bytes::from(expected.clone())),
                "at snapshot {} expected {}",
                snap,
                expected
            );
        }

        // Query at version 0 (before any write) → slow path → scan [key::0, key::0] → empty → None
        let result_before = mvcc.get_at(key.as_ref(), 0).await.unwrap();
        assert!(
            result_before.is_none(),
            "no value should exist before first write"
        );

        // Query at a very high version → fast path → current (v5)
        let result_latest = mvcc.get_at(key.as_ref(), u64::MAX - 1).await.unwrap();
        assert_eq!(result_latest, Some(Bytes::from("v5")));
    }

    async fn count_history_entries(mvcc: &MvccStore) -> usize {
        let stream = mvcc.history_store().iter_stream(64);
        futures::pin_mut!(stream);
        let mut count = 0;
        while let Some(batch) = stream.next().await {
            count += batch.unwrap().len();
        }
        count
    }

    #[tokio::test]
    async fn gc_below_removes_old_versions_keeps_anchor() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let _guard = gate.open_snapshot().await;

        let key = Bytes::from("gc_test");
        // Write 5 versions
        for i in 1..=5u32 {
            mvcc.set_versioned(key.clone(), Bytes::from(format!("v{i}")))
                .await
                .unwrap();
        }

        // History should have 4 entries (v1..v4 archived when v2..v5 overwrote)
        let count_before = count_history_entries(&mvcc).await;
        assert_eq!(count_before, 4, "should have 4 history entries");

        // GC below version 3: versions < 3 in history are version_at[0] and version_at[1].
        // Anchor = highest < 3, older one deleted.
        let deleted = mvcc.gc_below(3).await.unwrap();
        assert!(deleted >= 1, "should delete at least 1 old entry");

        let count_after = count_history_entries(&mvcc).await;
        assert!(count_after < count_before, "history should shrink");
    }

    #[tokio::test]
    async fn gc_below_zero_deletes_nothing() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let _guard = gate.open_snapshot().await;

        let key = Bytes::from("gc_noop");
        mvcc.set_versioned(key.clone(), Bytes::from("v1"))
            .await
            .unwrap();
        mvcc.set_versioned(key.clone(), Bytes::from("v2"))
            .await
            .unwrap();

        let deleted = mvcc.gc_below(0).await.unwrap();
        assert_eq!(deleted, 0);
    }

    #[tokio::test]
    async fn gc_convenience_uses_min_alive() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        // No snapshots open → min_alive = last_committed = 0
        // Write some data without snapshot (no history archived)
        mvcc.set_versioned(Bytes::from("k"), Bytes::from("v"))
            .await
            .unwrap();

        let deleted = mvcc.gc().await.unwrap();
        assert_eq!(deleted, 0, "nothing to GC when no history");
    }

    #[tokio::test]
    async fn apply_committed_ops_no_snapshots_skips_history() {
        let mvcc = make_mvcc();

        let key = Bytes::from("k_nohist");
        mvcc.main
            .set(key.clone(), Bytes::from("old"))
            .await
            .unwrap();

        let ops = vec![KvOp::Set(key.clone(), Bytes::from("new"))];
        mvcc.apply_committed_ops(ops, 5).await.unwrap();

        assert_eq!(
            mvcc.main.get(key.clone()).await.unwrap(),
            Bytes::from("new")
        );

        assert_eq!(mvcc.version_of(&key), 5);

        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut count = 0usize;
        while let Some(batch) = stream.next().await {
            count += batch.unwrap().len();
        }
        assert_eq!(
            count, 0,
            "history should stay empty without active snapshots"
        );
    }

    /// CRIT-2 regression test. Before the fix, `version_cache.entry()
    /// .insert_entry()` was a no-op when the key was already cached,
    /// so the second `apply_committed_ops(..., 200)` left the cached
    /// version stuck at 100 and SSI conflict detection silently
    /// failed.
    #[tokio::test]
    async fn version_cache_updates_on_repeated_writes_to_same_key() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());
        let _guard = gate.open_snapshot().await;
        let key = Bytes::from("repeated");

        mvcc.apply_committed_ops(vec![KvOp::Set(key.clone(), Bytes::from("v1"))], 100)
            .await
            .unwrap();
        assert_eq!(mvcc.version_of(&key), 100);

        mvcc.apply_committed_ops(vec![KvOp::Set(key.clone(), Bytes::from("v2"))], 200)
            .await
            .unwrap();
        // CRITICAL: must be 200, was 100 before the fix.
        assert_eq!(
            mvcc.version_of(&key),
            200,
            "version_cache must update on repeated writes"
        );
    }

    /// HIGH-2 regression guard. Before the fix, `snapshots_active`
    /// was sampled once at function entry; a snapshot that opened
    /// after the sample but before the first op would silently miss
    /// the archive. Per-op re-sampling closes that race for every
    /// op processed after the snapshot becomes visible.
    #[tokio::test]
    async fn apply_committed_ops_archives_even_if_snapshot_opens_mid_call() {
        let gate = make_gate();
        let mvcc = make_mvcc_with_gate(gate.clone());

        // No snapshots open yet — seed an old value through the raw
        // main store so apply will see it for archival.
        mvcc.main
            .set(Bytes::from("k"), Bytes::from("old"))
            .await
            .unwrap();

        // Open a snapshot right before apply (simulates "opened just
        // before, but after the flag would have been sampled at
        // function entry").
        let _g = gate.open_snapshot().await;

        mvcc.apply_committed_ops(vec![KvOp::Set(Bytes::from("k"), Bytes::from("new"))], 50)
            .await
            .unwrap();

        // History must contain "old" because the snapshot needs it.
        let stream = mvcc.history.iter_stream(64);
        futures::pin_mut!(stream);
        let mut found = false;
        while let Some(batch) = stream.next().await {
            for (_, hv) in batch.unwrap() {
                if hv.as_ref() == b"old" {
                    found = true;
                }
            }
        }
        assert!(found, "old value archived for active snapshot");
    }
}
