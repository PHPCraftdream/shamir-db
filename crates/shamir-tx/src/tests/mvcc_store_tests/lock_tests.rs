use super::helpers::make_mvcc;
use crate::mvcc_store::LockMode;
use bytes::Bytes;
use shamir_storage::error::DbError;
use shamir_storage::types::RecordKey;
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
        RecordKey::from(key.clone()),
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
        RecordKey::from(key.clone()),
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
        RecordKey::from(key.clone()),
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
            RecordKey::from(key.clone()),
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
    // Note: the single `yield_now().await` calls below (lines ~165, ~178) are
    // NOT spin-wait loops — they are one-shot yields to increase interleaving
    // odds. The entire (t1, t2) join is wrapped in a 3 s `tokio::time::timeout`
    // at line ~185, so even if `lock_key` deadlocks (the wound-wait hazard
    // this test exercises), the failure is a FAST named assertion, not an
    // anonymous nextest TIMEOUT.
    let t1_handle = tokio::spawn(async move {
        let (f1, n1, f2, n2) = t1;
        mvcc1
            .lock_key(RecordKey::from(key_a1), 1, f1, n1, LockMode::Exclusive)
            .await
            .unwrap();
        tokio::task::yield_now().await;
        mvcc1
            .lock_key(RecordKey::from(key_b1), 1, f2, n2, LockMode::Exclusive)
            .await
    });

    // T2: lock B (Exclusive), then A (Exclusive). Same wound/notify.
    let t2_handle = tokio::spawn(async move {
        let (f1, n1, f2, n2) = t2;
        mvcc2
            .lock_key(RecordKey::from(key_b2), 2, f1, n1, LockMode::Exclusive)
            .await
            .unwrap();
        tokio::task::yield_now().await;
        mvcc2
            .lock_key(RecordKey::from(key_a2), 2, f2, n2, LockMode::Exclusive)
            .await
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
    mvcc.lock_key(
        RecordKey::from(key.clone()),
        42,
        w.flag(),
        w.notify(),
        LockMode::Shared,
    )
    .await
    .unwrap();
    // Re-acquire Exclusive (upgrade) — same tx, must not deadlock.
    mvcc.lock_key(
        RecordKey::from(key.clone()),
        42,
        w.flag(),
        w.notify(),
        LockMode::Exclusive,
    )
    .await
    .unwrap();
    // And again — idempotent.
    mvcc.lock_key(
        RecordKey::from(key.clone()),
        42,
        w.flag(),
        w.notify(),
        LockMode::Exclusive,
    )
    .await
    .unwrap();

    // The tx still holds exactly one holder entry (no duplicates).
    let lock = mvcc
        .locks
        .get_sync(key.as_ref())
        .map(|e| Arc::clone(e.get()))
        .unwrap();
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
    mvcc.lock_key(
        RecordKey::from(key_a.clone()),
        7,
        w.flag(),
        w.notify(),
        LockMode::Exclusive,
    )
    .await
    .unwrap();
    mvcc.lock_key(
        RecordKey::from(key_b.clone()),
        7,
        w.flag(),
        w.notify(),
        LockMode::Shared,
    )
    .await
    .unwrap();

    // Confirm held.
    let la = mvcc
        .locks
        .get_sync(key_a.as_ref())
        .map(|e| Arc::clone(e.get()))
        .unwrap();
    {
        let s = la.state.lock().await;
        assert_eq!(s.holders.len(), 1);
        assert_eq!(s.mode, Some(LockMode::Exclusive));
    }

    // Release (as commit/abort would).
    mvcc.release_locks(
        7,
        &[
            RecordKey::from(key_a.clone()),
            RecordKey::from(key_b.clone()),
        ],
    )
    .await;

    // Both keys now empty.
    {
        let s = la.state.lock().await;
        assert!(s.holders.is_empty(), "holders must be empty after release");
        assert_eq!(s.mode, None, "mode must be None after release");
    }
    let lb = mvcc
        .locks
        .get_sync(key_b.as_ref())
        .map(|e| Arc::clone(e.get()))
        .unwrap();
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
    mvcc.set_versioned(RecordKey::from(Bytes::from("z")), Bytes::from("v"))
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
    mvcc.lock_key(
        RecordKey::from(key.clone()),
        1,
        t1.flag(),
        t1.notify(),
        LockMode::Shared,
    )
    .await
    .unwrap();
    // T2 (version 2) Shared — compatible, both hold.
    let t2 = TxWound::new();
    mvcc.lock_key(
        RecordKey::from(key.clone()),
        2,
        t2.flag(),
        t2.notify(),
        LockMode::Shared,
    )
    .await
    .unwrap();

    let lock = mvcc
        .locks
        .get_sync(key.as_ref())
        .map(|e| Arc::clone(e.get()))
        .unwrap();
    {
        let s = lock.state.lock().await;
        assert_eq!(s.holders.len(), 2, "two Shared holders");
        assert_eq!(s.mode, Some(LockMode::Shared));
    }

    // T0 (version 0, OLDEST) Exclusive → wounds both younger Shared
    // holders and acquires.
    let t0 = TxWound::new();
    mvcc.lock_key(
        RecordKey::from(key.clone()),
        0,
        t0.flag(),
        t0.notify(),
        LockMode::Exclusive,
    )
    .await
    .unwrap();
    assert!(t1.is_wounded(), "younger Shared holder T1 wounded");
    assert!(t2.is_wounded(), "younger Shared holder T2 wounded");
    let lock = mvcc
        .locks
        .get_sync(key.as_ref())
        .map(|e| Arc::clone(e.get()))
        .unwrap();
    let s = lock.state.lock().await;
    assert_eq!(
        s.holders.len(),
        1,
        "older Exclusive wounds younger Shared holders"
    );
    assert_eq!(s.holders[0].tx_version, 0);
    assert_eq!(s.mode, Some(LockMode::Exclusive));
}

/// Audit finding A6 — re-entrant Shared→Exclusive UPGRADE must not be
/// granted instantly when OTHER transactions also hold the key Shared.
///
/// Interleaving under the BUG: T1 and T2 both hold Shared on `k`; T1
/// (older) calls `lock_key(k, Exclusive)`. The buggy `compatible` gate
/// short-circuits on `re_entrant` alone (T1 already holds Shared), sets
/// `mode = Exclusive`, but leaves T2's holder entry in place → both txs
/// believe they hold Exclusive simultaneously (single-writer invariant
/// "Exclusive ⇒ exactly one holder" violated).
///
/// Post-fix: T1's Exclusive-upgrade request must be treated as
/// INCOMPATIBLE (because T2 — a different tx — still holds Shared), fall
/// into the wound-wait partition logic, WOUND T2 (younger), remove T2's
/// holder, and only THEN grant — leaving `holders == [T1]`,
/// `mode == Exclusive`.
#[tokio::test]
async fn lock_key_a6_older_upgrade_wounds_younger_shared_holder() {
    let mvcc = make_mvcc();
    let key = Bytes::from("a6_old");

    // T1 (version 1, OLDER) acquires Shared.
    let t1 = TxWound::new();
    mvcc.lock_key(
        RecordKey::from(key.clone()),
        1,
        t1.flag(),
        t1.notify(),
        LockMode::Shared,
    )
    .await
    .unwrap();
    // T2 (version 2, YOUNGER) acquires Shared — compatible, both hold.
    let t2 = TxWound::new();
    mvcc.lock_key(
        RecordKey::from(key.clone()),
        2,
        t2.flag(),
        t2.notify(),
        LockMode::Shared,
    )
    .await
    .unwrap();

    // Sanity: two Shared holders, mode Shared.
    {
        let lock = mvcc
            .locks
            .get_sync(key.as_ref())
            .map(|e| Arc::clone(e.get()))
            .unwrap();
        let s = lock.state.lock().await;
        assert_eq!(s.holders.len(), 2);
        assert_eq!(s.mode, Some(LockMode::Shared));
    }

    // T1 (older) requests the Shared→Exclusive UPGRADE. Must WOUND T2,
    // remove T2's holder, and grant — NOT return instantly while T2 still
    // holds Shared.
    mvcc.lock_key(
        RecordKey::from(key.clone()),
        1,
        t1.flag(),
        t1.notify(),
        LockMode::Exclusive,
    )
    .await
    .expect("older tx upgrade must succeed after wounding younger holder");

    // T2 (younger Shared holder) must have been wounded.
    assert!(
        t2.is_wounded(),
        "older tx upgrade must wound the younger Shared holder"
    );
    assert!(!t1.is_wounded(), "older tx must not wound itself");

    // Invariant restored: exactly one holder (T1), mode Exclusive.
    let lock = mvcc
        .locks
        .get_sync(key.as_ref())
        .map(|e| Arc::clone(e.get()))
        .unwrap();
    let s = lock.state.lock().await;
    assert_eq!(
        s.holders.len(),
        1,
        "A6: Exclusive upgrade must leave exactly one holder (no other Shared holders)"
    );
    assert_eq!(s.holders[0].tx_version, 1);
    assert_eq!(s.mode, Some(LockMode::Exclusive));

    // Observable-behavior cross-check: a THIRD tx (T3) attempting Shared
    // on the key must now CONFLICT (T1 holds a genuine single-holder
    // Exclusive). T3 is younger than T1, so it must WAIT, not acquire.
    let t3 = TxWound::new();
    let t3_acquired = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        mvcc.lock_key(
            RecordKey::from(key.clone()),
            3,
            t3.flag(),
            t3.notify(),
            LockMode::Shared,
        ),
    )
    .await;
    assert!(
        t3_acquired.is_err(),
        "T3 Shared must block while T1 holds genuine Exclusive (proves the invariant)"
    );
}

/// Audit finding A6 — symmetric case: a YOUNGER requester upgrading
/// Shared→Exclusive while an OLDER tx still holds Shared must WAIT
/// (per wound-wait's age rule), not be granted instantly.
///
/// Under the BUG, T2's upgrade would short-circuit to `compatible = true`
/// via `re_entrant` and grant immediately even though T1 (older) still
/// holds Shared. Post-fix, T2 must enter the wound-wait partition logic
/// and — because T1 is strictly OLDER — choose WAIT (it cannot wound
/// T1). The test asserts the wait by wrapping the upgrade in a bounded
/// timeout: if the buggy instant-grant fires, the future resolves Ok and
/// the timeout succeeds → test fails; if T2 correctly waits, the timeout
/// elapses → test passes.
#[tokio::test]
async fn lock_key_a6_younger_upgrade_waits_for_older_shared_holder() {
    let mvcc = make_mvcc();
    let key = Bytes::from("a6_young");

    // T1 (version 1, OLDER) acquires Shared.
    let t1 = TxWound::new();
    mvcc.lock_key(
        RecordKey::from(key.clone()),
        1,
        t1.flag(),
        t1.notify(),
        LockMode::Shared,
    )
    .await
    .unwrap();
    // T2 (version 2, YOUNGER) acquires Shared — compatible.
    let t2 = TxWound::new();
    mvcc.lock_key(
        RecordKey::from(key.clone()),
        2,
        t2.flag(),
        t2.notify(),
        LockMode::Shared,
    )
    .await
    .unwrap();

    // T2 (younger) requests Shared→Exclusive upgrade. T1 (older) still
    // holds Shared, so T2 must WAIT. Bounded by a timeout: a buggy
    // instant-grant resolves Ok (test fails); a correct wait elapses.
    let upgrade = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        mvcc.lock_key(
            RecordKey::from(key.clone()),
            2,
            t2.flag(),
            t2.notify(),
            LockMode::Exclusive,
        ),
    )
    .await;
    assert!(
        upgrade.is_err(),
        "A6: younger tx upgrade must WAIT while older tx holds Shared, not acquire instantly"
    );
    // The older holder is NOT wounded by the younger requester.
    assert!(
        !t1.is_wounded(),
        "younger requester must not wound the older Shared holder"
    );
    // T2 is not wounded either (nobody wounded it).
    assert!(!t2.is_wounded());
}

/// Audit finding A6 — regression guard: the COMMON correct case — a
/// single tx acquiring Shared then upgrading to Exclusive on a key IT
/// ALONE holds (no other concurrent holder) — must still get an INSTANT
/// grant. The fix narrows the re-entrant short-circuit only for the
/// "other holders present" sub-case; the solo-upgrade fast path must be
/// preserved.
#[tokio::test]
async fn lock_key_a6_solo_upgrade_no_other_holders_fast_path() {
    let mvcc = make_mvcc();
    let key = Bytes::from("a6_solo");
    let w = TxWound::new();

    // Solo Shared acquire.
    mvcc.lock_key(
        RecordKey::from(key.clone()),
        42,
        w.flag(),
        w.notify(),
        LockMode::Shared,
    )
    .await
    .unwrap();

    // Solo Shared→Exclusive upgrade — no other holders, must be instant.
    let upgrade = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        mvcc.lock_key(
            RecordKey::from(key.clone()),
            42,
            w.flag(),
            w.notify(),
            LockMode::Exclusive,
        ),
    )
    .await
    .expect("solo upgrade with no other holders must grant instantly (no wait)");
    upgrade.expect("solo upgrade must succeed");

    // Invariant holds: exactly one holder, mode Exclusive.
    let lock = mvcc
        .locks
        .get_sync(key.as_ref())
        .map(|e| Arc::clone(e.get()))
        .unwrap();
    let s = lock.state.lock().await;
    assert_eq!(s.holders.len(), 1);
    assert_eq!(s.holders[0].tx_version, 42);
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
        RecordKey::from(key.clone()),
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
            .lock_key(
                RecordKey::from(key_c),
                2,
                yw_flag,
                yw_notify,
                LockMode::Exclusive,
            )
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
