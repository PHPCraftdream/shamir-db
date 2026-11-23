use std::sync::mpsc;
use std::sync::{Arc, Barrier};

use shamir_test_diag::FlakeGuard;

use crate::repo::instance::support::metrics::EngineMetrics;

use super::super::{
    ShapeCounters, ShapeRegistry, ShapeStatsSnapshot, SHAPE_ROWS_CAP, SHAPE_ROWS_LOW_WATER,
};

fn key_of(tag: u64, index: u64) -> [u8; 16] {
    let mut key = [0u8; 16];
    key[..8].copy_from_slice(&tag.to_le_bytes());
    key[8..].copy_from_slice(&index.to_le_bytes());
    key
}

fn counted(snapshot: &ShapeStatsSnapshot) -> u64 {
    snapshot
        .rows
        .iter()
        .map(|(_, counters)| counters.executions)
        .sum::<u64>()
        + snapshot.other.executions
}

/// Fails unless `snapshot` is *entirely* the settled pre-reset generation
/// described by `(old_rows, old_total, old_tracked)` or *entirely* the empty
/// generation published by `reset`. A snapshot that mixes rows from one
/// generation with the mirror of another fails here.
fn assert_single_generation(
    snapshot: &ShapeStatsSnapshot,
    old_rows: usize,
    old_total: u64,
    old_tracked: u64,
) {
    assert!(
        snapshot.rows.len() <= SHAPE_ROWS_CAP,
        "row count {} exceeds the cap",
        snapshot.rows.len()
    );
    assert!(
        snapshot.rows.len() as u64 <= snapshot.tracked,
        "rows {} exceed the tracked mirror {}",
        snapshot.rows.len(),
        snapshot.tracked
    );
    let is_old = snapshot.rows.len() == old_rows
        && counted(snapshot) == old_total
        && snapshot.other == ShapeCounters::default()
        && snapshot.tracked == old_tracked;
    let is_fresh = snapshot.rows.is_empty()
        && snapshot.other == ShapeCounters::default()
        && snapshot.tracked == 0;
    assert!(
        is_old || is_fresh,
        "snapshot mixes generations: {snapshot:?}"
    );
}

#[test]
fn shape_eviction_bound_and_other_fold() {
    let guard = FlakeGuard::new("shape_eviction_bound_and_other_fold");
    let registry = Arc::new(ShapeRegistry::new());
    // Handshake: every recorder starts only once all eight are ready, so the
    // cap and the fold are exercised concurrently.
    let barrier = Arc::new(Barrier::new(8));
    guard.mark("8 recorders about to be released at the barrier");
    std::thread::scope(|scope| {
        for thread in 0..8u64 {
            let registry = Arc::clone(&registry);
            let barrier = Arc::clone(&barrier);
            scope.spawn(move || {
                barrier.wait();
                for index in 0..125u64 {
                    registry.record(key_of(thread, index), 7, false);
                }
            });
        }
    });
    guard.mark("8 recorders joined (1000 executions recorded)");

    let snapshot = registry.snapshot();
    assert!(snapshot.rows.len() <= SHAPE_ROWS_CAP);
    assert_eq!(counted(&snapshot), 1000);
    assert!(snapshot.other.executions > 0);
    guard.mark("eviction-bound and fold assertions held");
    guard.note("rows", snapshot.rows.len().to_string());
    guard.note("other executions", snapshot.other.executions.to_string());
}

#[test]
fn shape_stats_reset_clears_rows_and_other() {
    let registry = ShapeRegistry::new();
    registry.record(key_of(1, 1), 100, false);
    registry.record(key_of(1, 1), 50, true);

    let before = registry.snapshot();
    assert_eq!(before.rows.len(), 1);
    assert_eq!(before.rows[0].1.executions, 2);
    assert_eq!(before.rows[0].1.errors, 1);

    // Drive more distinct keys than the cap so the overflow folds into
    // `other`; a reset that forgets the `other` bucket must fail below.
    let overflow = SHAPE_ROWS_CAP as u64 + 64;
    for index in 0..overflow {
        registry.record(key_of(2, index), 4, false);
    }
    let folded = registry.snapshot();
    assert!(folded.other.executions > 0);
    assert!(folded.rows.len() <= SHAPE_ROWS_CAP);
    assert_eq!(counted(&folded), 2 + overflow);

    registry.reset();
    let cleared = registry.snapshot();
    assert!(cleared.rows.is_empty());
    assert_eq!(cleared.other, ShapeCounters::default());
    assert_eq!(cleared.tracked, 0);

    registry.record(key_of(1, 1), 7, false);
    let after = registry.snapshot();
    assert_eq!(after.rows.len(), 1);
    assert_eq!(after.rows[0].1.executions, 1);
    assert_eq!(after.rows[0].1.total_nanos, 7);
    assert_eq!(after.rows[0].1.errors, 0);
}

#[test]
fn concurrent_records_at_cap_keep_accounting_invariant() {
    let guard = FlakeGuard::new("concurrent_records_at_cap_keep_accounting_invariant");
    let registry = Arc::new(ShapeRegistry::new());
    for index in 0..SHAPE_ROWS_CAP as u64 {
        registry.record(key_of(u64::MAX, index), 3, false);
    }
    assert_eq!(registry.tracked(), SHAPE_ROWS_CAP);

    // Handshake: all eight threads are released together, so the 800 new
    // keys contend with a registry that is already at its cap.
    let barrier = Arc::new(Barrier::new(8));
    guard.mark("registry at cap; 8 recorders about to be released");
    std::thread::scope(|scope| {
        for thread in 0..8u64 {
            let registry = Arc::clone(&registry);
            let barrier = Arc::clone(&barrier);
            scope.spawn(move || {
                barrier.wait();
                for index in 0..100u64 {
                    registry.record(key_of(thread, index), 5, false);
                }
            });
        }
    });
    guard.mark("8 recorders joined (800 executions at cap)");

    let snapshot = registry.snapshot();
    assert!(snapshot.rows.len() <= SHAPE_ROWS_CAP);
    assert_eq!(counted(&snapshot), SHAPE_ROWS_CAP as u64 + 800);
    assert!(snapshot.other.executions > 0);
    guard.mark("accounting invariant held");
    guard.note("rows", snapshot.rows.len().to_string());
    guard.note("other executions", snapshot.other.executions.to_string());
}

#[test]
fn reset_during_reserved_insert_keeps_the_cap() {
    let registry = ShapeRegistry::new();
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let parked_registry = &registry;
    std::thread::scope(|scope| {
        let worker = scope.spawn(move || {
            // Parked *after* the slot reservation, *before* the map insert.
            parked_registry.record_paused(key_of(u64::MAX, 0), 1, false, || {
                ready_tx.send(()).expect("handshake: recorder parked");
                go_rx.recv().expect("handshake: recorder released");
            });
        });
        ready_rx
            .recv()
            .expect("recorder reached the reserve-insert window");
        registry.reset();
        go_tx.send(()).expect("release the parked recorder");
        worker.join().expect("parked recorder");
    });

    // The stray insert landed in the discarded state, so the live state is
    // free to take a full cap of brand-new keys.
    for index in 0..SHAPE_ROWS_CAP as u64 {
        registry.record(key_of(4, index), 2, false);
    }

    let live = registry.snapshot();
    assert_eq!(live.rows.len(), SHAPE_ROWS_CAP);
    assert_eq!(registry.tracked(), SHAPE_ROWS_CAP);
    assert_eq!(live.tracked, SHAPE_ROWS_CAP as u64);
    assert_eq!(counted(&live), SHAPE_ROWS_CAP as u64);
    assert_eq!(live.other, ShapeCounters::default());
}

/// mutant: a snapshot that re-loads the state after the pause, or one that
/// reads `other` from the live generation, would mix generations. The
/// snapshot is parked — via [`ShapeRegistry::snapshot_paused`] — after its row
/// copy and before its `(other)`/mirror reads, the reset publishes a fresh
/// state in that window, and the returned snapshot must still be *exactly*
/// the pinned old generation.
#[test]
fn snapshot_during_reset_is_internally_consistent() {
    let registry = ShapeRegistry::new();
    registry.record(key_of(5, 0), 3, false);
    registry.record(key_of(5, 1), 4, false);

    let settled = registry.snapshot();
    assert_eq!(settled.rows.len(), 2);
    assert_eq!(counted(&settled), 2);
    assert_eq!(settled.other, ShapeCounters::default());
    assert_eq!(settled.tracked, 2);

    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let parked_registry = &registry;
    let parked = std::thread::scope(|scope| {
        let worker = scope.spawn(move || {
            parked_registry.snapshot_paused(|| {
                ready_tx.send(()).expect("handshake: snapshot parked");
                go_rx.recv().expect("handshake: snapshot released");
            })
        });
        ready_rx
            .recv()
            .expect("snapshot reached the post-copy window");

        // The snapshot has already copied its rows and nothing has moved
        // yet: the live generation still holds exactly the two settled rows.
        let before = registry.snapshot();
        assert_eq!(before.rows.len(), 2);
        assert_eq!(counted(&before), 2);
        assert_eq!(before.tracked, 2);

        registry.reset();
        go_tx.send(()).expect("release the parked snapshot");
        worker.join().expect("parked snapshot")
    });

    // Not a mix: the parked snapshot pinned the old generation before the
    // reset, so every part of it is the old generation.
    assert_single_generation(&parked, 2, 2, 2);
    assert_eq!(parked.rows.len(), 2);
    assert_eq!(counted(&parked), 2);
    assert_eq!(parked.other, ShapeCounters::default());
    assert_eq!(parked.tracked, 2);

    let after = registry.snapshot();
    assert!(after.rows.is_empty());
    assert_eq!(after.other, ShapeCounters::default());
    assert_eq!(after.tracked, 0);
}

/// mutant: `ShapeState::snapshot_with` copying the rows and then reading
/// `(other)` without taking the cold-path guard (delete `acquire_exclusive`,
/// or force it to return `true`): the trim then runs inside the parked window,
/// evicts rows behind the copy's back and folds them into `(other)`, so
/// `counted` exceeds the records and the parked copy no longer holds a full
/// cap of rows. The seam and the guard land together, so the parent of this
/// change cannot compile this test; the guard-free variant of it fails here.
#[test]
fn snapshot_during_trim_counts_each_execution_once() {
    let guard = FlakeGuard::new("snapshot_during_trim_counts_each_execution_once");
    let registry = ShapeRegistry::new();
    for index in 0..SHAPE_ROWS_CAP as u64 {
        registry.record(key_of(u64::MAX, index), 7, false);
    }
    let at_cap = registry.snapshot();
    assert_eq!(at_cap.rows.len(), SHAPE_ROWS_CAP);
    assert_eq!(counted(&at_cap), SHAPE_ROWS_CAP as u64);
    guard.mark("registry filled to the cap, no trim yet");
    guard.note("rows at cap", at_cap.rows.len().to_string());

    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let parked_registry = &registry;
    let parked = std::thread::scope(|scope| {
        let worker = scope.spawn(move || {
            // Parked *after* the row copy, *before* the `(other)` read.
            parked_registry.snapshot_paused(|| {
                ready_tx.send(()).expect("handshake: snapshot parked");
                go_rx.recv().expect("handshake: snapshot released");
            })
        });
        ready_rx
            .recv()
            .expect("snapshot reached the post-copy window");

        // At the cap this folds into `(other)` and asks for a trim. The
        // snapshot holds the guard, so the trim is suppressed and nothing
        // is evicted behind the parked copy's back.
        registry.record(key_of(1, 0), 11, false);

        go_tx.send(()).expect("release the parked snapshot");
        worker.join().expect("parked snapshot")
    });

    guard.mark("parked snapshot joined");
    // Each execution counted exactly once, and the suppressed trim left every
    // row in place.
    assert_eq!(counted(&parked), SHAPE_ROWS_CAP as u64 + 1);
    assert_eq!(parked.rows.len(), SHAPE_ROWS_CAP);
    assert!(
        parked.rows.len() as u64 <= parked.tracked,
        "rows {} exceed the tracked mirror {}",
        parked.rows.len(),
        parked.tracked
    );
    guard.note("parked rows", parked.rows.len().to_string());
    guard.note(
        "parked other executions",
        parked.other.executions.to_string(),
    );

    // Guard released: the deferred trim runs and shrinks the map.
    registry.record(key_of(1, 1), 13, false);
    let snapshot = registry.snapshot();
    guard.mark("deferred trim observed");
    assert_eq!(counted(&snapshot), SHAPE_ROWS_CAP as u64 + 2);
    assert!(snapshot.rows.len() <= SHAPE_ROWS_CAP);
    assert!(snapshot.rows.len() <= SHAPE_ROWS_LOW_WATER);
    assert!(
        snapshot.rows.len() as u64 <= snapshot.tracked,
        "rows {} exceed the tracked mirror {}",
        snapshot.rows.len(),
        snapshot.tracked
    );
    guard.mark("count-once and deferred-trim assertions held");
    guard.note("rows after trim", snapshot.rows.len().to_string());
    guard.note("other executions", snapshot.other.executions.to_string());
}

#[test]
fn reset_discards_a_pinned_reservation_not_the_live_slot() {
    let registry = ShapeRegistry::new();
    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let parked_registry = &registry;
    std::thread::scope(|scope| {
        let worker = scope.spawn(move || {
            parked_registry.record_paused(key_of(u64::MAX, 2), 1, false, || {
                ready_tx.send(()).expect("handshake: recorder parked");
                go_rx.recv().expect("handshake: recorder released");
            });
        });
        ready_rx
            .recv()
            .expect("recorder reached the reserve-insert window");
        registry.reset();
        go_tx.send(()).expect("release the parked recorder");
        worker.join().expect("parked recorder");
    });

    for index in 0..3u64 {
        registry.record(key_of(6, index), 1, false);
    }

    let live = registry.snapshot();
    assert_eq!(live.rows.len(), 3);
    // The discarded reservation must not leave a phantom +1 behind.
    assert_eq!(registry.tracked(), 3);
    assert_eq!(live.tracked, 3);
    assert_eq!(counted(&live), 3);
}

/// mutant: no `Drop` release (or a `mem::forget` of the guard) — the panic leaves
/// `exclusive` at `true`, every later `trim` returns immediately, and the rows stay at
/// the cap instead of shrinking.
///
/// The seam panics while `snapshot_with` holds the guard, so the unwind must run the
/// guard's `Drop`. Filling the cap and then recording 64 distinct keys makes the 1st
/// and the 34th overflow key each trim down to [`SHAPE_ROWS_LOW_WATER`] (the keys in
/// between re-insert), so the final map sits 30 keys above the low-water mark and
/// strictly below the cap. A leaked guard erases exactly that: all 256 rows stay in
/// place and every overflow key folds into `(other)` without a single removal.
#[test]
fn panic_in_the_snapshot_seam_releases_the_guard() {
    let guard = FlakeGuard::new("panic_in_the_snapshot_seam_releases_the_guard");
    let registry = ShapeRegistry::new();
    guard.mark("snapshot about to panic inside its seam");
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        registry.snapshot_paused(|| panic!("seam panic"))
    }));
    assert!(
        caught.is_err(),
        "the seam panic must unwind through the snapshot"
    );
    guard.mark("seam panic unwound");

    for index in 0..SHAPE_ROWS_CAP as u64 {
        registry.record(key_of(u64::MAX, index), 5, false);
    }
    for index in 0..64u64 {
        registry.record(key_of(u64::MAX, SHAPE_ROWS_CAP as u64 + index), 5, false);
    }

    let snapshot = registry.snapshot();
    assert!(
        snapshot.rows.len() < SHAPE_ROWS_CAP,
        "rows {} never shrank below the cap: the panicking snapshot leaked the guard",
        snapshot.rows.len()
    );
    assert!(snapshot.other.executions > 0);
    assert_eq!(counted(&snapshot), SHAPE_ROWS_CAP as u64 + 64);
    guard.mark("rows shrank below the cap and every execution is counted once");
    guard.note("rows", snapshot.rows.len().to_string());
    guard.note("other executions", snapshot.other.executions.to_string());
}

/// mutant: a gauge that counts the `(other)` fold as an extra row (or `tracked() + 1`) — the folded executions are non-zero here, so the equality fails.
#[test]
fn shapes_tracked_gauge_excludes_the_other_fold() {
    let guard = FlakeGuard::new("shapes_tracked_gauge_excludes_the_other_fold");
    let metrics = EngineMetrics::default();
    guard.mark("recording past the cap through the metrics-owned registry");
    for i in 0..(SHAPE_ROWS_CAP as u64 + 64) {
        metrics.shape_stats().record(key_of(u64::MAX, i), 5, false);
    }

    let snapshot = metrics.shape_stats().snapshot();
    guard.mark("shape snapshot taken");
    assert!(
        snapshot.other.executions > 0,
        "the (other) fold is empty, so this test is vacuous"
    );
    assert_eq!(metrics.shape_stats().tracked(), snapshot.rows.len());
    assert_eq!(
        metrics.snapshot().shapes_tracked,
        snapshot.rows.len() as u64
    );
    guard.mark("the shapes_tracked gauge equals the live row count");
    guard.note("rows", snapshot.rows.len().to_string());
    guard.note("other executions", snapshot.other.executions.to_string());
}
