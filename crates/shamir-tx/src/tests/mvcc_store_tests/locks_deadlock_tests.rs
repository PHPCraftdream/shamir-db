//! H2 — `MvccStore::locks` mixed `_async`/`_sync` whole-runtime deadlock
//! regression test.
//!
//! Same structural class as the #589 fix for the `cells` map
//! (`crates/shamir-tx/src/mvcc_store/mod.rs::publish_cell`, commit
//! `7a4abf62`). The per-table `locks` scc map previously mixed:
//! * `lock_key`      — `self.locks.entry_async(key).await`  (every Level-3
//!   pessimistic lock acquisition).
//! * `release_locks` — `self.locks.get_sync(key)`           (every Level-3
//!   commit AND abort, via `release_pessimistic_locks`, called from
//!   `crates/shamir-engine/src/tx/commit.rs:825-826`).
//!
//! The pessimistic-locking hot-key case — many txs contending for the SAME
//! `RecordKey` — funnels onto ONE bucket by construction. The interleaving
//! that deadlocked before the fix:
//! 1. Tx A calls `lock_key(k)` → `entry_async(k)` suspends (bucket held by
//!    another acquirer / releaser).
//! 2. The holder releases; saa hands the exclusive bucket lock to A's
//!    suspended task. A now owns the lock while sitting in tokio's run
//!    queue, unpolled.
//! 3. Before A is polled, N txs (B..N) finish on the N worker threads
//!    (commit or abort); each `release_locks` → `get_sync(k)` → PARKS its
//!    OS worker thread on the same bucket.
//! 4. All workers parked → A never polled → whole-runtime deadlock. The
//!    parked releasers are EXACTLY the txs whose release would have let
//!    A's waiters make progress — even a near-deadlock inflates wound-wait
//!    latencies badly.
//!
//! **Why this test exists** (mirrors how the codebase already reasons about
//! `overlay_ordering_tests.rs`): this hazard is a RACE WINDOW, not a
//! deterministic deadlock — nextest's parallelism only sometimes lands all
//! workers in `release_locks` → `get_sync` at the exact instant a
//! `lock_key` task is sitting in the run queue holding the handed-off
//! bucket lock. The goal of the test is therefore BOTH (a) to exercise the
//! interleaving so nextest's parallelism has a real chance to catch a
//! future regression over time, AND (b) a NAMED bounded
//! `tokio::time::timeout` so that a real regression fails fast and
//! identifiably instead of hanging the entire nextest run with an
//! anonymous TIMEOUT. The timeout is NOT a workaround for flakiness — it
//! is this test's own guard against a real regression hanging the whole
//! suite (cf.
//! `crates/shamir-index/src/vector/tests/quantized_graph_tests.rs:1630`).
//!
//! NB: this test uses `lock_key`/`release_locks` directly rather than the
//! full tx commit path because that is the exact API surface the fix
//! targets — `release_locks`'s `get_sync` is the synchronous accessor
//! whose existence forces `lock_key`'s `entry` op to be synchronous too
//! (the project's convention, established by the #589 fix).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use super::helpers::make_mvcc;
use crate::mvcc_store::{LockMode, MvccStore};
use shamir_storage::types::RecordKey;

/// Number of concurrent txs all hammering the SAME shared hot key. Each tx
/// repeatedly acquires (lock_key) and releases (release_locks) the same key,
/// so all contention funnels onto ONE `locks` bucket — the worst case for
/// the mixed `entry_async`/`get_sync` hazard.
const TXS: usize = 8;

/// Acquire/release iterations per tx. Each iteration drives the exact
/// interleaving the pre-fix hazard needed: a `lock_key` → `entry_async(k)`
/// racing `release_locks` → `get_sync(k)` on the same hot key.
const ITERS: usize = 100;

/// Test bundle: a tx's wound flag + wake notify, mirrors the `TxWound`
/// helper in `lock_tests.rs` (kept here so this test module is
/// self-contained and the H2 test file does not need to depend on the
/// private `TxWound` in `lock_tests.rs`).
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
}

/// Site 2 regression: `TXS` concurrent Level-3 txs all hammer the SAME
/// shared hot `RecordKey` — each repeatedly acquires Exclusive via
/// `lock_key` and releases via `release_locks` — on a runtime with only
/// TWO worker threads (the smallest non-trivial count — on a 1-2 worker
/// runtime a single racing release is enough to expose the pre-fix hazard).
///
/// Every acquire/release targets ONE `locks` bucket (the key is constant
/// for the run), so this exercises the worst-case shape of the hazard:
/// a `lock_key` → `entry_async(k)` task handed the bucket lock while
/// racing `release_locks` → `get_sync(k)` calls park every worker.
///
/// Pre-fix expectation: under the mixed `entry_async`/`get_sync` hazard,
/// all worker threads can park in `release_locks` while a handed-off
/// `lock_key` task owns the bucket lock → whole-runtime deadlock → the
/// `tokio::time::timeout` below fires and the named assertion points
/// unambiguously at this hazard.
///
/// Post-fix: `lock_key` uses `entry_sync`, so every bucket lock is only
/// ever held by a RUNNING thread for a few instructions (an
/// `Arc::clone`-or-insert) → bounded waits, no deadlock window → the run
/// completes well within the timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn locks_concurrent_acquire_release_shared_hot_key_no_deadlock() {
    let mvcc: Arc<MvccStore> = Arc::new(make_mvcc());
    let key = RecordKey::from(b"locks-hot-key" as &[u8]);

    // Each tx gets a UNIQUE tx_version (wound-wait priority order) but they
    // all hammer the SAME key. Distinct versions keep the test realistic
    // (real txs have distinct priorities) while still funnelling every
    // acquire/release onto one bucket.
    let next_version = Arc::new(AtomicU64::new(1));

    let mut handles = Vec::with_capacity(TXS);
    for _ in 0..TXS {
        let mvcc = Arc::clone(&mvcc);
        let key = key.clone();
        let next_version = Arc::clone(&next_version);
        handles.push(tokio::spawn(async move {
            // Assign this tx's stable priority version ONCE so wound-wait
            // determinism holds (older wins). We do not bump it per
            // iteration — re-acquiring the same key with the same
            // tx_version is the re-entrant fast path, but interleaved
            // acquire/release across txs still exercises the bucket-lock
            // contention that exposes the hazard.
            let tx_version = next_version.fetch_add(1, Ordering::Relaxed);
            let wound = TxWound::new();
            for _ in 0..ITERS {
                // lock_key → entry_sync(key) post-fix (entry_async pre-fix).
                // Any conflict here resolves via wound-wait; we do NOT
                // unwrap Ok — a wound is a legitimate outcome under
                // contention and the test cares about liveness, not which
                // tx acquired on which iteration.
                let _ = mvcc
                    .lock_key(
                        key.clone(),
                        tx_version,
                        wound.flag(),
                        wound.notify(),
                        LockMode::Exclusive,
                    )
                    .await;
                // release_locks → get_sync(key). The exact synchronous
                // accessor that, before the fix, parked the OS worker
                // thread on the same bucket a handed-off entry_async task
                // was sitting on.
                mvcc.release_locks(tx_version, std::slice::from_ref(&key))
                    .await;
                // Yield to maximise interleaving between acquirers and
                // releasers across worker threads.
                tokio::task::yield_now().await;
            }
        }));
    }

    // Bounded guard: a real regression hangs the suite here, this turns
    // the silent 180s nextest-TIMEOUT into a fast, named, specific failure
    // (NOT a flakiness workaround — see module doc above).
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        for h in handles {
            h.await.unwrap();
        }
    })
    .await
    .expect(
        "locks acquire/release hammer deadlocked — this is the #589-class \
         entry_async/get_sync mixed-lock hazard on the SAME hot-key bucket. \
         lock_key MUST use entry_sync, not entry_async. \
         See module doc + commit 7a4abf62 for the same-class cells-map fix.",
    );

    // Post-run sanity: every tx released its key on the last iteration,
    // so the holders should be empty across all keys. `locks_len` is the
    // test-only accessor (carries the disallowed_methods allow comment).
    // Leftover empty entries are intentionally kept by `release_locks`
    // (cheap; no GC), so `locks_len()` only confirms the map is non-zero
    // (the hot key was inserted) — not that holders are drained. We
    // therefore re-acquire + release once more to confirm the map is still
    // servicable post-hammer (no poisoned state).
    let wound = TxWound::new();
    mvcc.lock_key(
        key.clone(),
        next_version.fetch_add(1, Ordering::Relaxed),
        wound.flag(),
        wound.notify(),
        LockMode::Exclusive,
    )
    .await
    .expect("post-hammer re-acquire must succeed — locks map is servicable");
    assert!(
        mvcc.locks_len() >= 1,
        "locks map must contain at least the hot key after the hammer"
    );
}
