use crate::mvcc_store::MvccStore;
use crate::repo_tx_gate::RepoTxGate;
use crate::tests::mvcc_store_tests::gc::helpers::make_gate;
use crate::tests::mvcc_store_tests::gc::test_stores::make_failing_history_mvcc;
use crate::version_codec::encode_version_key;
use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::error::DbError;
use shamir_storage::types::RecordKey;
use shamir_storage::types::Store;
use std::sync::atomic::Ordering;
use std::sync::Arc;

// ================================================================
// Regression tests — I/O error propagation (fault injection).
// ================================================================

/// Regression: `delete_versioned` propagates `history.set()` errors.
///
/// The log is the sole durable write. If `history.set()` (the tombstone
/// write) fails, the error propagates and the delete is treated as if it
/// never happened — no stale cell, caller sees `Err`.
#[tokio::test]
async fn delete_versioned_propagates_remove_error() {
    let gate = make_gate();
    let (mvcc, history) = make_failing_history_mvcc(gate.clone());

    // Arm: the next `set` call on history will fail (tombstone write).
    history.fail_set.store(true, Ordering::Relaxed);

    let result = mvcc
        .delete_versioned(RecordKey::from(Bytes::from("k")))
        .await;
    assert!(
        result.is_err(),
        "delete_versioned must propagate history.set() I/O error"
    );
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("injected"),
        "error should be the injected fault, got: {err_msg}"
    );
}

/// Regression: `set_versioned` propagates `history.set()` errors.
///
/// The log is the sole durable write. If `history.set()` fails, the error
/// propagates and no version is committed.
#[tokio::test]
async fn set_versioned_propagates_archive_read_error() {
    let gate = make_gate();
    let (mvcc, history) = make_failing_history_mvcc(gate.clone());

    // Arm: the next `set` call on history will fail (log write).
    history.fail_set.store(true, Ordering::Relaxed);

    let result = mvcc
        .set_versioned(RecordKey::from(Bytes::from("k")), Bytes::from("new_val"))
        .await;
    assert!(
        result.is_err(),
        "set_versioned must propagate history.set() I/O error"
    );
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("injected"),
        "error should be the injected fault, got: {err_msg}"
    );

    // Nothing was written — get_current returns None.
    history.fail_set.store(false, Ordering::Relaxed);
    let via_seam = mvcc
        .get_current(RecordKey::from(Bytes::from("k")))
        .await
        .unwrap();
    assert!(
        via_seam.is_none(),
        "FINAL-A: key must be absent when history.set() fails"
    );
}

/// Regression: `delete_versioned` propagates `history.set()` (tombstone).
///
/// Mirrors `set_versioned_propagates_archive_read_error` for the delete path.
/// The caller sees `Err` when the log write fails — the error is not swallowed.
#[tokio::test]
async fn delete_versioned_propagates_archive_read_error() {
    let gate = make_gate();
    let (mvcc, history) = make_failing_history_mvcc(gate.clone());

    // Arm: next history write fails → tombstone can't land.
    history.fail_set.store(true, Ordering::Relaxed);

    let result = mvcc
        .delete_versioned(RecordKey::from(Bytes::from("k")))
        .await;
    assert!(
        result.is_err(),
        "delete_versioned must propagate history.set() I/O error (tombstone)"
    );
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("injected"),
        "error should be the injected fault, got: {err_msg}"
    );
}

// ================================================================
// RV-TX-2CB455A7 — failed history.transact leaves the RecordCell
// advanced with no rollback; prior version masked on point reads.
// Fix: (a) exact-version-miss reads fall back through
// `resolve_read(key, min(snapshot_or_floor, cur_v - 1), 0)`; (b) failed
// writes remove their attempted version+ts keys before the guard settles
// as Aborted; failed cleanup is fail-stop.
// ================================================================

/// Barrier + fault store: the FIRST armed `set()` call pauses on a Notify
/// barrier (so the test can interleave a concurrent committer), then either
/// fails (when `fail` is set) or delegates. No sleeps — all sequencing via
/// `Notify`.
pub(in crate::tests::mvcc_store_tests) mod barrier_fault_store {
    use async_trait::async_trait;
    use bytes::Bytes;
    use futures::stream::Stream;
    use shamir_storage::error::{DbError, DbResult};
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_storage::types::{KvOp, RecordKey, Store};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use tokio::sync::watch;
    use tokio::sync::Notify;

    pub struct BarrierFaultStore {
        inner: InMemoryStore,
        pub entered: Arc<Notify>,
        pub release: Arc<Notify>,
        pub fail: AtomicBool,
        armed: AtomicBool,
        paused: AtomicBool,
        /// RV-TX-2CB455A7 (T4): count of successful delegated `set` calls,
        /// broadcast through `write_tx` so the test can await N durable writes
        /// without sleeps.
        write_count: AtomicU64,
        /// Kept in the struct (never dropped) so `wait_writes` subscribers can
        /// never see a closed channel: the Sender lives as long as the Arc'd
        /// store.
        write_tx: watch::Sender<u64>,
    }

    impl BarrierFaultStore {
        pub fn new() -> Self {
            let (write_tx, _) = watch::channel(0);
            Self {
                inner: InMemoryStore::new(),
                entered: Arc::new(Notify::new()),
                release: Arc::new(Notify::new()),
                fail: AtomicBool::new(false),
                armed: AtomicBool::new(false),
                paused: AtomicBool::new(false),
                write_count: AtomicU64::new(0),
                write_tx,
            }
        }

        /// Arm: the next `set()` pauses on the barrier once.
        pub fn arm(&self) {
            self.armed.store(true, Ordering::SeqCst);
        }

        /// Await until at least `n` successful delegated `set` calls have
        /// completed. Sleep-free: parks on the watch channel. The Sender is
        /// held by the struct itself, so `changed()` can only ever fail on a
        /// fully-dropped store (impossible while the Arc is alive).
        pub async fn wait_writes(&self, n: u64) {
            let mut rx = self.write_tx.subscribe();
            while *rx.borrow_and_update() < n {
                rx.changed()
                    .await
                    .expect("write-count sender lives as long as the store");
            }
        }
    }

    #[async_trait]
    impl Store for BarrierFaultStore {
        async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
            if self.armed.swap(false, Ordering::SeqCst) && !self.paused.swap(true, Ordering::SeqCst)
            {
                self.entered.notify_one();
                self.release.notified().await;
            }
            if self.fail.load(Ordering::SeqCst) {
                return Err(DbError::Storage("injected I/O fault".into()));
            }
            let res = self.inner.set(key, value).await;
            if res.is_ok() {
                let n = self.write_count.fetch_add(1, Ordering::SeqCst) + 1;
                let _ = self.write_tx.send(n);
            }
            res
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
                        self.set(k, v).await?;
                    }
                    KvOp::Remove(k) => {
                        self.remove(k).await?;
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

/// Non-atomic store: `transact` applies the FIRST op, then fails (simulating
/// a backend without atomic batch semantics). Used to prove the compensating
/// cleanup removes half-applied version keys.
pub(in crate::tests::mvcc_store_tests) mod non_atomic_store {
    use async_trait::async_trait;
    use bytes::Bytes;
    use futures::stream::Stream;
    use shamir_storage::error::{DbError, DbResult};
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_storage::types::{KvOp, RecordKey, Store};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};

    pub struct NonAtomicStore {
        inner: InMemoryStore,
        fail_after_first_op: AtomicBool,
    }

    impl NonAtomicStore {
        pub fn new() -> Self {
            Self {
                inner: InMemoryStore::new(),
                fail_after_first_op: AtomicBool::new(false),
            }
        }

        /// Arm: the next `transact` applies its first op, then errors.
        pub fn arm(&self) {
            self.fail_after_first_op.store(true, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl Store for NonAtomicStore {
        async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
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
            for (i, op) in ops.into_iter().enumerate() {
                match op {
                    KvOp::Set(k, v) => {
                        self.set(k, v).await?;
                    }
                    KvOp::Remove(k) => {
                        self.remove(k).await?;
                    }
                }
                if i == 0 && self.fail_after_first_op.swap(false, Ordering::SeqCst) {
                    return Err(DbError::Storage("injected non-atomic fault".into()));
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

/// Warm the gate to version 5: four filler writes (`w1..w4`) then `k` →
/// `"v5"`, so the next allocation is the brief's v6.
async fn seed_to_v5(mvcc: &MvccStore) -> RecordKey {
    for i in 1..=4u8 {
        mvcc.set_versioned(
            RecordKey::from(Bytes::from(format!("warm{i}"))),
            Bytes::from_static(b"warm"),
        )
        .await
        .unwrap();
    }
    let k = RecordKey::from(Bytes::from_static(b"k"));
    mvcc.set_versioned(k.clone(), Bytes::from_static(b"v5"))
        .await
        .unwrap();
    k
}

/// T1 — RV-TX-2CB455A7: with v5 committed, a failed write at v6 must leave
/// get_current / get_at(floor) / get_current_many / get_at_many /
/// current_stream all returning v5.
///
/// FAILS ON HEAD with only the exact-read fallback reverted: restore
/// `Err(DbError::NotFound(_)) => Ok(None)` (dropping the
/// `if cur_v > 0` fallback arm) in `resolve_read` (store.rs) — the cell
/// reads `cur_v = 6`, the exact lookup misses and the prior v5 is masked
/// as `None` on every point read.
#[tokio::test]
async fn rv_tx_2cb455a7_failed_write_keeps_prior_version_visible() {
    let gate = make_gate();
    let (mvcc, history) = make_failing_history_mvcc(gate.clone());
    let k = seed_to_v5(&mvcc).await;

    history.fail_set.store(true, Ordering::Relaxed);
    assert!(mvcc
        .set_versioned(k.clone(), Bytes::from_static(b"v6"))
        .await
        .is_err());
    history.fail_set.store(false, Ordering::Relaxed);

    let floor = mvcc.gate.last_committed();
    assert_eq!(
        mvcc.get_current(k.clone()).await.unwrap().as_deref(),
        Some(&b"v5"[..])
    );
    assert_eq!(
        mvcc.get_at(b"k", floor).await.unwrap().as_deref(),
        Some(&b"v5"[..])
    );
    assert_eq!(
        mvcc.get_current_many(&[Bytes::from_static(b"k")])
            .await
            .unwrap()[0]
            .as_deref(),
        Some(&b"v5"[..])
    );
    assert_eq!(
        mvcc.get_at_many(&[Bytes::from_static(b"k")], floor)
            .await
            .unwrap()[0]
            .as_deref(),
        Some(&b"v5"[..])
    );
    let mut stream = Box::pin(mvcc.current_stream(64));
    let batch = stream.next().await.unwrap().unwrap();
    assert!(batch
        .iter()
        .any(|(kb, v)| kb.as_ref() == b"k" && v.as_ref() == b"v5"));
}

/// T2 — two consecutive failed writes (v6 then v7) still leave v5 visible.
///
/// FAILS ON HEAD with the exact-read fallback reverted (as in T1): after
/// the second failure the cell reads `cur_v = 7`, the exact lookups miss
/// and get_current returns `None` instead of v5.
#[tokio::test]
async fn rv_tx_2cb455a7_two_failed_writes_keep_prior_version_visible() {
    let gate = make_gate();
    let (mvcc, history) = make_failing_history_mvcc(gate.clone());
    let k = seed_to_v5(&mvcc).await;

    for attempt in 0..2 {
        history.fail_set.store(true, Ordering::Relaxed);
        assert!(mvcc
            .set_versioned(k.clone(), Bytes::from(format!("v{}", 6 + attempt)))
            .await
            .is_err());
        history.fail_set.store(false, Ordering::Relaxed);
    }

    assert_eq!(
        mvcc.get_current(k.clone()).await.unwrap().as_deref(),
        Some(&b"v5"[..])
    );
    assert_eq!(
        mvcc.get_at(b"k", mvcc.gate.last_committed())
            .await
            .unwrap()
            .as_deref(),
        Some(&b"v5"[..])
    );
}

/// T3 — a failed set_versioned_many batch leaves the fresh batch member
/// absent (`None`) and the prior committed member (`k` at v5) visible.
///
/// FAILS ON HEAD with the documented single revert — dropping the
/// `None if *cur_v > 0` arm in `get_current_many`
/// (crates/shamir-tx/src/mvcc_store/history/version_entry.rs): the batch
/// bumped both cells before the faulting transact, so the warm miss for `k`
/// in the BATCHED path resolves to `None` instead of falling back to v5 and
/// the `get_current_many` assertion below fails (the point-read assertions
/// exercise the separate `resolve_read` fallback and still pass).
#[tokio::test]
async fn rv_tx_2cb455a7_failed_batch_keeps_prior_version_visible() {
    let gate = make_gate();
    let (mvcc, history) = make_failing_history_mvcc(gate.clone());
    let k = seed_to_v5(&mvcc).await;

    history.fail_set.store(true, Ordering::Relaxed);
    let err = mvcc
        .set_versioned_many(vec![
            (
                RecordKey::from(Bytes::from_static(b"fresh")),
                Bytes::from_static(b"fv"),
            ),
            (k.clone(), Bytes::from_static(b"v6")),
        ])
        .await;
    assert!(err.is_err());
    history.fail_set.store(false, Ordering::Relaxed);

    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from_static(b"fresh")))
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        mvcc.get_current(k.clone()).await.unwrap().as_deref(),
        Some(&b"v5"[..])
    );
    // The batched API must exercise the SAME HistoryMiss fallback: after the
    // revert described above this resolves to None instead of v5.
    assert_eq!(
        mvcc.get_current_many(&[Bytes::from_static(b"k")])
            .await
            .unwrap()[0]
            .as_deref(),
        Some(&b"v5"[..])
    );
}

/// T4 — one shared `MvccStore` over ONE `BarrierFaultStore`: W1 parks inside
/// its `history.set` (unsettled v6), W2 (same cells, same overlay) allocates
/// and durably commits v7, then W1 fails and its compensating cleanup (Removes
/// — the fault only fails `set`) settles v6 Aborted so the contiguous floor
/// advances past 7 and W2's `strict_ack` completes with `Ok(7)`. Deterministic
/// sequencing via `Notify` + `watch` — no sleeps.
///
/// Old test (deadlock): W2 was awaited while W1 was still parked, but W2's
/// `strict_ack` (`gate.await_visible(7)` in writes.rs → progress.rs) can never
/// complete before W1's guard settles — the contiguous watermark stalls at 5
/// until v6 aborts. W1 was only released AFTER awaiting W2, so W2 waited
/// forever. The old test also used a SECOND `MvccStore` instance, so the
/// same-cell concurrency was never exercised. This rewrite fixes both.
///
/// Write-count accounting: each `set_versioned` issues TWO delegated `set`
/// calls (version key + ts key), so the seed is count 2 and `wait_writes(4)`
/// deterministically proves W2's durable write completed FIRST (parked W1
/// contributes nothing — it fails before its `inner.set`).
///
/// WHAT THIS PROVES (and what it does not): a failed writer's compensating
/// cleanup and abort settlement must not mask, delete or roll back a
/// concurrently COMMITTED newer version of the same key. It would fail under
/// the rejected "restore the cell to the old version on write failure"
/// design (the restore would hide W2's version from `try_reserve` and
/// current reads) and under a cleanup that removes more than W1's own
/// attempted keys. It does NOT exercise the exact-miss read fallback (W2
/// writes the exact cell version, so that path is covered by T1-T3).
#[tokio::test]
async fn rv_tx_2cb455a7_failed_writer_does_not_mask_concurrent_commit() {
    let gate = Arc::new(RepoTxGate::fresh());
    let history = Arc::new(barrier_fault_store::BarrierFaultStore::new());
    let mvcc = Arc::new(MvccStore::new(
        history.clone() as Arc<dyn Store>,
        gate.clone(),
    ));
    let k = RecordKey::from(Bytes::from_static(b"k"));
    let seed_v = mvcc
        .set_versioned(k.clone(), Bytes::from_static(b"v5"))
        .await
        .unwrap(); // write count: 2 (version key + ts key)

    history.arm();
    let mvcc_w1 = mvcc.clone();
    let k1 = k.clone();
    // W1 parks inside its history.set on the barrier — never completes; the
    // write count stays at 2.
    let w1 =
        tokio::spawn(async move { mvcc_w1.set_versioned(k1, Bytes::from_static(b"w1")).await });
    history.entered.notified().await;

    let mvcc_w2 = mvcc.clone();
    let k2 = k.clone();
    // W2: arm already consumed, fail still false → its history.set succeeds;
    // guard.commit() marks 7 Materialized but the contiguous floor stalls at
    // 5/6, so W2 parks in strict_ack.
    let w2 =
        tokio::spawn(async move { mvcc_w2.set_versioned(k2, Bytes::from_static(b"w2")).await });

    // Deterministically asserts W2's durable write completed FIRST (4 = seed
    // 2 + W2's version key + ts key).
    history.wait_writes(4).await;

    // Now fail W1: its compensating cleanup is Removes (unaffected by the
    // set-fault), succeeds, the guard settles Aborted, the floor advances
    // past 7, and W2's strict_ack completes.
    history.fail.store(true, Ordering::SeqCst);
    history.release.notify_one();
    assert!(w1.await.unwrap().is_err(), "W1 must see the injected fault");
    // Allocation order: seed, then W1 (seed+1, fails), then W2 (seed+2).
    assert_eq!(
        w2.await.unwrap().unwrap(),
        seed_v + 2,
        "W2 must ack as the version allocated after W1's failed one"
    );

    assert_eq!(
        mvcc.get_current(k.clone()).await.unwrap().as_deref(),
        Some(&b"w2"[..])
    );
    assert_eq!(
        mvcc.get_at(b"k", mvcc.gate.last_committed())
            .await
            .unwrap()
            .as_deref(),
        Some(&b"w2"[..])
    );
    assert_eq!(
        mvcc.get_current_many(&[Bytes::from_static(b"k")])
            .await
            .unwrap()[0]
            .as_deref(),
        Some(&b"w2"[..])
    );
}

/// T5 — guard test (passes on HEAD): after a failed write the cell still
/// reads the failed (aborted) version 6, NOT the prior committed v5, so a
/// snapshot-5 `try_reserve` reports a stale-write conflict (`false`).
///
/// The rejected design it discriminates is "restore the cell to old_v on
/// write failure" (explicitly rejected by the RV-TX-2CB455A7 adjudication):
/// with the cell restored to v5, snapshot 5 equals the (restored) current
/// version, so `try_reserve(k, 5, 99)` would SUCCEED (`true`) and the
/// assertion below would fail. (Snapshot 4 would reject under BOTH designs
/// and could not distinguish them.)
#[tokio::test]
async fn rv_tx_2cb455a7_try_reserve_conflicts_after_failure() {
    let gate = make_gate();
    let (mvcc, history) = make_failing_history_mvcc(gate.clone());
    let k = seed_to_v5(&mvcc).await;

    history.fail_set.store(true, Ordering::Relaxed);
    assert!(mvcc
        .set_versioned(k.clone(), Bytes::from_static(b"v6"))
        .await
        .is_err());
    history.fail_set.store(false, Ordering::Relaxed);

    assert!(!mvcc.try_reserve(k, 5, 99));
}

/// T6 — a non-atomic backend applies the first Set (the version key) then
/// errors: the compensating cleanup must leave the half-applied v6 key
/// NotFound in history.
///
/// FAILS ON HEAD with only the `compensate_failed_write` call in
/// `set_versioned_inner` removed: the half-applied `k::6` entry survives in
/// history and the `get` assertion sees `Ok`.
#[tokio::test]
async fn rv_tx_2cb455a7_non_atomic_transact_cleanup_removes_half_applied_version() {
    let history = Arc::new(non_atomic_store::NonAtomicStore::new());
    let mvcc = MvccStore::new(
        history.clone() as Arc<dyn Store>,
        Arc::new(RepoTxGate::fresh()),
    );
    let k = seed_to_v5(&mvcc).await;

    history.arm();
    assert!(mvcc
        .set_versioned(k.clone(), Bytes::from_static(b"v6"))
        .await
        .is_err());

    let vk6 = encode_version_key(b"k", 6);
    assert!(
        matches!(history.get(vk6.into()).await, Err(DbError::NotFound(_))),
        "half-applied failed version key must be removed from history"
    );
}

/// T7 — when the compensating removal itself fails the version is not
/// settled: `requires_reopen` is set, the floor stalls below v6, and
/// further writes are refused (fail-stop).
///
/// FAILS ON HEAD with the `set_requires_reopen` + `mem::forget` fail-stop
/// path in `compensate_failed_write` removed: on HEAD no cleanup is
/// attempted, the guard drops as Aborted so the floor reaches 6, and
/// `requires_reopen` stays false.
#[tokio::test]
async fn rv_tx_2cb455a7_failed_cleanup_requires_reopen_and_refuses_writes() {
    let gate = make_gate();
    let (mvcc, history) = make_failing_history_mvcc(gate.clone());
    let k = seed_to_v5(&mvcc).await;

    history.fail_set.store(true, Ordering::Relaxed);
    history.fail_remove.store(true, Ordering::Relaxed);
    assert!(mvcc
        .set_versioned(k.clone(), Bytes::from_static(b"v6"))
        .await
        .is_err());

    assert!(
        mvcc.gate.requires_reopen(),
        "cleanup failure must set requires_reopen"
    );
    assert!(
        mvcc.gate.last_committed() < 6,
        "floor must stall below the unsettled v6, got {}",
        mvcc.gate.last_committed()
    );
    match mvcc
        .set_versioned(k.clone(), Bytes::from_static(b"v7"))
        .await
    {
        Err(DbError::RepoRequiresReopen(_)) => {}
        other => panic!("writes must be refused after failed cleanup, got {other:?}"),
    }
}

/// T8 — rebuilding an `MvccStore` from the same history shows no trace of
/// the failed v6: the half-applied version key was removed, so a rebuilt
/// store reads v5.
///
/// FAILS ON HEAD with `compensate_failed_write` removed from
/// `set_versioned_inner`: the non-atomic store keeps the half-applied
/// `k::6`, so the rebuilt store's cold scan resolves v6 (`"v6bytes"`).
#[tokio::test]
async fn rv_tx_2cb455a7_rebuilt_store_shows_no_failed_version() {
    let history = Arc::new(non_atomic_store::NonAtomicStore::new());
    let mvcc = MvccStore::new(
        history.clone() as Arc<dyn Store>,
        Arc::new(RepoTxGate::fresh()),
    );
    let k = seed_to_v5(&mvcc).await;

    history.arm();
    assert!(mvcc
        .set_versioned(k.clone(), Bytes::from_static(b"v6bytes"))
        .await
        .is_err());

    let rebuilt = MvccStore::new(
        history.clone() as Arc<dyn Store>,
        Arc::new(RepoTxGate::fresh()),
    );
    assert_eq!(
        rebuilt.get_current(k.clone()).await.unwrap().as_deref(),
        Some(&b"v5"[..])
    );
    let vk6 = encode_version_key(b"k", 6);
    assert!(matches!(
        rebuilt.history_store().get(vk6.into()).await,
        Err(DbError::NotFound(_))
    ));
}
