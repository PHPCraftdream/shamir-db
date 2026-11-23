use std::sync::mpsc;
use std::sync::{Arc, Barrier};

use super::super::{ShapeCounters, ShapeRegistry, ShapeStatsSnapshot, SHAPE_ROWS_CAP};

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

/// Signed total of the same sum, so the "never negative" invariant is a real
/// assertion rather than a vacuous unsigned comparison.
fn total_counted_signed(snapshot: &ShapeStatsSnapshot) -> i128 {
    snapshot
        .rows
        .iter()
        .map(|(_, counters)| i128::from(counters.executions))
        .sum::<i128>()
        + i128::from(snapshot.other.executions)
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
        total_counted_signed(snapshot) >= 0,
        "negative execution total in {snapshot:?}"
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
    let registry = Arc::new(ShapeRegistry::new());
    // Handshake: every recorder starts only once all eight are ready, so the
    // cap and the fold are exercised concurrently.
    let barrier = Arc::new(Barrier::new(8));
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

    let snapshot = registry.snapshot();
    assert!(snapshot.rows.len() <= SHAPE_ROWS_CAP);
    assert_eq!(counted(&snapshot), 1000);
    assert!(snapshot.other.executions > 0);
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
    let registry = Arc::new(ShapeRegistry::new());
    for index in 0..SHAPE_ROWS_CAP as u64 {
        registry.record(key_of(u64::MAX, index), 3, false);
    }
    assert_eq!(registry.tracked(), SHAPE_ROWS_CAP);

    // Handshake: all eight threads are released together, so the 800 new
    // keys contend with a registry that is already at its cap.
    let barrier = Arc::new(Barrier::new(8));
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

    let snapshot = registry.snapshot();
    assert!(snapshot.rows.len() <= SHAPE_ROWS_CAP);
    assert_eq!(counted(&snapshot), SHAPE_ROWS_CAP as u64 + 800);
    assert!(snapshot.other.executions > 0);
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
    // The pinned state will also carry the parked recorder's reservation.
    let old_tracked = settled.tracked + 1;

    let (ready_tx, ready_rx) = mpsc::channel::<()>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let parked_registry = &registry;
    std::thread::scope(|scope| {
        let worker = scope.spawn(move || {
            parked_registry.record_paused(key_of(u64::MAX, 1), 1, false, || {
                ready_tx.send(()).expect("handshake: recorder parked");
                go_rx.recv().expect("handshake: recorder released");
            });
        });
        ready_rx
            .recv()
            .expect("recorder reached the reserve-insert window");

        let before = registry.snapshot();
        assert_single_generation(&before, 2, 2, old_tracked);

        registry.reset();
        // Allowed to observe either generation, but never a mix of both.
        let middle = registry.snapshot();
        assert_single_generation(&middle, 2, 2, old_tracked);

        go_tx.send(()).expect("release the parked recorder");
        worker.join().expect("parked recorder");
    });

    let after = registry.snapshot();
    assert_single_generation(&after, 2, 2, old_tracked);
    assert!(after.rows.is_empty());
    assert_eq!(after.other, ShapeCounters::default());
    assert_eq!(after.tracked, 0);
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
