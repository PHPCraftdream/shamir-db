//! H1 — `active_snapshots` mixed `_async`/`_sync` whole-runtime deadlock
//! regression test.
//!
//! Same structural class as the #589 fix for the `cells` map
//! (`crates/shamir-tx/src/mvcc_store/mod.rs::publish_cell`, commit
//! `7a4abf62`). The `active_snapshots` scc map previously mixed:
//! * `bump_refcount`            — `entry_async(version).await`  (every
//!   `open_snapshot` / `open_snapshot_serializable`).
//! * `SnapshotGuard::drop`      — `entry_sync(v)`               (every tx
//!   end, inline on whatever runtime worker drops the guard).
//! * `min_alive` (GC tick)      — `iter_sync(...)`              (vacuum /
//!   prune ticks; scc's sync iterator parks the calling thread per bucket).
//! * `active_snapshots_empty`   — `is_empty()`                  (vacuum
//!   fast paths; scc's `is_empty` also takes bucket read locks
//!   synchronously).
//!
//! The map is keyed by `version`, and every concurrent `open_snapshot`
//! targets the SAME key — the CURRENT `last_committed` version. So — like
//! the `cells` map's hot key — all contention funnels onto ONE bucket by
//! construction. The interleaving that deadlocked before the fix:
//! 1. Tx A calls `open_snapshot` → `entry_async(v_cur)` suspends (bucket
//!    held by another opener/dropper).
//! 2. The holder releases; saa hands the exclusive bucket lock to A's
//!    suspended task. A now owns the lock while sitting in tokio's run
//!    queue, unpolled.
//! 3. Before A is polled, N committed txs finish on the N worker threads;
//!    each `SnapshotGuard::drop` runs `entry_sync(v_cur)` → PARKS its OS
//!    worker thread on the same bucket.
//! 4. All workers parked → A is never polled → whole-runtime deadlock.
//!
//! **Why this test exists** (mirrors how the codebase already reasons about
//! `overlay_ordering_tests.rs`): this hazard is a RACE WINDOW, not a
//! deterministic deadlock — nextest's parallelism only sometimes lands all
//! workers in `SnapshotGuard::drop` at the exact instant a `bump_refcount`
//! task is sitting in the run queue holding the handed-off bucket lock. The
//! goal of the test is therefore BOTH (a) to exercise the interleaving so
//! nextest's parallelism has a real chance to catch a future regression
//! over time, AND (b) a NAMED bounded `tokio::time::timeout` so that a real
//! regression fails fast and identifiably (and specifically points at this
//! hazard) instead of hanging the entire nextest run for 180s with an
//! anonymous TIMEOUT. The timeout is NOT a workaround for flakiness — it is
//! this test's own guard against a real regression hanging the whole suite
//! (cf. `crates/shamir-index/src/vector/tests/quantized_graph_tests.rs:1630`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::repo_tx_gate::RepoTxGate;

/// Number of concurrent open/close hammer tasks. Each task tight-loops
/// `open_snapshot` + drop the returned guard, racing every other task on
/// the SAME `last_committed` version (the floor is fixed for the run, so
/// every opener targets one bucket).
const HAMMERS: usize = 8;

/// Iterations per hammer. Each iteration is one open + one drop — the exact
/// `register_snapshot` → `bump_refcount` / `SnapshotGuard::drop` interleaving
/// under test.
const ITERS: usize = 400;

/// One concurrent open/close hammer of `active_snapshots`. Drives the exact
/// interleaving the pre-fix hazard needed: many openers bumping the
/// refcount on the SAME hot version while many droppers run
/// `entry_sync` on it.
async fn hammer(gate: Arc<RepoTxGate>, stop: Arc<AtomicBool>) {
    for _ in 0..ITERS {
        // Open: register_snapshot → bump_refcount. The floor never moves
        // during this run (`last_committed` is fixed), so every iteration
        // targets the same single bucket — the worst case for the
        // mixed-async/sync hazard.
        let guard = gate.open_snapshot().await;
        // Drop: SnapshotGuard::drop → entry_sync(v). The exact synchronous
        // accessor that, before the fix, parked the OS worker thread on
        // the same bucket a handed-off `entry_async` task was sitting on.
        drop(guard);
        // Yield to maximise interleaving between openers and droppers
        // across worker threads.
        tokio::task::yield_now().await;
        if stop.load(Ordering::Relaxed) {
            return;
        }
    }
}

/// Site 1 regression: hammer `open_snapshot` / `SnapshotGuard::drop` from
/// `HAMMERS` concurrent tasks on a runtime with only TWO worker threads
/// (the smallest non-trivial count — on a 1-2 worker runtime a single
/// racing drop is enough to expose the pre-fix hazard), with all tasks
/// targeting the SAME `last_committed` floor.
///
/// Pre-fix expectation: under the mixed `entry_async`/`entry_sync` hazard,
/// all worker threads can park in `SnapshotGuard::drop` while a handed-off
/// `bump_refcount` task owns the bucket lock → whole-runtime deadlock → the
/// `tokio::time::timeout` below fires and the named assertion points
/// unambiguously at this hazard.
///
/// Post-fix: every accessor is synchronous, so every bucket lock is only
/// ever held by a RUNNING thread for a few instructions → bounded waits,
/// no deadlock window → the run completes well within the timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_snapshots_concurrent_open_close_no_deadlock() {
    let gate = Arc::new(RepoTxGate::fresh());
    // Pin the floor so EVERY opener targets the same single bucket.
    gate.publish_committed(1);

    let stop = Arc::new(AtomicBool::new(false));

    let mut handles = Vec::with_capacity(HAMMERS);
    for _ in 0..HAMMERS {
        let g = Arc::clone(&gate);
        let s = Arc::clone(&stop);
        handles.push(tokio::spawn(async move { hammer(g, s).await }));
    }

    // Bounded guard: a real regression hangs the suite here, this turns
    // the silent 180s nextest-TIMEOUT into a fast, named, specific failure
    // (NOT a flakiness workaround — see module doc above).
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        for h in handles {
            h.await.unwrap();
        }
    })
    .await
    .expect(
        "active_snapshots open/close hammer deadlocked — this is the \
         #589-class entry_async/entry_sync mixed-lock hazard on the \
         SAME hot-key bucket (publish_committed(1) floor). \
         bump_refcount MUST use entry_sync, not entry_async. \
         See module doc + commit 7a4abf62 for the same-class cells-map fix.",
    );

    // After a clean run the gate should be fully drained (every guard was
    // dropped). Sanity: confirms no snapshot leaked through a lost-drop
    // race during the hammer.
    stop.store(true, Ordering::Relaxed);
    assert!(
        gate.active_snapshots_empty(),
        "active_snapshots must be empty after every guard dropped"
    );
}

/// Site 1 (variant): the same hazard also manifests on the
/// `open_snapshot_serializable` path (which calls `register_snapshot` →
/// `bump_refcount` identically, plus an `active_serializable_count`
/// increment). Exercise it explicitly so a future refactor that re-introduces
/// `entry_async` on the serializable path is also caught. Same timeout guard.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_snapshots_concurrent_serializable_open_close_no_deadlock() {
    let gate = Arc::new(RepoTxGate::fresh());
    gate.publish_committed(1);

    let stop = Arc::new(AtomicBool::new(false));

    let mut handles = Vec::with_capacity(HAMMERS);
    for _ in 0..HAMMERS {
        let g = Arc::clone(&gate);
        let s = Arc::clone(&stop);
        handles.push(tokio::spawn(async move {
            for _ in 0..ITERS {
                let guard = g.open_snapshot_serializable().await;
                drop(guard);
                tokio::task::yield_now().await;
                if s.load(Ordering::Relaxed) {
                    return;
                }
            }
        }));
    }

    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        for h in handles {
            h.await.unwrap();
        }
    })
    .await
    .expect(
        "active_snapshots serializable open/close hammer deadlocked — \
         #589-class entry_async/entry_sync hazard on the serializable \
         open path (shares bump_refcount with the non-serializable path).",
    );

    stop.store(true, Ordering::Relaxed);
    assert!(
        gate.active_snapshots_empty(),
        "active_snapshots must be empty after every serializable guard dropped"
    );
    assert_eq!(
        gate.active_serializable_count(),
        0,
        "active_serializable_count must be 0 after every serializable guard dropped"
    );
}

/// Site 1 (variant): a GC `min_alive` scan (`iter_sync`) running concurrently
/// with the open/close hammer exercises the third synchronous accessor on
/// the same map. Pre-fix, an `iter_sync` walker reaching the hot bucket
/// during a handed-off `entry_async` window would park that worker too,
/// widening the deadlock. Post-fix every walker+opener+dropper is
/// synchronous, so the run completes within the bounded guard.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_snapshots_min_alive_scan_vs_open_close_no_deadlock() {
    let gate = Arc::new(RepoTxGate::fresh());
    gate.publish_committed(1);

    let stop = Arc::new(AtomicBool::new(false));

    // Open/close hammer.
    let mut handles = Vec::with_capacity(HAMMERS);
    for _ in 0..HAMMERS {
        let g = Arc::clone(&gate);
        let s = Arc::clone(&stop);
        handles.push(tokio::spawn(async move { hammer(g, s).await }));
    }

    // Concurrent GC-tick scanner: tight-loops `min_alive` (which walks the
    // map via `iter_sync`) for the whole hammer duration.
    let gate_gc = Arc::clone(&gate);
    let stop_gc = Arc::clone(&stop);
    let gc_handle = tokio::spawn(async move {
        while !stop_gc.load(Ordering::Relaxed) {
            // min_alive walks active_snapshots via iter_sync while the
            // hammer openers/droppers race on the same map.
            let _ = gate_gc.min_alive();
            tokio::task::yield_now().await;
        }
    });

    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        for h in handles {
            h.await.unwrap();
        }
    })
    .await
    .expect(
        "active_snapshots open/close vs min_alive(iter_sync) scan deadlocked — \
         #589-class entry_async/iter_sync hazard on the SAME hot-key bucket.",
    );

    stop.store(true, Ordering::Relaxed);
    gc_handle.await.unwrap();
    assert!(
        gate.active_snapshots_empty(),
        "active_snapshots must be empty after every guard dropped"
    );
}
