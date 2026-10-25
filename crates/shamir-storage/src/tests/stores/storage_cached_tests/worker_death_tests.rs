//! ROUND 4 owner-death tests (RV-STORAGE-897437C3).
//!
//! Every test here targets the single FIFO owner task and its health
//! watch in `storage_cached.rs`. All synchronization is explicit
//! oneshot: an `entered` oneshot proves the owner is parked inside the
//! gated backing op, and a `release` oneshot (held by the test) lets it
//! proceed. `HANG` timeouts are ONLY hang detectors; blocked-ness and
//! readiness are proven with `poll!` and awaited `DbResult`s, never by
//! timeout expiration.

use crate::storage_cached::CachedStore;
use crate::storage_in_memory::InMemoryStore;
use crate::types::{KvOp, RecordKey, Store};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::Stream;
use std::pin::{pin, Pin};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;
use tokio::sync::oneshot;

/// 10 s outer bound: ONLY a hang detector, never part of the protocol.
const HANG: Duration = Duration::from_secs(10);

fn rk(b: &[u8]) -> RecordKey {
    RecordKey::from_slice(b)
}

/// Deterministically await the mock's `entered` oneshot: the owner has
/// entered (and parked in) the gated backing op.
async fn entered(rx: &mut oneshot::Receiver<()>) {
    tokio::time::timeout(HANG, rx)
        .await
        .expect("owner never entered the gated backing op")
        .expect("entered oneshot dropped");
}

type BatchStream =
    Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, crate::error::DbError>> + Send>>;

/// The FIRST `set` fires `entered_tx`, then parks on `release_rx` BEFORE
/// applying; `fail_sets`/`panic_sets` act after the release (so the async
/// ack is observably delivered while the backing op is parked).
/// `flush_calls`/`drain_calls` count durability ops reaching the backing.
struct DeathMock {
    inner: Arc<InMemoryStore>,
    entered_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    release_rx: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
    fail_sets: bool,
    panic_sets: bool,
    flush_calls: Arc<AtomicUsize>,
    drain_calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Store for DeathMock {
    async fn insert(&self, value: Bytes) -> crate::error::DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> crate::error::DbResult<bool> {
        let entered = { self.entered_tx.lock().unwrap().take() };
        if let Some(tx) = entered {
            let _ = tx.send(());
            let release = { self.release_rx.lock().unwrap().take() };
            if let Some(rx) = release {
                let _ = rx.await;
            }
        }
        if self.panic_sets {
            panic!("simulated backing-store panic");
        }
        if self.fail_sets {
            return Err(crate::error::DbError::Storage(
                "simulated set failure".into(),
            ));
        }
        self.inner.set(key, value).await
    }
    async fn remove(&self, key: RecordKey) -> crate::error::DbResult<bool> {
        self.inner.remove(key).await
    }
    async fn flush(&self) -> crate::error::DbResult<()> {
        self.flush_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.flush().await
    }
    async fn flush_drain(&self) -> crate::error::DbResult<()> {
        self.drain_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.flush_drain().await
    }
    async fn get(&self, key: RecordKey) -> crate::error::DbResult<Bytes> {
        self.inner.get(key).await
    }
    fn iter_stream(&self, batch_size: usize) -> BatchStream {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> BatchStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

// ============================================================================
// T6 — Owner abort with queued jobs: every waiter (already queued or
// submitting later) gets the sticky Err; each barrier still attempts the
// inner durability op directly (F1's fallback + the explicit flush both
// count).
// ============================================================================

#[tokio::test]
async fn rv_storage_897437c3_owner_abort_queued_jobs_fail_all_waiters_sticky() {
    let (entered_tx, mut entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let flush_calls = Arc::new(AtomicUsize::new(0));
    let drain_calls = Arc::new(AtomicUsize::new(0));
    let mock = Arc::new(DeathMock {
        inner: Arc::new(InMemoryStore::new()),
        entered_tx: Arc::new(Mutex::new(Some(entered_tx))),
        release_rx: Arc::new(Mutex::new(Some(release_rx))),
        fail_sets: false,
        panic_sets: false,
        flush_calls: flush_calls.clone(),
        drain_calls: drain_calls.clone(),
    });
    let store = Arc::new(
        CachedStore::new_async(mock.clone() as Arc<dyn Store>)
            .await
            .unwrap(),
    );
    let k = rk(b"k");

    // A parks inside the backing set. Independent revert caught: an owner
    // that acks only AFTER the backing await (sync-style) or an abort path
    // that loses queued replies makes this ack hang or return Err.
    let (ack_tx, ack_rx) = oneshot::channel();
    tokio::spawn({
        let s = store.clone();
        let k = k.clone();
        async move {
            let r = s.set(k, Bytes::from_static(b"A")).await;
            let _ = ack_tx.send(r);
        }
    });
    entered(&mut entered_rx).await; // owner parked in the gated backing set
    let ack = tokio::time::timeout(HANG, ack_rx)
        .await
        .expect("async ack hung")
        .expect("ack channel dropped");
    ack.expect("async set must ack Ok while the backing write is parked");

    // B, C, F1 and F2 are pinned and polled ONCE: Pending proves each is
    // QUEUED behind A's in-flight backing write (poll!, not a timeout).
    let mut b = pin!(store.set(k.clone(), Bytes::from_static(b"B")));
    assert!(matches!(futures::poll!(b.as_mut()), Poll::Pending));
    let mut c = pin!(store.transact(vec![KvOp::Set(k.clone(), Bytes::from_static(b"C"))]));
    assert!(matches!(futures::poll!(c.as_mut()), Poll::Pending));
    let mut f1 = pin!(store.flush());
    assert!(matches!(futures::poll!(f1.as_mut()), Poll::Pending));
    let mut f2 = pin!(store.flush_drain());
    assert!(matches!(futures::poll!(f2.as_mut()), Poll::Pending));

    // Kill the owner mid-await: the guard publishes the terminal failure.
    store.abort_owner_for_tests();
    drop(release_tx);

    // The HANG timeout is only a detector: each awaited future is the real
    // operation future, so the asserted value is its DbResult.
    let rb = tokio::time::timeout(HANG, b.as_mut())
        .await
        .unwrap_or_else(|_| panic!("queued waiter B hung"));
    assert!(rb.is_err(), "queued waiter B must fail after owner death");
    let rc = tokio::time::timeout(HANG, c.as_mut())
        .await
        .unwrap_or_else(|_| panic!("queued waiter C hung"));
    assert!(rc.is_err(), "queued waiter C must fail after owner death");
    let rf1 = tokio::time::timeout(HANG, f1.as_mut())
        .await
        .unwrap_or_else(|_| panic!("queued barrier F1 hung"));
    assert!(
        rf1.is_err(),
        "queued barrier F1 must fail after owner death"
    );
    let rf2 = tokio::time::timeout(HANG, f2.as_mut())
        .await
        .unwrap_or_else(|_| panic!("queued barrier F2 hung"));
    assert!(
        rf2.is_err(),
        "queued barrier F2 must fail after owner death"
    );

    // Sticky: later submissions and trusted reads also fail.
    assert!(store
        .set(k.clone(), Bytes::from_static(b"X"))
        .await
        .is_err());
    assert!(store.get(k.clone()).await.is_err());
    assert!(store.transact(vec![KvOp::Remove(k.clone())]).await.is_err());
    // The mirror is NOT swapped away.
    assert!(store.cache().peek_with(&k, |_, _| ()).is_some());

    // Post-mortem barriers still attempt the inner durability op directly:
    // F1's fallback counted 1, this explicit flush makes 2 (and likewise
    // for the drain). Independent revert caught: a flush that short-
    // circuits past `inner.flush()` on the sticky failure skips the inner
    // durability op and these counts stay at 1/1.
    assert!(store.flush().await.is_err());
    assert_eq!(
        flush_calls.load(Ordering::SeqCst),
        2,
        "F1 fallback + explicit flush must both reach inner"
    );
    assert!(store.flush_drain().await.is_err());
    assert_eq!(
        drain_calls.load(Ordering::SeqCst),
        2,
        "F2 fallback + explicit drain must both reach inner"
    );
}

// ============================================================================
// T7 — Abort BEFORE the owner's first poll (guard built before spawn):
// health goes sticky-terminal deterministically (awaited, not raced);
// barriers fail and each attempts the inner durability op.
// ============================================================================

#[tokio::test]
async fn rv_storage_897437c3_abort_before_first_poll_health_sticky_barriers_prompt() {
    let flush_calls = Arc::new(AtomicUsize::new(0));
    let drain_calls = Arc::new(AtomicUsize::new(0));
    let (entered_tx, entered_rx) = oneshot::channel();
    let (_release_tx, release_rx) = oneshot::channel();
    let mock = Arc::new(DeathMock {
        inner: Arc::new(InMemoryStore::new()),
        entered_tx: Arc::new(Mutex::new(Some(entered_tx))),
        release_rx: Arc::new(Mutex::new(Some(release_rx))),
        fail_sets: false,
        panic_sets: false,
        flush_calls: flush_calls.clone(),
        drain_calls: drain_calls.clone(),
    });
    // Paused-owner constructor: the owner is spawned behind a gate and, on
    // the current-thread runtime below, is never polled before we abort.
    let (store, _pause) =
        CachedStore::new_sync_paused_owner_for_tests(mock.clone() as Arc<dyn Store>)
            .await
            .unwrap();
    store.abort_owner_for_tests();
    drop(entered_rx); // never fired: the owner is never polled

    // Deterministic terminal wait (NOT an elapsed-time assertion).
    // EXACT REVERT (this test's own card, RV-STORAGE-897437C3): terminal
    // publication after a pre-first-poll abort is protected by TWO
    // independent publishers, so this wait only fails when BOTH are
    // removed together — the pre-spawn `OwnerGuard` (built before
    // `tokio::spawn`, dropped when the never-polled owner future is
    // dropped) AND the `supervise_owner` JoinHandle publisher. Reverting
    // the guard ALONE does NOT fail this test: the supervisor still
    // publishes the sticky terminal failure when the aborted handle is
    // awaited — the card is protected in depth.
    tokio::time::timeout(HANG, store.await_health_terminal_for_tests())
        .await
        .expect("health never went terminal after pre-poll abort");

    let k = rk(b"k");
    assert!(
        store.get(k.clone()).await.is_err(),
        "read must be sticky-Err"
    );
    let res = tokio::time::timeout(HANG, store.flush())
        .await
        .expect("flush hung after pre-poll abort");
    assert!(res.is_err(), "flush must report the terminal failure");
    assert_eq!(
        flush_calls.load(Ordering::SeqCst),
        1,
        "flush must still attempt inner"
    );
    assert!(tokio::time::timeout(HANG, store.flush_drain())
        .await
        .expect("drain hung")
        .is_err());
    assert_eq!(
        drain_calls.load(Ordering::SeqCst),
        1,
        "drain must still attempt inner"
    );
    assert!(store.set(k, Bytes::from_static(b"v")).await.is_err());
}

// ============================================================================
// T8 — Backing error AFTER the async ack: the store becomes terminally
// failed; multiple waiters and later submissions all see the sticky Err.
// ============================================================================

#[tokio::test]
async fn rv_storage_897437c3_backing_error_after_async_ack_sticky_multiple_waiters() {
    let flush_calls = Arc::new(AtomicUsize::new(0));
    let drain_calls = Arc::new(AtomicUsize::new(0));
    let (entered_tx, mut entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let mock = Arc::new(DeathMock {
        inner: Arc::new(InMemoryStore::new()),
        entered_tx: Arc::new(Mutex::new(Some(entered_tx))),
        release_rx: Arc::new(Mutex::new(Some(release_rx))),
        fail_sets: true,
        panic_sets: false,
        flush_calls: flush_calls.clone(),
        drain_calls: drain_calls.clone(),
    });
    let store = Arc::new(
        CachedStore::new_async(mock as Arc<dyn Store>)
            .await
            .unwrap(),
    );
    let k = rk(b"k");

    // The async ack itself succeeds (tentative cache publication); the
    // backing failure stays parked behind the release oneshot until the
    // ack is OBSERVABLY delivered. Independent revert caught: an owner
    // that acks after the backing await (or drops the ack on failure)
    // makes this assert fail before the gate is ever released.
    let (ack_tx, ack_rx) = oneshot::channel();
    tokio::spawn({
        let s = store.clone();
        let k = k.clone();
        async move {
            let r = s.set(k, Bytes::from_static(b"A")).await;
            let _ = ack_tx.send(r);
        }
    });
    entered(&mut entered_rx).await;
    let ack = tokio::time::timeout(HANG, ack_rx)
        .await
        .expect("async ack hung")
        .expect("caller dropped ack channel");
    ack.expect("async ack must succeed before the gated backing failure");
    assert!(store.cache().peek_with(&k, |_, _| ()).is_some());
    release_tx.send(()).expect("release oneshot closed");

    // Deterministic publication wait: no racing the health watch.
    tokio::time::timeout(HANG, store.await_health_terminal_for_tests())
        .await
        .expect("backing failure never published");

    // Multiple waiters, each awaited as its actual operation future (no
    // JoinHandle indirection): all get the sticky Err.
    let f1 = pin!(store.flush());
    let r1 = tokio::time::timeout(HANG, f1)
        .await
        .expect("flush waiter hung");
    assert!(r1.is_err(), "flush waiter must see the sticky Err");
    let f2 = pin!(store.flush_drain());
    let r2 = tokio::time::timeout(HANG, f2)
        .await
        .expect("drain waiter hung");
    assert!(r2.is_err(), "drain waiter must see the sticky Err");
    let w = pin!(store.set(k.clone(), Bytes::from_static(b"B")));
    let rw = tokio::time::timeout(HANG, w)
        .await
        .expect("queued writer hung");
    assert!(rw.is_err(), "queued writer must see the sticky Err");
    assert!(store.get(k.clone()).await.is_err());
    assert!(store.remove(k.clone()).await.is_err());
    assert!(
        store.cache().peek_with(&k, |_, _| ()).is_some(),
        "no swap(None)"
    );

    // Post-mortem barriers still attempt the inner durability op directly;
    // F1/F2's fallbacks already counted one each, so the explicit calls
    // bring both counters to 2.
    assert!(store.flush().await.is_err());
    assert_eq!(flush_calls.load(Ordering::SeqCst), 2);
    assert!(store.flush_drain().await.is_err());
    assert_eq!(drain_calls.load(Ordering::SeqCst), 2);
}

// ============================================================================
// Legacy regression (RV-STORAGE-897437C3 original card): a PANICKING
// backing store inside the owner must surface as a sticky Err from
// `flush()`, not hang or report Ok. The ack is OBSERVABLY delivered while
// the panic is gated behind the release oneshot; the panic path itself is
// awaited deterministically via the terminal-health helper.
// ============================================================================

#[tokio::test]
async fn rv_storage_897437c3_flush_errors_when_owner_panics() {
    let flush_calls = Arc::new(AtomicUsize::new(0));
    let drain_calls = Arc::new(AtomicUsize::new(0));
    let (entered_tx, mut entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let mock = Arc::new(DeathMock {
        inner: Arc::new(InMemoryStore::new()),
        entered_tx: Arc::new(Mutex::new(Some(entered_tx))),
        release_rx: Arc::new(Mutex::new(Some(release_rx))),
        fail_sets: false,
        panic_sets: true,
        flush_calls: flush_calls.clone(),
        drain_calls: drain_calls.clone(),
    });
    let store = Arc::new(
        CachedStore::new_async(mock as Arc<dyn Store>)
            .await
            .unwrap(),
    );
    let k = rk(b"k");

    // Cache updated, job accepted; the owner is parked at the panic gate.
    // Independent revert caught: an owner that acks after the backing
    // await propagates the panic to the caller (JoinError) instead of this
    // Ok ack.
    let (ack_tx, ack_rx) = oneshot::channel();
    tokio::spawn({
        let s = store.clone();
        let k = k.clone();
        async move {
            let r = s.set(k, Bytes::from_static(b"v")).await;
            let _ = ack_tx.send(r);
        }
    });
    entered(&mut entered_rx).await;
    let ack = tokio::time::timeout(HANG, ack_rx)
        .await
        .expect("async ack hung")
        .expect("caller dropped ack channel");
    ack.expect("async ack must succeed before the gated panic");

    // Trigger the panic, then await the terminal publication
    // deterministically (no timeout-expiration "evidence").
    release_tx.send(()).expect("release oneshot closed");
    tokio::time::timeout(HANG, store.await_health_terminal_for_tests())
        .await
        .expect("owner panic never published a terminal failure");

    let res = tokio::time::timeout(HANG, store.flush())
        .await
        .expect("flush must not hang after owner panic");
    assert!(
        res.is_err(),
        "flush must report the dead owner, not Ok: {:?}",
        res
    );
    // Sticky, not consumed-once.
    assert!(store.flush().await.is_err());
    assert_eq!(
        flush_calls.load(Ordering::SeqCst),
        2,
        "both post-mortem flushes must still attempt inner"
    );
}
