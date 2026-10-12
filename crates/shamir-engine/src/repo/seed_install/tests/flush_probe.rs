use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{RecordKey, Store};

use crate::meta::repl_bookmark::load_replication_bookmark;
use crate::repo::SeedInstallState;

// Setup-only test instrument. Atomics provide deterministic boundary evidence,
// not a power-loss emulator. The actual disk test is separately Unix-gated.
pub(super) struct FlushProbe {
    pub inner: Arc<dyn Store>,
    pub info: Arc<dyn Store>,
    pub fail: AtomicBool,
    pub flushed: AtomicBool,
    pub flushes: AtomicUsize,
    pub predecessor: Option<Arc<FlushProbe>>,
    pub is_bookmark: bool,
}

type TestStream = Pin<Box<dyn Stream<Item = DbResult<Vec<(RecordKey, Bytes)>>> + Send>>;

#[async_trait]
impl Store for FlushProbe {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        if self.is_bookmark {
            assert!(self
                .predecessor
                .as_ref()
                .unwrap()
                .flushed
                .load(Ordering::Acquire));
            assert!(matches!(
                SeedInstallState::load(&self.info).await?,
                Some(SeedInstallState::Installing(_))
            ));
        }
        self.inner.set(key, value).await
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }

    async fn flush(&self) -> DbResult<()> {
        self.flushes.fetch_add(1, Ordering::AcqRel);
        if !self.is_bookmark {
            assert_eq!(load_replication_bookmark(&self.info).await?, 0);
        }
        if self.fail.load(Ordering::Acquire) {
            return Err(DbError::Storage("injected seed flush failure".into()));
        }
        self.inner.flush().await?;
        self.flushed.store(true, Ordering::Release);
        Ok(())
    }

    fn iter_stream(&self, size: usize) -> TestStream {
        self.inner.iter_stream(size)
    }

    fn scan_prefix_stream(&self, prefix: Bytes, size: usize) -> TestStream {
        self.inner.scan_prefix_stream(prefix, size)
    }

    fn iter_range_stream(
        &self,
        start: Option<Bytes>,
        end: Option<Bytes>,
        size: usize,
    ) -> TestStream {
        self.inner.iter_range_stream(start, end, size)
    }

    fn iter_range_stream_reverse(
        &self,
        start: Option<Bytes>,
        end: Option<Bytes>,
        size: usize,
    ) -> TestStream {
        self.inner.iter_range_stream_reverse(start, end, size)
    }
}
