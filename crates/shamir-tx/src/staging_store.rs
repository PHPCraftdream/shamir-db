//! In-memory write buffer for a single transaction.
//!
//! All writes go into a local `scc::HashMap`. Reads check the local
//! buffer first (serving staged writes / staged removes), then fall
//! through to the base `Store`.
//!
//! On commit: `drain()` returns `Vec<KvOp>` for an atomic
//! `base.transact(ops)` call. On abort: just drop the `StagingStore`.

use bytes::Bytes;
use scc::HashMap;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{KvOp, RecordKey, Store};
use std::sync::Arc;

#[derive(Debug, Clone)]
enum StagedOp {
    Set(Bytes),
    Remove,
}

/// Per-transaction staging buffer with read-through semantics.
///
/// Created at tx begin, consumed at commit (via `drain`), or dropped
/// at abort. NOT `Clone` — ownership is single (the `TxContext`).
pub struct StagingStore {
    base: Arc<dyn Store>,
    writes: HashMap<RecordKey, StagedOp>,
}

impl StagingStore {
    pub fn new(base: Arc<dyn Store>) -> Self {
        Self {
            base,
            writes: HashMap::new(),
        }
    }

    /// Borrow the base store this staging buffer wraps.
    ///
    /// Used by `commit_tx` Phase 5 to apply drained ops via
    /// `base.transact(ops)` — atomic batch publish per table.
    pub fn base(&self) -> &Arc<dyn Store> {
        &self.base
    }

    /// Read-through: staged value first, then base store.
    /// Staged `Remove` returns `NotFound` even if base has the key.
    ///
    /// cancel-safe: yes — single `.await` per branch; `scc::HashMap::read_async`
    /// is cancel-safe (no partial state on drop) and `base.get` may not be,
    /// but cancellation there leaves no local state modified.
    pub async fn get(&self, k: RecordKey) -> DbResult<Bytes> {
        if let Some(op) = self.writes.read_async(&k, |_, v| v.clone()).await {
            return match op {
                StagedOp::Set(b) => Ok(b),
                StagedOp::Remove => Err(DbError::NotFound(format!("staged remove: {:?}", k))),
            };
        }
        self.base.get(k).await
    }

    /// Stage a set (creates or overwrites).
    ///
    /// cancel-safe: yes — `upsert_async` either completes the upsert or leaves
    /// the map unchanged on cancellation (CAS-based, no partial state).
    pub async fn set(&self, k: RecordKey, v: Bytes) {
        let _ = self.writes.upsert_async(k, StagedOp::Set(v)).await;
    }

    /// Stage a remove.
    ///
    /// cancel-safe: yes — same reasoning as `set`.
    pub async fn remove(&self, k: RecordKey) {
        let _ = self.writes.upsert_async(k, StagedOp::Remove).await;
    }

    /// Snapshot of all staged ops without consuming.
    ///
    /// Used by `commit_tx` Phase 4 to emit data ops into the WAL
    /// entry, separate from Phase 5's `drain()` that actually applies
    /// them. Must be called under `RepoTxGate::commit_lock` — caller
    /// guarantees no concurrent writers.
    pub fn snapshot_ops(&self) -> Vec<KvOp> {
        let mut ops = Vec::new();
        self.writes.scan(|k, v| match v {
            StagedOp::Set(bytes) => ops.push(KvOp::Set(k.clone(), bytes.clone())),
            StagedOp::Remove => ops.push(KvOp::Remove(k.clone())),
        });
        ops
    }

    /// Drain all staged writes into a `Vec<KvOp>` suitable for
    /// `Store::transact`. Consumes `self`.
    ///
    /// The caller (TxContext commit phase) combines ops from all
    /// per-table StagingStores and feeds them to a single
    /// `store.transact(all_ops)` for atomic publish.
    pub fn drain(self) -> Vec<KvOp> {
        let mut ops = Vec::new();
        // scc::HashMap::scan is synchronous — closure receives (&K, &V).
        self.writes.scan(|k, v| match v {
            StagedOp::Set(bytes) => ops.push(KvOp::Set(k.clone(), bytes.clone())),
            StagedOp::Remove => ops.push(KvOp::Remove(k.clone())),
        });
        ops
    }

    /// Number of unique keys with staged writes.
    pub fn len(&self) -> usize {
        self.writes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.writes.is_empty()
    }

    /// cancel-safe: NO — iterates the staged keys (snapshot via
    /// `scan_async`) then invokes `update_async` per key. Cancellation
    /// mid-iteration leaves some staged values rewritten and others not,
    /// breaking the invariant that all overlay ids are remapped. Caller
    /// must abort the tx on cancellation (drop the StagingStore).
    ///
    /// Rewrite all staged `Set` values via a byte transform.
    ///
    /// Used by `TxContext::apply_id_remap` during commit phase 1 to
    /// replace overlay interner ids with stable base ids in staged
    /// record bytes before they reach `transact()`.
    pub async fn rewrite_set_bytes<F>(&self, mut f: F) -> Result<(), String>
    where
        F: FnMut(&Bytes) -> Result<Bytes, String>,
    {
        let keys: Vec<RecordKey> = {
            let mut out = Vec::new();
            self.writes.scan_async(|k, _v| out.push(k.clone())).await;
            out
        };
        for k in keys {
            let mut err: Option<String> = None;
            self.writes
                .update_async(&k, |_kk, op| {
                    if let StagedOp::Set(bytes) = op {
                        match f(bytes) {
                            Ok(new_bytes) => *bytes = new_bytes,
                            Err(e) => err = Some(e),
                        }
                    }
                })
                .await;
            if let Some(e) = err {
                return Err(e);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shamir_storage::storage_in_memory::InMemoryStore;

    fn mem_store() -> Arc<dyn Store> {
        Arc::new(InMemoryStore::new())
    }

    #[tokio::test]
    async fn get_after_set_returns_staged_value() {
        let base = mem_store();
        let staging = StagingStore::new(base);
        let k: RecordKey = Bytes::from_static(b"k1");
        staging.set(k.clone(), Bytes::from_static(b"v1")).await;
        assert_eq!(staging.get(k).await.unwrap(), Bytes::from_static(b"v1"));
    }

    #[tokio::test]
    async fn get_after_remove_returns_not_found_even_if_base_has_key() {
        let base = mem_store();
        let k: RecordKey = Bytes::from_static(b"k1");
        base.set(k.clone(), Bytes::from_static(b"original"))
            .await
            .unwrap();

        let staging = StagingStore::new(base);
        staging.remove(k.clone()).await;
        assert!(staging.get(k).await.is_err());
    }

    #[tokio::test]
    async fn get_falls_through_to_base_if_not_staged() {
        let base = mem_store();
        let k: RecordKey = Bytes::from_static(b"k1");
        base.set(k.clone(), Bytes::from_static(b"from_base"))
            .await
            .unwrap();

        let staging = StagingStore::new(base);
        assert_eq!(
            staging.get(k).await.unwrap(),
            Bytes::from_static(b"from_base")
        );
    }

    #[tokio::test]
    async fn set_then_remove_collapses_to_remove() {
        let base = mem_store();
        let staging = StagingStore::new(base);
        let k: RecordKey = Bytes::from_static(b"k1");

        staging.set(k.clone(), Bytes::from_static(b"v")).await;
        staging.remove(k.clone()).await;

        assert!(staging.get(k).await.is_err());
        assert_eq!(staging.len(), 1); // one key, final op = Remove
    }

    #[tokio::test]
    async fn remove_then_set_collapses_to_set() {
        let base = mem_store();
        let k: RecordKey = Bytes::from_static(b"k1");
        base.set(k.clone(), Bytes::from_static(b"original"))
            .await
            .unwrap();

        let staging = StagingStore::new(base);
        staging.remove(k.clone()).await;
        staging.set(k.clone(), Bytes::from_static(b"new")).await;

        assert_eq!(staging.get(k).await.unwrap(), Bytes::from_static(b"new"));
    }

    #[tokio::test]
    async fn drain_produces_kvop_batch() {
        let base = mem_store();
        let staging = StagingStore::new(base);
        let k1: RecordKey = Bytes::from_static(b"k1");
        let k2: RecordKey = Bytes::from_static(b"k2");
        let k3: RecordKey = Bytes::from_static(b"k3");

        staging.set(k1.clone(), Bytes::from_static(b"v1")).await;
        staging.remove(k2.clone()).await;
        staging.set(k3.clone(), Bytes::from_static(b"v3")).await;

        let ops = staging.drain();
        assert_eq!(ops.len(), 3);

        let sets: Vec<_> = ops
            .iter()
            .filter(|o| matches!(o, KvOp::Set(_, _)))
            .collect();
        let removes: Vec<_> = ops
            .iter()
            .filter(|o| matches!(o, KvOp::Remove(_)))
            .collect();
        assert_eq!(sets.len(), 2);
        assert_eq!(removes.len(), 1);
    }

    #[tokio::test]
    async fn len_tracks_unique_keys() {
        let base = mem_store();
        let staging = StagingStore::new(base);
        let k: RecordKey = Bytes::from_static(b"k1");

        assert!(staging.is_empty());
        staging.set(k.clone(), Bytes::from_static(b"v1")).await;
        assert_eq!(staging.len(), 1);
        staging.set(k.clone(), Bytes::from_static(b"v2")).await;
        assert_eq!(staging.len(), 1); // same key, still 1
    }

    #[tokio::test]
    async fn snapshot_ops_does_not_consume() {
        let base: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let staging = StagingStore::new(base);
        staging
            .set(
                RecordKey::from(Bytes::from_static(b"k1")),
                Bytes::from_static(b"v1"),
            )
            .await;
        staging
            .remove(RecordKey::from(Bytes::from_static(b"k2")))
            .await;

        let snapshot1 = staging.snapshot_ops();
        let snapshot2 = staging.snapshot_ops();
        assert_eq!(snapshot1.len(), 2);
        assert_eq!(snapshot2.len(), 2, "snapshot_ops must NOT consume");
        assert_eq!(staging.len(), 2);
    }
}
