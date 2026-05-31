//! Write-op planning primitives for transactional index commit.

use crate::index2::backend::{IndexBackend, IndexError};
use shamir_storage::types::{KvOp, Store};
use std::sync::Arc;

// Re-export from shamir-tx where the pure-data enum now lives.
pub use shamir_tx::IndexWriteOp;

/// Apply a slice of index write ops against a store + backend.
///
/// `SetPosting` / `RemovePosting` go to `store.set / store.remove`.
/// `BumpFtsStats` goes to `backend.apply_in_memory`.
///
/// Non-tx callers invoke this right after `plan_*`.
/// Tx callers invoke this under the commit lock after merging all
/// ops from `TxContext.index_write_set`.
pub async fn apply_index_ops(
    ops: &[IndexWriteOp],
    store: &Arc<dyn Store>,
    backend: &dyn IndexBackend,
) -> Result<(), IndexError> {
    // Collect Set/Remove postings into one ordered KvOp batch (preserving
    // input order — Set-then-Remove on the same key still yields the
    // last-write-wins semantics of the per-key loop). On transactional
    // backends `Store::transact` collapses N fsyncs into one; on the
    // default loop impl the result is identical to the previous per-op
    // path.
    let mut kv_ops: Vec<KvOp> = Vec::with_capacity(ops.len());
    let mut in_memory_ops: Vec<IndexWriteOp> = Vec::new();

    for op in ops {
        match op {
            IndexWriteOp::SetPosting { key, value } => {
                kv_ops.push(KvOp::Set(key.clone(), value.clone()));
            }
            IndexWriteOp::RemovePosting { key } => {
                kv_ops.push(KvOp::Remove(key.clone()));
            }
            other => in_memory_ops.push(other.clone()),
        }
    }

    if !kv_ops.is_empty() {
        store
            .transact(kv_ops)
            .await
            .map_err(|e| IndexError::Storage(e.to_string()))?;
    }

    if !in_memory_ops.is_empty() {
        backend.apply_in_memory(&in_memory_ops).await?;
    }

    Ok(())
}

/// Apply a batch of staged index ops at transaction commit time
/// (commit pipeline Phase 5c).
///
/// `tx.index_write_set` accumulates `IndexWriteOp`s **without** index-id
/// attribution (only a per-op `table_token`). At commit we therefore:
///
/// - `SetPosting` / `RemovePosting` → applied directly to the table's
///   `info_store` (the same physical store every index2 backend writes
///   its postings into — see `TableManager::create`). This is exactly
///   what V2 WAL recovery does for `IndexPut` / `IndexDel`
///   (`recovery::replay_v2_op`), so re-applying after a happy-path
///   commit is idempotent (`set`/`remove` are last-write-wins).
///
/// - `BumpFtsStats` → broadcast to **all** of the table's index2
///   backends via `apply_in_memory`. Only the FTS-ranked backend
///   reacts (its `apply_in_memory` matches `BumpFtsStats`); every other
///   backend's default impl is a no-op. Broadcasting is necessary
///   because the op carries no idx_id to pinpoint the owning backend.
///   `BumpFtsStats` is in-memory only and is **not** serialised to the
///   WAL (`wal_ops_from_tx` skips it), so crash recovery rebuilds these
///   counters via `rebuild()` on open rather than replaying them.
pub async fn apply_index_ops_at_commit(
    ops: &[IndexWriteOp],
    info_store: &Arc<dyn Store>,
    backends: &[Arc<dyn IndexBackend>],
) -> Result<(), IndexError> {
    // Collapse all SetPosting / RemovePosting ops into one ordered
    // `Store::transact` batch. On transactional backends (sled, redb,
    // fjall, persy, nebari, canopy) the batch is one fsync instead of N
    // — exactly mirroring the V2 WAL recovery path's effect when it
    // batch-replays IndexPut/IndexDel. Last-write-wins semantics are
    // preserved by feeding ops in their original order. BumpFtsStats is
    // in-memory only and unchanged.
    let mut kv_ops: Vec<KvOp> = Vec::with_capacity(ops.len());
    let mut in_memory_ops: Vec<IndexWriteOp> = Vec::new();

    for op in ops {
        match op {
            IndexWriteOp::SetPosting { key, value } => {
                kv_ops.push(KvOp::Set(key.clone(), value.clone()));
            }
            IndexWriteOp::RemovePosting { key } => {
                kv_ops.push(KvOp::Remove(key.clone()));
            }
            other => in_memory_ops.push(other.clone()),
        }
    }

    if !kv_ops.is_empty() {
        info_store
            .transact(kv_ops)
            .await
            .map_err(|e| IndexError::Storage(e.to_string()))?;
    }

    if !in_memory_ops.is_empty() {
        for backend in backends {
            backend.apply_in_memory(&in_memory_ops).await?;
        }
    }

    Ok(())
}

/// tx-aware variant of [`apply_index_ops`].
///
/// - `tx == None` → behaves exactly like [`apply_index_ops`]: ops are
///   applied immediately (`SetPosting`/`RemovePosting` go to the
///   store; in-memory ops go to `backend.apply_in_memory`).
/// - `tx == Some(tx)` → ops are **staged** in `tx.index_write_set`
///   under the supplied `table_token`. Nothing is written to the
///   store or to the backend's in-memory state. A dropped tx
///   (rolled back) therefore leaves no postings; a committed tx
///   applies them via the commit pipeline. See HIGH-6.
///
/// `table_token` is the deterministic per-table hash (see
/// `table_manager::table_token_for`). It is ignored when `tx == None`.
///
/// HIGH-6: staged ops are applied on the happy commit path by
/// `commit::commit_tx_inner` Phase 5c via [`apply_index_ops_at_commit`],
/// and replayed on crash recovery via `recovery::replay_v2_op`
/// (`IndexPut` / `IndexDel`). A dropped/aborted tx leaves no postings
/// because `index_write_set` is owned by the `TxContext` (RAII drop).
pub async fn apply_index_ops_tx(
    ops: &[IndexWriteOp],
    store: &Arc<dyn Store>,
    backend: &dyn IndexBackend,
    table_token: u64,
    tx: Option<&mut shamir_tx::TxContext>,
) -> Result<(), IndexError> {
    if let Some(tx) = tx {
        tx.index_write_set
            .extend(ops.iter().cloned().map(|op| (table_token, op)));
        return Ok(());
    }
    apply_index_ops(ops, store, backend).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index2::backend::{IndexBackend, IndexError, IndexQuery, IndexResult};
    use crate::index2::descriptor::IndexDescriptor;
    use crate::index2::kind::IndexKind;
    use async_trait::async_trait;
    use bytes::Bytes;
    use shamir_storage::storage_in_memory::InMemoryStore;
    use smallvec::SmallVec;
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicU32, Ordering};

    // Minimal mock backend for testing apply_index_ops
    struct MockBackend {
        descriptor: IndexDescriptor,
        bump_count: AtomicU32,
    }

    impl MockBackend {
        fn new() -> Self {
            Self {
                descriptor: IndexDescriptor::new(
                    1,
                    "mock",
                    100,
                    SmallVec::new(),
                    IndexKind::Btree { unique: false },
                ),
                bump_count: AtomicU32::new(0),
            }
        }
    }

    #[async_trait]
    impl IndexBackend for MockBackend {
        fn descriptor(&self) -> &IndexDescriptor {
            &self.descriptor
        }
        async fn lookup(&self, _: IndexQuery) -> Result<IndexResult, IndexError> {
            Ok(IndexResult::Set(BTreeSet::new()))
        }
        async fn rebuild(&self, _: Arc<dyn Store>) -> Result<(), IndexError> {
            Ok(())
        }
        async fn drop_all(&self) -> Result<(), IndexError> {
            Ok(())
        }

        async fn apply_in_memory(&self, ops: &[IndexWriteOp]) -> Result<(), IndexError> {
            for op in ops {
                if let IndexWriteOp::BumpFtsStats { .. } = op {
                    self.bump_count.fetch_add(1, Ordering::Relaxed);
                }
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn apply_empty_ops_noop() {
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let backend = MockBackend::new();
        apply_index_ops(&[], &store, &backend).await.unwrap();
    }

    #[tokio::test]
    async fn apply_set_posting_writes_to_store() {
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let backend = MockBackend::new();
        let ops = vec![IndexWriteOp::SetPosting {
            key: Bytes::from_static(b"posting_key_1"),
            value: Bytes::from_static(b"posting_val"),
        }];
        apply_index_ops(&ops, &store, &backend).await.unwrap();
        let val = store
            .get(Bytes::from_static(b"posting_key_1"))
            .await
            .unwrap();
        assert_eq!(val.as_ref(), b"posting_val");
    }

    #[tokio::test]
    async fn apply_remove_posting_removes_from_store() {
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        store
            .set(Bytes::from_static(b"k"), Bytes::from_static(b"v"))
            .await
            .unwrap();
        let backend = MockBackend::new();
        let ops = vec![IndexWriteOp::RemovePosting {
            key: Bytes::from_static(b"k"),
        }];
        apply_index_ops(&ops, &store, &backend).await.unwrap();
        assert!(store.get(Bytes::from_static(b"k")).await.is_err());
    }

    #[tokio::test]
    async fn apply_bump_fts_stats_delegates_to_backend() {
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let backend = MockBackend::new();
        let ops = vec![
            IndexWriteOp::BumpFtsStats {
                doc_len: 10,
                sign: 1,
            },
            IndexWriteOp::BumpFtsStats {
                doc_len: 5,
                sign: -1,
            },
        ];
        apply_index_ops(&ops, &store, &backend).await.unwrap();
        assert_eq!(backend.bump_count.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn lookup_tx_none_forwards_to_lookup() {
        let backend = MockBackend::new();
        let res = backend
            .lookup_tx(
                0,
                IndexQuery::Point {
                    keys: SmallVec::new(),
                },
                None,
                None,
            )
            .await
            .unwrap();
        assert!(matches!(res, IndexResult::Set(ref s) if s.is_empty()));
    }

    #[tokio::test]
    async fn lookup_tx_some_forwards_to_lookup() {
        use shamir_types::types::record_id::RecordId;
        let backend = MockBackend::new();
        // Non-vector backends ignore the staged slice and forward to lookup.
        let staged: Vec<(RecordId, Vec<f32>)> = Vec::new();
        let res = backend
            .lookup_tx(
                0,
                IndexQuery::Point {
                    keys: SmallVec::new(),
                },
                None,
                Some(&staged),
            )
            .await
            .unwrap();
        assert!(matches!(res, IndexResult::Set(ref s) if s.is_empty()));
    }

    #[tokio::test]
    async fn apply_mixed_ops_in_order() {
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let backend = MockBackend::new();
        let ops = vec![
            IndexWriteOp::SetPosting {
                key: Bytes::from_static(b"a"),
                value: Bytes::from_static(b"1"),
            },
            IndexWriteOp::SetPosting {
                key: Bytes::from_static(b"b"),
                value: Bytes::from_static(b"2"),
            },
            IndexWriteOp::RemovePosting {
                key: Bytes::from_static(b"a"),
            },
            IndexWriteOp::BumpFtsStats {
                doc_len: 7,
                sign: 1,
            },
        ];
        apply_index_ops(&ops, &store, &backend).await.unwrap();
        // "a" was set then removed -> not found
        assert!(store.get(Bytes::from_static(b"a")).await.is_err());
        // "b" set -> found
        assert_eq!(
            store.get(Bytes::from_static(b"b")).await.unwrap().as_ref(),
            b"2"
        );
        // bump called once
        assert_eq!(backend.bump_count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn apply_index_ops_tx_none_forwards() {
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let backend = MockBackend::new();
        let ops = vec![
            IndexWriteOp::SetPosting {
                key: Bytes::from_static(b"k1"),
                value: Bytes::from_static(b"v1"),
            },
            IndexWriteOp::BumpFtsStats {
                doc_len: 5,
                sign: 1,
            },
        ];
        apply_index_ops_tx(&ops, &store, &backend, 0, None)
            .await
            .unwrap();

        let got = store.get(Bytes::from_static(b"k1")).await.unwrap();
        assert_eq!(got.as_ref(), b"v1");
        assert_eq!(backend.bump_count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn apply_index_ops_tx_some_stages_into_tx() {
        use shamir_tx::{IsolationLevel, TxContext, TxId};
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let backend = MockBackend::new();
        let mut tx = TxContext::new(TxId::new(1), 0, 42, IsolationLevel::Snapshot);
        let ops = vec![
            IndexWriteOp::SetPosting {
                key: Bytes::from_static(b"k2"),
                value: Bytes::from_static(b"v2"),
            },
            IndexWriteOp::BumpFtsStats {
                doc_len: 7,
                sign: 1,
            },
        ];
        const TBL_TOKEN: u64 = 0xdead_beef;
        apply_index_ops_tx(&ops, &store, &backend, TBL_TOKEN, Some(&mut tx))
            .await
            .unwrap();

        // Nothing written to the live store / backend in tx mode.
        assert!(store.get(Bytes::from_static(b"k2")).await.is_err());
        assert_eq!(backend.bump_count.load(Ordering::Relaxed), 0);

        // All ops appear in tx.index_write_set under the supplied token.
        assert_eq!(tx.index_write_set.len(), 2);
        for (tok, _) in &tx.index_write_set {
            assert_eq!(*tok, TBL_TOKEN);
        }
    }

    #[tokio::test]
    async fn apply_index_ops_tx_drop_leaves_no_postings() {
        // HIGH-6 regression: with tx == Some, dropping the tx without
        // commit must NOT leave any posting in the store / in-memory
        // backend state. Staging in `index_write_set` is RAII-cleaned
        // because TxContext owns the vec.
        use shamir_tx::{IsolationLevel, TxContext, TxId};
        let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
        let backend = MockBackend::new();
        {
            let mut tx = TxContext::new(TxId::new(99), 0, 1, IsolationLevel::Snapshot);
            let ops = vec![
                IndexWriteOp::SetPosting {
                    key: Bytes::from_static(b"ghost"),
                    value: Bytes::from_static(b"v"),
                },
                IndexWriteOp::BumpFtsStats {
                    doc_len: 3,
                    sign: 1,
                },
            ];
            apply_index_ops_tx(&ops, &store, &backend, 1, Some(&mut tx))
                .await
                .unwrap();
            assert_eq!(tx.index_write_set.len(), 2);
            // tx dropped here.
        }
        assert!(
            store.get(Bytes::from_static(b"ghost")).await.is_err(),
            "rolled-back tx must not leave postings in store (HIGH-6)"
        );
        assert_eq!(
            backend.bump_count.load(Ordering::Relaxed),
            0,
            "rolled-back tx must not bump backend in-memory state"
        );
    }
}
