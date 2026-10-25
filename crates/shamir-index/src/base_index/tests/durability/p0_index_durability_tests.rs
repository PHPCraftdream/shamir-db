//! P0 stage A5 — RV-INDEX-86978429 regression test.
//!
//! `SortedIndexManager::load` used to map EVERY definition-store `get` error
//! to a successful empty initialization. A transient store error therefore
//! silently loaded zero sorted definitions, and a later `persist_defs`
//! (triggered by any register/drop/rename) serialized and wrote the empty
//! registry back — permanently destroying the unread definitions. Only a
//! missing record (`NotFound`) means "no sorted indexes yet"; every other
//! store error must propagate and fail the open.
//!
//! Why only the FIRST `get` fails: with the `load()` fix reverted (back to
//! `Err(_) => return Ok(())`), `SortedIndexManager::new` proceeds into
//! `recover_in_progress_drops` → `load_dropping_sorted` (ddl.rs), whose `get`
//! must SUCCEED (returning NotFound → Ok) — otherwise `new()` would still
//! return Err and the test would pass vacuously on the reverted code. With
//! the fix in place, `load()` itself propagates the injected error → `new()`
//! returns Err. Failing only call 1 therefore makes the test discriminate
//! precisely the `load()` propagation.

use crate::base_index::sorted_index_manager::SortedIndexManager;
use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const GET_FAIL_MSG: &str = "injected get failure";

type TestStream = Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>;

/// `Store` wrapper whose `get` fails ONLY on the first call (subsequent calls
/// pass through to inner). See the module doc for why the fault must be
/// first-call-only to be discriminating.
struct GetFailStore {
    inner: Arc<dyn Store>,
    get_calls: AtomicU64,
}

impl GetFailStore {
    fn new(inner: Arc<dyn Store>) -> Self {
        Self {
            inner,
            get_calls: AtomicU64::new(0),
        }
    }
}

#[async_trait]
impl Store for GetFailStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        self.inner.set(key, value).await
    }
    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        if self.get_calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(DbError::Storage(GET_FAIL_MSG.to_string()));
        }
        self.inner.get(key).await
    }
    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }
    fn iter_stream(&self, batch_size: usize) -> TestStream {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> TestStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
    async fn set_many(&self, items: Vec<(RecordKey, Bytes)>) -> DbResult<Vec<bool>> {
        self.inner.set_many(items).await
    }
    async fn remove_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<bool>> {
        self.inner.remove_many(keys).await
    }
}

/// RV-INDEX-86978429: a definition-store read error on open must propagate —
/// `SortedIndexManager::new` must fail closed instead of initializing with
/// zero sorted definitions (which a later `persist_defs` would persist,
/// destroying the unread registry).
#[tokio::test]
async fn rv_index_86978429_defs_get_error_fails_open_not_empty_init() {
    let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let failing: Arc<dyn Store> = Arc::new(GetFailStore::new(Arc::clone(&inner)));

    let res = SortedIndexManager::new(Arc::clone(&failing)).await;
    assert!(
        res.is_err(),
        "SortedIndexManager::new must propagate definition-store get errors — \
         old code swallowed them into an empty registry that a later \
         persist_defs would write back, destroying the definitions"
    );
}
