use crate::backend::{IndexBackend, IndexError, IndexQuery, IndexResult};
use crate::descriptor::IndexDescriptor;
use crate::kind::IndexKind;
use crate::write_ops::{apply_index_ops, apply_index_ops_tx, IndexWriteOp};
use async_trait::async_trait;
use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use smallvec::SmallVec;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

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
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
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
