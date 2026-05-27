//! Write-op planning primitives for transactional index commit.

use crate::index2::backend::{IndexBackend, IndexError};
use shamir_storage::types::Store;
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
    let mut in_memory_ops: Vec<&IndexWriteOp> = Vec::new();

    for op in ops {
        match op {
            IndexWriteOp::SetPosting { key, value } => {
                store
                    .set(key.clone(), value.clone())
                    .await
                    .map_err(|e| IndexError::Storage(e.to_string()))?;
            }
            IndexWriteOp::RemovePosting { key } => {
                let _ = store
                    .remove(key.clone())
                    .await
                    .map_err(|e| IndexError::Storage(e.to_string()))?;
            }
            other => {
                in_memory_ops.push(other);
            }
        }
    }

    if !in_memory_ops.is_empty() {
        backend
            .apply_in_memory(
                &in_memory_ops
                    .iter()
                    .map(|o| (*o).clone())
                    .collect::<Vec<_>>(),
            )
            .await?;
    }

    Ok(())
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
}
