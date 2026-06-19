use crate::mvcc_store::MvccStore;
use crate::repo_tx_gate::RepoTxGate;
use shamir_storage::types::Store;
use std::sync::Arc;

// ================================================================
// Fault-injecting Store double for I/O-error propagation tests.
// ================================================================

pub(super) mod failing_store {
    use async_trait::async_trait;
    use bytes::Bytes;
    use shamir_storage::error::{DbError, DbResult};
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_storage::types::{KvOp, RecordKey, Store};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};

    use futures::stream::Stream;

    /// A test double that wraps `InMemoryStore` and can be armed to
    /// inject I/O errors on `get` and/or `remove` calls. Used to
    /// regression-test that `MvccStore` propagates non-NotFound
    /// errors rather than swallowing them.
    pub struct FailingStore {
        inner: InMemoryStore,
        /// When `true`, the next `get` call returns a Storage error.
        pub fail_get: AtomicBool,
        /// When `true`, the next `remove` call returns a Storage error.
        pub fail_remove: AtomicBool,
        /// When `true`, the next `set` call returns a Storage error.
        pub fail_set: AtomicBool,
    }

    impl FailingStore {
        pub fn new() -> Self {
            Self {
                inner: InMemoryStore::new(),
                fail_get: AtomicBool::new(false),
                fail_remove: AtomicBool::new(false),
                fail_set: AtomicBool::new(false),
            }
        }

        fn injected_error() -> DbError {
            DbError::Storage("injected I/O fault".into())
        }
    }

    #[async_trait]
    impl Store for FailingStore {
        async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
            self.inner.insert(value).await
        }

        async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
            if self.fail_set.load(Ordering::Relaxed) {
                return Err(Self::injected_error());
            }
            self.inner.set(key, value).await
        }

        async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
            if self.fail_get.load(Ordering::Relaxed) {
                return Err(Self::injected_error());
            }
            self.inner.get(key).await
        }

        async fn remove(&self, key: RecordKey) -> DbResult<bool> {
            if self.fail_remove.load(Ordering::Relaxed) {
                return Err(Self::injected_error());
            }
            self.inner.remove(key).await
        }

        fn iter_stream(
            &self,
            batch_size: usize,
        ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
            self.inner.iter_stream(batch_size)
        }

        fn scan_prefix_stream(
            &self,
            prefix: Bytes,
            batch_size: usize,
        ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
            self.inner.scan_prefix_stream(prefix, batch_size)
        }

        async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
            // Honour per-op fault flags so batched paths
            // (set_versioned_many, apply_committed_ops) also hit the
            // injection when they call `self.history.set(...)` (log write).
            for op in ops {
                match op {
                    KvOp::Set(k, v) => {
                        let _ = self.set(k, v).await?;
                    }
                    KvOp::Remove(k) => {
                        let _ = self.remove(k).await?;
                    }
                }
            }
            Ok(())
        }
    }
}

// ================================================================
// PausableStore — test double for MVCC-2 deterministic harness.
// ================================================================
//
// Wraps an inner Store and adds a pause point inside `set()`:
// when `armed` is true the first `set()` call signals `entered`
// (so the test knows it is inside the gap) and then blocks on
// `pause_gate` until the test calls `release()`.  This lets the
// test deterministically open a snapshot INSIDE the fast-path
// window between `active_snapshots_empty()` and the actual write.
//
// All other Store methods are delegated to `inner`.
pub(super) mod pausable_store {
    use async_trait::async_trait;
    use bytes::Bytes;
    use futures::stream::Stream;
    use shamir_storage::error::{DbError, DbResult};
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_storage::types::{KvOp, RecordKey, Store};
    use std::pin::Pin;
    use std::sync::{
        atomic::{AtomicBool, Ordering::SeqCst},
        Arc,
    };
    use tokio::sync::Notify;

    pub struct PausableStore {
        pub inner: InMemoryStore,
        /// When `true`, the next `set()` call will pause.
        pub armed: Arc<AtomicBool>,
        /// Notified when `set()` has entered the pause point (before
        /// the actual write) — the test waits on this to know the
        /// write task is suspended in the window.
        pub entered: Arc<Notify>,
        /// The write task blocks here until the test calls `release()`.
        pub pause_gate: Arc<Notify>,
    }

    impl PausableStore {
        pub fn new() -> Self {
            Self {
                inner: InMemoryStore::new(),
                armed: Arc::new(AtomicBool::new(false)),
                entered: Arc::new(Notify::new()),
                pause_gate: Arc::new(Notify::new()),
            }
        }

        /// Arm: the next `set()` call will pause.
        pub fn arm(&self) {
            self.armed.store(true, SeqCst);
        }

        /// Release: unblock the paused `set()`.
        pub fn release(&self) {
            self.pause_gate.notify_one();
        }
    }

    #[async_trait]
    impl Store for PausableStore {
        async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
            if self.armed.swap(false, SeqCst) {
                // Signal to the test that we have reached the pause point
                // (BEFORE the actual log write — i.e., between
                // `publish_cell` and `history.set()`).
                self.entered.notify_one();
                // Block until the test calls release().
                self.pause_gate.notified().await;
            }
            self.inner.set(key, value).await
        }

        async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
            self.inner.insert(value).await
        }

        async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
            self.inner.get(key).await
        }

        async fn remove(&self, key: RecordKey) -> DbResult<bool> {
            self.inner.remove(key).await
        }

        async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
            for op in ops {
                match op {
                    KvOp::Set(k, v) => {
                        let _ = self.set(k, v).await?;
                    }
                    KvOp::Remove(k) => {
                        let _ = self.remove(k).await?;
                    }
                }
            }
            Ok(())
        }

        fn iter_stream(
            &self,
            batch_size: usize,
        ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
            self.inner.iter_stream(batch_size)
        }

        fn scan_prefix_stream(
            &self,
            prefix: Bytes,
            batch_size: usize,
        ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
            self.inner.scan_prefix_stream(prefix, batch_size)
        }
    }
}

// ================================================================
// CountingStore — test double that counts scan_prefix_stream calls.
// ================================================================
//
// Wraps an InMemoryStore and increments an AtomicUsize on every
// `scan_prefix_stream` call. Used by L6 tests to assert that the
// targeted-remove fast path does NOT perform a prefix scan.
pub(super) mod counting_store {
    use async_trait::async_trait;
    use bytes::Bytes;
    use futures::stream::Stream;
    use shamir_storage::error::{DbError, DbResult};
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_storage::types::{KvOp, RecordKey, Store};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub struct CountingStore {
        inner: InMemoryStore,
        pub scan_prefix_count: AtomicUsize,
    }

    impl CountingStore {
        pub fn new() -> Self {
            Self {
                inner: InMemoryStore::new(),
                scan_prefix_count: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl Store for CountingStore {
        async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
            self.inner.insert(value).await
        }

        async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
            self.inner.set(key, value).await
        }

        async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
            self.inner.get(key).await
        }

        async fn remove(&self, key: RecordKey) -> DbResult<bool> {
            self.inner.remove(key).await
        }

        async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
            self.inner.transact(ops).await
        }

        fn iter_stream(
            &self,
            batch_size: usize,
        ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
            self.inner.iter_stream(batch_size)
        }

        fn scan_prefix_stream(
            &self,
            prefix: Bytes,
            batch_size: usize,
        ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
            self.scan_prefix_count.fetch_add(1, Ordering::Relaxed);
            self.inner.scan_prefix_stream(prefix, batch_size)
        }
    }
}

/// Helper: build an MvccStore whose `history` is a FailingStore.
/// History is the sole write target, so I/O error injection must be
/// applied here.
pub(super) fn make_failing_history_mvcc(
    gate: Arc<RepoTxGate>,
) -> (MvccStore, Arc<failing_store::FailingStore>) {
    let history = Arc::new(failing_store::FailingStore::new());
    let mvcc = MvccStore::new(history.clone() as Arc<dyn Store>, gate);
    (mvcc, history)
}
