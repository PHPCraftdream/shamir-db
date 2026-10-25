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
use std::sync::{Arc, Mutex};
use tokio::sync::{oneshot, Notify};

/// 10 s outer bound: ONLY a hang detector, never part of the protocol.
const HANG: std::time::Duration = std::time::Duration::from_secs(10);

/// A `Store` wrapper whose `set`/`remove` fail AFTER parking on `gate` (so
/// the owner's early async ack is observably delivered to the caller before
/// the failure publishes), simulating a backing store that goes
/// unavailable. `get`/`iter_stream`/`scan_prefix_stream` delegate normally
/// so `CachedStore::new_async`'s initial load still works.
struct FailingStore {
    inner: Arc<dyn Store>,
    gate: Arc<Notify>,
}

#[async_trait]
impl Store for FailingStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }

    async fn set(&self, _key: RecordKey, _value: Bytes) -> DbResult<bool> {
        self.gate.notified().await;
        Err(DbError::Storage("simulated backing-store failure".into()))
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }

    async fn remove(&self, _key: RecordKey) -> DbResult<bool> {
        self.gate.notified().await;
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

/// Runs `op` against the store on a spawned task and returns its result
/// only after the owner's reply was OBSERVABLY delivered to a caller (the
/// spawned caller sent it back over a oneshot) — the gated backing failure
/// stays parked until the test releases the gate.
async fn ack_observably<T: Send + 'static>(
    store: Arc<CachedStore>,
    op: impl FnOnce(Arc<CachedStore>) -> Pin<Box<dyn std::future::Future<Output = DbResult<T>> + Send>>
        + Send
        + 'static,
) -> DbResult<T> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let r = op(store.clone()).await;
        let _ = tx.send(r);
    });
    tokio::time::timeout(HANG, rx)
        .await
        .expect("async ack not delivered before the gated failure")
        .expect("caller dropped ack channel")
}

/// A background `set` failure must surface as an `Err` from the NEXT
/// `flush()` call — not be silently swallowed into an `Ok(())`. The cache
/// itself still holds the value (write-behind semantics unchanged); only
/// the durability signal changes.
#[tokio::test]
async fn test_async_flush_surfaces_background_set_failure() {
    let plain_inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let gate = Arc::new(Notify::new());
    let failing_inner = Arc::new(FailingStore {
        inner: plain_inner,
        gate: gate.clone(),
    }) as Arc<dyn Store>;
    let cached = Arc::new(CachedStore::new_async(failing_inner).await.unwrap());

    let key = RecordKey::from_slice(b"will-fail");
    // The synchronous part of `set` (cache upsert + enqueue) succeeds —
    // only the background write to `inner` fails. The failure is gated so
    // the early async ack is OBSERVABLY delivered (oneshot from the spawned
    // caller) before the error publishes, per the async-ack contract.
    let k2 = key.clone();
    let ack = ack_observably(cached.clone(), |s| {
        Box::pin(async move { s.set(k2, Bytes::from_static(b"v")).await })
    })
    .await;
    ack.expect("async ack must succeed before the gated backing failure");

    // Cache already has the value (write-behind: visible immediately).
    assert!(cached.cache().peek_with(&key, |_, _| ()).is_some());

    // Release the gated backing failure: the background write now fails
    // terminally, before flush() runs.
    gate.notify_one();
    let flush_result = cached.flush().await;
    assert!(
        flush_result.is_err(),
        "flush() must report the background write failure, not silently succeed"
    );

    // ROUND 4 (RV-STORAGE-e70a0ddc) supersedes the old consume-once rule: a
    // background write failure is now STICKY-TERMINAL, so this second
    // flush() must ALSO report Err (previously it expected Ok after the
    // error was "cleared"). The cache mirror itself is still intact.
    cached
        .flush()
        .await
        .expect_err("sticky terminal failure must persist across flush() calls");
}

/// Same as above, for the `remove` path.
#[tokio::test]
async fn test_async_flush_surfaces_background_remove_failure() {
    let plain_inner = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let gate = Arc::new(Notify::new());
    let failing_inner = Arc::new(FailingStore {
        inner: plain_inner,
        gate: gate.clone(),
    }) as Arc<dyn Store>;
    let cached = Arc::new(CachedStore::new_async(failing_inner).await.unwrap());

    let key = RecordKey::from_slice(b"remove-will-fail");
    let ack = ack_observably(cached.clone(), |s| {
        Box::pin(async move { s.remove(key.clone()).await })
    })
    .await;
    ack.expect("async ack must succeed before the gated backing failure");

    gate.notify_one();
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
    let gate = Arc::new(Notify::new());
    let failing_inner = Arc::new(FailingStore {
        inner: plain_inner,
        gate: gate.clone(),
    }) as Arc<dyn Store>;
    let cached_concrete = Arc::new(CachedStore::new_async(failing_inner).await.unwrap());
    let cached_dyn: Arc<dyn Store> = cached_concrete.clone();

    let ack = ack_observably(cached_concrete.clone(), |s| {
        Box::pin(async move {
            s.set(
                RecordKey::from_slice(b"dyn-will-fail"),
                Bytes::from_static(b"v"),
            )
            .await
        })
    })
    .await;
    ack.expect("async ack must succeed before the gated backing failure");

    gate.notify_one();
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
/// counts `flush()` calls that reach it. The failing `set` parks on a
/// oneshot release BEFORE returning Err, so the owner's early async ack
/// is observably delivered first (the test releases only after observing
/// the ack) — without the gate the ack/terminal-publication race could
/// make the caller's ack Err nondeterministically. Everything else
/// delegates.
struct PartialFailStore {
    inner: Arc<dyn Store>,
    fail_key: RecordKey,
    flush_called: Arc<std::sync::atomic::AtomicBool>,
    fail_release_rx: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}

#[async_trait]
impl Store for PartialFailStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        if key == self.fail_key {
            let release = { self.fail_release_rx.lock().unwrap().take() };
            if let Some(rx) = release {
                let _ = rx.await;
            }
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
    let (release_tx, release_rx) = oneshot::channel();
    let partial_fail_inner = Arc::new(PartialFailStore {
        inner: plain_inner.clone(),
        fail_key: fail_key.clone(),
        flush_called: flush_called.clone(),
        fail_release_rx: Arc::new(Mutex::new(Some(release_rx))),
    }) as Arc<dyn Store>;
    let cached = Arc::new(CachedStore::new_async(partial_fail_inner).await.unwrap());

    // One background write fails — but ONLY after the test observes the
    // async ack over a oneshot (the failure is parked behind the release
    // oneshot), so the ack here is deterministically Ok. Independent
    // revert caught: an owner that acks only after the backing await (or
    // an unguarded immediate failure racing the ack) makes this ack Err
    // or hang.
    let (ack_tx, ack_rx) = oneshot::channel();
    {
        let cached = cached.clone();
        let fail_key = fail_key.clone();
        tokio::spawn(async move {
            let r = cached.set(fail_key, Bytes::from_static(b"v")).await;
            let _ = ack_tx.send(r);
        });
    }
    let ack = tokio::time::timeout(HANG, ack_rx)
        .await
        .expect("async ack hung")
        .expect("caller dropped ack channel");
    ack.expect("async ack must succeed before the gated backing failure");

    // Cache already has the value (write-behind: visible immediately).
    assert!(cached.cache().peek_with(&fail_key, |_, _| ()).is_some());

    // Release the gated backing failure and await the sticky publication
    // deterministically before any further command.
    release_tx.send(()).expect("release oneshot closed");
    tokio::time::timeout(HANG, cached.await_health_terminal_for_tests())
        .await
        .expect("background write failure never published");

    // ROUND 4 (RV-STORAGE-e70a0ddc): a background write failure is
    // sticky-terminal, so the previously-expected "second, different write
    // must still succeed" is no longer possible — the store is terminally
    // failed and rejects every subsequent command.
    let ok_key = RecordKey::from_slice(b"will-succeed");
    cached
        .set(ok_key.clone(), Bytes::from_static(b"ok"))
        .await
        .expect_err("store must be terminally failed after the background write failure");

    // flush() must surface the sticky failure...
    assert!(
        cached.flush().await.is_err(),
        "flush() must still report the terminal background failure"
    );

    // ...but inner.flush() must have run regardless (post-mortem direct
    // attempt) -- the regression this test guards against is a silent skip
    // of the inner durability op entirely.
    assert!(
        flush_called.load(Ordering::SeqCst),
        "inner.flush() must run even when a background write failed -- \
         skipping it silently drops every OTHER buffered write, not just \
         the one that failed"
    );
}
