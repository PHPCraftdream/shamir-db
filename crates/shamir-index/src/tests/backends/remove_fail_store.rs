use async_trait::async_trait;
use bytes::Bytes;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{RecordKey, Store};

type TestStream = std::pin::Pin<
    Box<
        dyn futures::Stream<
                Item = Result<Vec<(RecordKey, bytes::Bytes)>, shamir_storage::error::DbError>,
            > + Send,
    >,
>;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
pub(super) struct RemoveFailStore {
    inner: Arc<dyn Store>,
    fail: AtomicBool,
}
impl RemoveFailStore {
    /// Fresh store that fails the FIRST `remove` (then recovers).
    pub(super) fn new(inner: Arc<dyn Store>) -> Self {
        Self {
            inner,
            fail: AtomicBool::new(true),
        }
    }
}
#[async_trait]
impl Store for RemoveFailStore {
    async fn insert(&self, v: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(v).await
    }
    async fn set(&self, k: RecordKey, v: Bytes) -> DbResult<bool> {
        self.inner.set(k, v).await
    }
    async fn get(&self, k: RecordKey) -> DbResult<Bytes> {
        self.inner.get(k).await
    }
    async fn remove(&self, k: RecordKey) -> DbResult<bool> {
        if self.fail.swap(false, Ordering::SeqCst) {
            return Err(DbError::Storage("injected remove failure".into()));
        }
        self.inner.remove(k).await
    }
    fn iter_stream(&self, n: usize) -> TestStream {
        self.inner.iter_stream(n)
    }
    fn scan_prefix_stream(&self, p: Bytes, n: usize) -> TestStream {
        self.inner.scan_prefix_stream(p, n)
    }
    async fn set_many(&self, v: Vec<(RecordKey, Bytes)>) -> DbResult<Vec<bool>> {
        self.inner.set_many(v).await
    }
    async fn remove_many(&self, v: Vec<RecordKey>) -> DbResult<Vec<bool>> {
        self.inner.remove_many(v).await
    }
}
