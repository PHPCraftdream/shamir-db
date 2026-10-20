#![allow(deprecated)]

use crate::error::{DbError, DbResult};
use crate::storage_cached::CachedStore;
use crate::storage_in_memory::InMemoryStore;
use crate::types::{RecordKey, Store};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::Arc;
/// A `Store` wrapper whose `set`/`remove` always fail, simulating a backing
/// store that goes unavailable. `get`/`iter_stream`/`scan_prefix_stream`
/// delegate normally so `CachedStore::new_async`'s initial load still works.
struct FailingStore {
    inner: Arc<dyn Store>,
}

#[async_trait]
impl Store for FailingStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }

    async fn set(&self, _key: RecordKey, _value: Bytes) -> DbResult<bool> {
        Err(DbError::Storage("simulated backing-store failure".into()))
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }

    async fn remove(&self, _key: RecordKey) -> DbResult<bool> {
        Err(DbError::Storage("simulated backing-store failure".into()))
    }

    fn iter_stream(
        &self,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, crate::error::DbError>> + Send>>
    {
        self.inner.iter_stream(batch_size)
    }

    fn scan_prefix_stream(
        &self,
        prefix: Bytes,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, crate::error::DbError>> + Send>>
    {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

/// A background `set` failure must surface as an `Err` from the NEXT
/// `flush()` call — not be silently swallowed into an `Ok(())`. The cache
/// itself still holds the value (write-behind semantics unchanged); only
/// the durability signal changes.
#[tokio::test]
async fn test_async_flush_surfaces_background_set_failure() {
    let plain_inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let failing_inner = Arc::new(FailingStore { inner: plain_inner }) as Arc<dyn Store>;
    let cached = CachedStore::new_async(failing_inner).await.unwrap();

    let key = RecordKey::from_slice(b"will-fail");
    // The synchronous part of `set` (cache upsert + enqueue) succeeds —
    // only the background write to `inner` fails.
    cached
        .set(key.clone(), Bytes::from_static(b"v"))
        .await
        .unwrap();

    // Cache already has the value (write-behind: visible immediately).
    assert!(cached.cache().peek_with(&key, |_, _| ()).is_some());

    let flush_result = cached.flush().await;
    assert!(
        flush_result.is_err(),
        "flush() must report the background write failure, not silently succeed"
    );

    // The error is reported exactly once: a second flush() with no new
    // writes since must succeed (nothing pending, nothing to report).
    cached
        .flush()
        .await
        .expect("error must be cleared after being reported once");
}

/// Same as above, for the `remove` path.
#[tokio::test]
async fn test_async_flush_surfaces_background_remove_failure() {
    let plain_inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let failing_inner = Arc::new(FailingStore { inner: plain_inner }) as Arc<dyn Store>;
    let cached = CachedStore::new_async(failing_inner).await.unwrap();

    let key = RecordKey::from_slice(b"remove-will-fail");
    cached.remove(key).await.unwrap();

    let flush_result = cached.flush().await;
    assert!(
        flush_result.is_err(),
        "flush() must report the background remove failure, not silently succeed"
    );
}

/// The `Store::flush` trait-dispatch override (reached through `Arc<dyn
/// Store>`, the shape every engine call site actually holds) must ALSO
/// surface the background failure, not just the inherent `CachedStore::flush`.
#[tokio::test]
async fn test_async_flush_surfaces_background_failure_via_dyn_store() {
    let plain_inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let failing_inner = Arc::new(FailingStore { inner: plain_inner }) as Arc<dyn Store>;
    let cached_concrete = Arc::new(CachedStore::new_async(failing_inner).await.unwrap());
    let cached_dyn: Arc<dyn Store> = cached_concrete.clone();

    cached_dyn
        .set(
            RecordKey::from_slice(b"dyn-will-fail"),
            Bytes::from_static(b"v"),
        )
        .await
        .unwrap();

    assert!(
        cached_dyn.flush().await.is_err(),
        "Store::flush via dyn dispatch must also surface the background failure"
    );
}

// ============================================================================
// Regression (found by @oh review, 2026-08-11): `Store::flush` must run
// `inner.flush()` unconditionally, even when a background write failed.
// An earlier version of the #1082 fix used `wait_for_async_writes().await?`,
// which short-circuited past `inner.flush()` entirely on ANY recorded
// background error -- silently skipping `inner`'s own durability work
// (e.g. draining a MemBuffer overlay, fsync) for every OTHER,
// successfully-buffered write, not just the one that failed.
// ============================================================================

/// A `Store` wrapper whose `set` fails for exactly one designated key and
/// counts `flush()` calls that reach it. Everything else delegates.
struct PartialFailStore {
    inner: Arc<dyn Store>,
    fail_key: RecordKey,
    flush_called: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait]
impl Store for PartialFailStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        if key == self.fail_key {
            return Err(DbError::Storage("simulated failure for this key".into()));
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
        self.flush_called.store(true, Ordering::SeqCst);
        self.inner.flush().await
    }

    fn iter_stream(
        &self,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, crate::error::DbError>> + Send>>
    {
        self.inner.iter_stream(batch_size)
    }

    fn scan_prefix_stream(
        &self,
        prefix: Bytes,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, crate::error::DbError>> + Send>>
    {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

/// A background write failure for ONE key must not prevent `inner.flush()`
/// from running at all -- if it did, every OTHER, successfully-buffered
/// write would silently fail to become durable at the next `flush()`,
/// typically the graceful-shutdown flush (the one call that actually
/// matters).
#[tokio::test]
async fn test_async_flush_runs_inner_flush_despite_background_failure() {
    let plain_inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let flush_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fail_key = RecordKey::from_slice(b"will-fail");
    let partial_fail_inner = Arc::new(PartialFailStore {
        inner: plain_inner.clone(),
        fail_key: fail_key.clone(),
        flush_called: flush_called.clone(),
    }) as Arc<dyn Store>;
    let cached = CachedStore::new_async(partial_fail_inner).await.unwrap();

    // One background write fails...
    cached
        .set(fail_key.clone(), Bytes::from_static(b"v"))
        .await
        .unwrap();
    // ...but a second, different write must still succeed in the background.
    let ok_key = RecordKey::from_slice(b"will-succeed");
    cached
        .set(ok_key.clone(), Bytes::from_static(b"ok"))
        .await
        .unwrap();

    // flush() must surface the background failure...
    assert!(
        cached.flush().await.is_err(),
        "flush() must still report the background failure"
    );

    // ...but inner.flush() must have run regardless -- the regression this
    // test guards against is an early `?` on the background-error branch
    // that skipped it entirely.
    assert!(
        flush_called.load(Ordering::SeqCst),
        "inner.flush() must run even when a background write failed -- \
         skipping it silently drops every OTHER buffered write, not just \
         the one that failed"
    );

    // And the successfully-written key actually landed in inner.
    let got = plain_inner.get(ok_key).await.unwrap();
    assert_eq!(got.as_ref(), b"ok");
}
