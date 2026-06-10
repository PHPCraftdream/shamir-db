use super::helpers::make_mvcc;
use crate::mvcc_store::LockMode;
use bytes::Bytes;
use shamir_storage::error::DbError;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// ----------------------------------------------------------------
// S2 — Level-3 pessimistic locking (wound-wait).
// ----------------------------------------------------------------

/// Test bundle: a tx's wound flag + wake notify. Each test tx gets one
/// and passes clones into every `lock_key` call so a wound issued on
/// one key wakes a wait parked on another.
struct TxWound {
    wounded: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl TxWound {
    fn new() -> Self {
        Self {
            wounded: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(tokio::sync::Notify::new()),
        }
    }
    fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.wounded)
    }
    fn notify(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.notify)
    }
    fn is_wounded(&self) -> bool {
        self.wounded.load(Ordering::Acquire)
    }
    fn set_wounded(&self) {
        self.wounded.store(true, Ordering::Release);
    }
}

/// Wound-wait basic: an OLDER tx (smaller version) requesting a
/// conflicting lock WOUNDS the younger holder (younger's `wounded`
/// becomes true, older acquires). A YOUNGER requester against an older
/// holder WAITS (asserted via timeout).
#[tokio::test]
async fn lock_key_wound_wait_basic() {
    let mvcc = make_mvcc();
    let key = Bytes::from("lk");

    // Younger tx (version 20) holds Exclusive.
    let younger = TxWound::new();
    mvcc.lock_key(
        key.clone(),
        20,
        younger.flag(),
        younger.notify(),
        LockMode::Exclusive,
    )
    .await
    .unwrap();
    assert!(!younger.is_wounded());

    // Older tx (version 10) requests Exclusive → must WOUND the younger
    // holder and acquire immediately.
    let older = TxWound::new();
    mvcc.lock_key(
        key.clone(),
        10,
        older.flag(),
        older.notify(),
        LockMode::Exclusive,
    )
    .await
    .unwrap();
    assert!(
        younger.is_wounded(),
        "older tx must wound the younger holder"
    );
    assert!(!older.is_wounded());
}

/// A younger requester against an older holder WAITS — it must not
/// acquire while the older holds the lock. Bounded by a timeout so a
/// bug (e.g. acquiring anyway) fails the test instead of hanging.
#[tokio::test]
async fn lock_key_younger_waits_for_older() {
    let mvcc = make_mvcc();
    let key = Bytes::from("wait");

    // Older tx (version 5) holds Exclusive.
    let older = TxWound::new();
    mvcc.lock_key(
        key.clone(),
        5,
        older.flag(),
        older.notify(),
        LockMode::Exclusive,
    )
    .await
    .unwrap();

    // Younger tx (version 9) requests Exclusive → must WAIT. Wrap in a
    // timeout: if it acquired (bug) the test fails; if it correctly
    // waits, the timeout fires.
    let younger = TxWound::new();
    let wait_future = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        mvcc.lock_key(
            key.clone(),
            9,
            younger.flag(),
            younger.notify(),
            LockMode::Exclusive,
        ),
    )
    .await;
    assert!(
        wait_future.is_err(),
        "younger tx must WAIT on older holder (timeout expected, not acquisition)"
    );
    assert!(
        !younger.is_wounded(),
        "younger waiter must not be wounded by the older holder"
    );
}

/// Deadlock-freedom: two Level-3 txs lock keys in OPPOSITE order (T1:
/// A then B; T2: B then A) concurrently. Both runs must terminate
/// (bounded by a generous timeout) and neither deadlocks. This is the
/// core invariant: wound-wait on the total version order cannot cycle.
#[tokio::test]
async fn lock_key_deadlock_freedom_opposite_order() {
    let mvcc = Arc::new(make_mvcc());
    let key_a = Bytes::from("deadlock_a");
    let key_b = Bytes::from("deadlock_b");

    // T1 = version 1 (older), T2 = version 2 (younger). T1 has higher
    // priority. When they conflict, T2 gets wounded and T1 proceeds.
    // Each tx uses ONE TxWound across all its lock_key calls so a wound
    // issued on one key wakes a wait parked on another (mirrors the
    // real TxContext.wound_notify invariant).
    let t1 = TxWound::new();
    let t2 = TxWound::new();

    // Clone the flag/notify TWICE per tx (one per lock_key call) so
    // both calls share the same underlying Arcs.
    let t1 = (t1.flag(), t1.notify(), t1.flag(), t1.notify());
    let t2 = (t2.flag(), t2.notify(), t2.flag(), t2.notify());

    let mvcc1 = Arc::clone(&mvcc);
    let mvcc2 = Arc::clone(&mvcc);
    let key_a1 = key_a.clone();
    let key_b1 = key_b.clone();
    let key_a2 = key_a.clone();
    let key_b2 = key_b.clone();

    // T1: lock A (Exclusive), then B (Exclusive). Same wound/notify.
    let t1_handle = tokio::spawn(async move {
        let (f1, n1, f2, n2) = t1;
        mvcc1
            .lock_key(key_a1, 1, f1, n1, LockMode::Exclusive)
            .await
            .unwrap();
        tokio::task::yield_now().await;
        mvcc1.lock_key(key_b1, 1, f2, n2, LockMode::Exclusive).await
    });

    // T2: lock B (Exclusive), then A (Exclusive). Same wound/notify.
    let t2_handle = tokio::spawn(async move {
        let (f1, n1, f2, n2) = t2;
        mvcc2
            .lock_key(key_b2, 2, f1, n1, LockMode::Exclusive)
            .await
            .unwrap();
        tokio::task::yield_now().await;
        mvcc2.lock_key(key_a2, 2, f2, n2, LockMode::Exclusive).await
    });

    // Bound with a generous timeout: a real deadlock hangs CI and fails.
    let (r1, r2) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        (t1_handle.await.unwrap(), t2_handle.await.unwrap())
    })
    .await
    .expect("deadlock-freedom: both txs must terminate within timeout");

    // At least one completes by wounding/serialization. T2 (younger) is
    // the one that gets wounded when it tries to take A (held by T1):
    // a wound means T2's second lock_key returns Err. T1 (older) wounds
    // T2 and succeeds.
    let _ = r1; // T1 result
    let _ = r2; // T2 result (may be Ok or Err depending on interleaving)
}

/// Re-entrant: the same tx_version acquiring the same key twice (e.g.
/// read then write) does NOT self-deadlock. A Shared acquire followed
/// by an Exclusive acquire for the same tx succeeds.
#[tokio::test]
async fn lock_key_reentrant_same_tx_no_self_deadlock() {
    let mvcc = make_mvcc();
    let key = Bytes::from("reent");
    let w = TxWound::new();

    // First acquire Shared.
    mvcc.lock_key(key.clone(), 42, w.flag(), w.notify(), LockMode::Shared)
        .await
        .unwrap();
    // Re-acquire Exclusive (upgrade) — same tx, must not deadlock.
    mvcc.lock_key(key.clone(), 42, w.flag(), w.notify(), LockMode::Exclusive)
        .await
        .unwrap();
    // And again — idempotent.
    mvcc.lock_key(key.clone(), 42, w.flag(), w.notify(), LockMode::Exclusive)
        .await
        .unwrap();

    // The tx still holds exactly one holder entry (no duplicates).
    let lock = mvcc.locks.get(&key).map(|e| Arc::clone(e.get())).unwrap();
    let state = lock.state.lock().await;
    assert_eq!(
        state.holders.len(),
        1,
        "re-entrant re-acquire must not duplicate holders"
    );
    assert_eq!(state.holders[0].tx_version, 42);
    assert_eq!(state.mode, Some(LockMode::Exclusive));
}

/// Release on commit and on abort: after a Level-3 tx's locks are
/// released, the holders are empty (mode None). Both the commit path
/// and the abort path call `release_locks`.
#[tokio::test]
async fn release_locks_clears_holders() {
    let mvcc = make_mvcc();
    let key_a = Bytes::from("rel_a");
    let key_b = Bytes::from("rel_b");
    let w = TxWound::new();

    // Acquire Exclusive on both keys.
    mvcc.lock_key(key_a.clone(), 7, w.flag(), w.notify(), LockMode::Exclusive)
        .await
        .unwrap();
    mvcc.lock_key(key_b.clone(), 7, w.flag(), w.notify(), LockMode::Shared)
        .await
        .unwrap();

    // Confirm held.
    let la = mvcc.locks.get(&key_a).map(|e| Arc::clone(e.get())).unwrap();
    {
        let s = la.state.lock().await;
        assert_eq!(s.holders.len(), 1);
        assert_eq!(s.mode, Some(LockMode::Exclusive));
    }

    // Release (as commit/abort would).
    mvcc.release_locks(7, &[key_a.clone(), key_b.clone()]).await;

    // Both keys now empty.
    {
        let s = la.state.lock().await;
        assert!(s.holders.is_empty(), "holders must be empty after release");
        assert_eq!(s.mode, None, "mode must be None after release");
    }
    let lb = mvcc.locks.get(&key_b).map(|e| Arc::clone(e.get())).unwrap();
    {
        let s = lb.state.lock().await;
        assert!(s.holders.is_empty());
        assert_eq!(s.mode, None);
    }
}

/// Zero-overhead invariant: a Snapshot and a Serializable tx never
/// populate `locks`. The locks registry stays empty when no Level-3
/// lock is acquired (the snapshot/serializable paths never call
/// `lock_key`). This is verified at the MvccStore level: regular
/// set_versioned/get_at leave `locks` untouched.
#[tokio::test]
async fn locks_registry_empty_without_pessimistic_acquire() {
    let mvcc = make_mvcc();
    // Snapshot-style writes (no lock_key calls).
    mvcc.set_versioned(Bytes::from("z"), Bytes::from("v"))
        .await
        .unwrap();
    let _ = mvcc.get_at(b"z", 0).await.unwrap();
    assert_eq!(
        mvcc.locks_len(),
        0,
        "locks registry must stay empty without an explicit Level-3 acquire"
    );
}

/// Shared+Shared compatibility: two DISTINCT txs can both hold Shared
/// on the same key (multiple readers). A third Exclusive request
/// conflicts and wounds the younger Shared holders.
#[tokio::test]
async fn lock_key_shared_shared_compatible() {
    let mvcc = make_mvcc();
    let key = Bytes::from("ss");

    // T1 (version 1) Shared.
    let t1 = TxWound::new();
    mvcc.lock_key(key.clone(), 1, t1.flag(), t1.notify(), LockMode::Shared)
        .await
        .unwrap();
    // T2 (version 2) Shared — compatible, both hold.
    let t2 = TxWound::new();
    mvcc.lock_key(key.clone(), 2, t2.flag(), t2.notify(), LockMode::Shared)
        .await
        .unwrap();

    let lock = mvcc.locks.get(&key).map(|e| Arc::clone(e.get())).unwrap();
    {
        let s = lock.state.lock().await;
        assert_eq!(s.holders.len(), 2, "two Shared holders");
        assert_eq!(s.mode, Some(LockMode::Shared));
    }

    // T0 (version 0, OLDEST) Exclusive → wounds both younger Shared
    // holders and acquires.
    let t0 = TxWound::new();
    mvcc.lock_key(key.clone(), 0, t0.flag(), t0.notify(), LockMode::Exclusive)
        .await
        .unwrap();
    assert!(t1.is_wounded(), "younger Shared holder T1 wounded");
    assert!(t2.is_wounded(), "younger Shared holder T2 wounded");
    let lock = mvcc.locks.get(&key).map(|e| Arc::clone(e.get())).unwrap();
    let s = lock.state.lock().await;
    assert_eq!(
        s.holders.len(),
        1,
        "older Exclusive wounds younger Shared holders"
    );
    assert_eq!(s.holders[0].tx_version, 0);
    assert_eq!(s.mode, Some(LockMode::Exclusive));
}

/// When a tx is wounded while WAITING (on a different key than where
/// the wound is issued), its `lock_key` returns `DbError::Conflict`
/// instead of acquiring. This exercises the per-tx `wound_notify`:
/// the wound is triggered via the flag + the tx's own notify, waking
/// it from a wait parked on the key's notify.
#[tokio::test]
async fn lock_key_wounded_waiter_aborts() {
    let mvcc = Arc::new(make_mvcc());
    let key = Bytes::from("wabort");

    // Older tx (version 1) holds Exclusive.
    let older = TxWound::new();
    mvcc.lock_key(
        key.clone(),
        1,
        older.flag(),
        older.notify(),
        LockMode::Exclusive,
    )
    .await
    .unwrap();

    // Younger tx (version 2) starts waiting.
    let younger = TxWound::new();
    let younger_notify = younger.notify();
    let mvcc_c = Arc::clone(&mvcc);
    let key_c = key.clone();
    let yw_flag = younger.flag();
    let yw_notify = younger.notify();
    let wait = tokio::spawn(async move {
        mvcc_c
            .lock_key(key_c, 2, yw_flag, yw_notify, LockMode::Exclusive)
            .await
    });

    // Give the waiter a chance to park, then wound it via its own
    // notify (simulating a wound issued on a DIFFERENT key).
    tokio::task::yield_now().await;
    younger.set_wounded();
    younger_notify.notify_one();

    let result = tokio::time::timeout(std::time::Duration::from_secs(2), wait)
        .await
        .expect("wounded waiter must return (not hang)")
        .unwrap();

    assert!(
        matches!(result, Err(DbError::Conflict(_))),
        "wounded waiter must return Conflict error, got {:?}",
        result
    );
}
