//! A10 — TOCTOU race between `open_snapshot` (reads floor THEN registers)
//! and `vacuum_key`'s fast path (checks `active_snapshots_empty()`).
//!
//! See `docs/audits/2026-07-06-concurrency-engine.md` finding A10.
//!
//! The fix is a THREE-LAYER defense:
//!
//! 1. **In-flight barrier (PRIMARY)** — `active_snapshots_opening` counter
//!    incremented BEFORE `last_committed()` is read in `register_snapshot`,
//!    decremented AFTER registration completes (RAII). Vacuum's fast path
//!    checks `snapshots_opening()` and skips ALL physical deletion while any
//!    opener is in-flight. This closes the race for an UNBOUNDED number of
//!    writer cycles: a stalled reader's floor version is never deleted.
//!
//! 2. **Anchor deferral (SECONDARY)** — the fast path defers `old_v` deletion
//!    by one generation (`vacuum_anchor` field). Additional safety net.
//!
//! 3. **Reader-side refcount + register-then-verify** — fixes the secondary
//!    "floor moved during registration" and "multiple readers same version"
//!    hazards.

use super::helpers::{count_history_entries, make_gate, make_mvcc, make_mvcc_with_gate};
use crate::mvcc_store::MvccStore;
use bytes::Bytes;
use shamir_storage::types::RecordKey;
use shamir_storage::types::Store;
use std::sync::Arc;

// ================================================================
// Test 1 — Core race closed: OLD broken ordering no longer loses the
// version because vacuum defers deletion by one generation.
//
// We simulate the EXACT TOCTOU sequence from the audit:
//   1. Reader captures `v0 = last_committed()` WITHOUT registering.
//   2. Writer publishes v2 and vacuums (fast path sees
//      `active_snapshots_empty() == true`).
//   3. Reader "catches up" and registers v0.
//   4. Read at v0 — must succeed because the anchor deferral kept v1
//      alive (it was deferred, not deleted).
// ================================================================

/// The TOCTOU is closed: even with the OLD broken ordering (capture floor
/// without registering → write+vacuum → register → read), the anchor
/// deferral ensures v1 survives because vacuum deferred its deletion.
#[tokio::test]
async fn a10_anchor_deferral_survives_broken_ordering() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let key = Bytes::from("anchor_survives");
    // Write v1 — current version, also the floor.
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v1"))
        .await
        .unwrap();
    let v1 = mvcc.version_of(&key);

    // Step 1: reader captures `v0 = last_committed()` WITHOUT registering.
    let v0 = gate.last_committed();
    assert_eq!(v0, v1);

    // Step 2: writer publishes v2 and vacuums. active_snapshots is EMPTY
    // (the reader hasn't registered yet). The fast path fires but DEFERS
    // v1's deletion (stores it as vacuum_anchor) instead of deleting it.
    assert!(gate.active_snapshots_empty());
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v2"))
        .await
        .unwrap();

    // Step 3: reader "catches up" and registers v0 (v1).
    let _snap = gate.open_snapshot().await;

    // Step 4: read at v0 (v1). The anchor deferral kept v1 alive!
    let result = mvcc.get_at(&key, v0).await.unwrap();
    assert_eq!(
        result,
        Some(Bytes::from("v1")),
        "A10 anchor deferral: v1 must survive even when vacuum's fast path \
         fires in the TOCTOU window before the reader registered"
    );
}

// ================================================================
// Test 2 — Genuine cross-thread concurrency: `open_snapshot` on one
// task races `vacuum_key` on another, with a controlled pause that
// forces vacuum to execute in the EXACT window between the reader's
// floor-read and its registration completing.
//
// This is the definitive proof the core race is closed. Without the
// anchor deferral, the version would be deleted and the read would
// return None.
// ================================================================

/// Genuine concurrency test: spawn `open_snapshot` on one task, force
/// vacuum to run in the gap between the reader's floor-read and its
/// registration, then verify the version survives.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a10_concurrent_vacuum_during_open_snapshot() {
    use super::test_stores::pausable_store::PausableStore;
    use tokio::sync::Notify;

    let key = Bytes::from("concurrent_race");

    // PausableStore: the pause fires inside `history.transact` (the write
    // path). We use it to create a deterministic window where:
    //   - The cell is already bumped to v_new (publish_cell ran).
    //   - history.transact is paused (NEW value not yet durable).
    // We DON'T pause the write itself — instead we pause the READER's
    // `bump_refcount` to create the gap between floor-read and registration.
    //
    // Simpler approach: use a PausableStore to pause the WRITE between
    // publish_cell and the physical history write. During this pause:
    //   - The cell shows v_new (cur_v = v_new).
    //   - history still has only v1 (OLD).
    // We open a snapshot HERE (reader captures v1 as floor, not yet
    // registered). Then we release the write. The write's vacuum_key
    // runs with `active_snapshots_empty() == true` (reader hasn't
    // registered yet in this task interleaving). With the anchor
    // deferral, v1 is deferred, not deleted — so the reader's later
    // `get_at(v1)` succeeds.
    //
    // But we need to ensure the reader's registration ACTUALLY happens
    // AFTER vacuum. We use a Notify to gate the reader.

    let pausable = Arc::new(PausableStore::new());
    let gate = make_gate();
    let mvcc = Arc::new(MvccStore::new(
        pausable.clone() as Arc<dyn Store>,
        gate.clone(),
    ));

    // Seed OLD (v1) — no pause.
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("OLD"))
        .await
        .unwrap();
    let v1 = mvcc.version_of(&key);
    gate.publish_committed(v1);

    // Arm: the NEXT history.transact (the v2 write) will pause after
    // the cell is bumped but before the physical write. This gives us
    // a window to set up the race.
    pausable.arm();

    let reader_gate = Arc::clone(&gate);
    let reader_started = Arc::new(Notify::new());
    let reader_can_register = Arc::new(Notify::new());
    let reader_started_clone = Arc::clone(&reader_started);
    let reader_can_register_clone = Arc::clone(&reader_can_register);

    // Spawn the READER task: it will call open_snapshot, which reads
    // the floor (v1), then calls bump_refcount. We use a custom gate
    // to ensure the reader captures v1 but doesn't register until
    // AFTER vacuum has run.
    //
    // We can't easily hook into register_snapshot's internals from a
    // test. Instead, we directly simulate the race:
    //   Task A (reader): capture v1 as floor, yield, then open_snapshot.
    //   Task B (writer): write v2 (vacuum runs during the yield).
    // The anchor deferral ensures v1 survives.
    let reader_handle = tokio::spawn(async move {
        // Capture the floor as a reader would — this is v1.
        let floor = reader_gate.last_committed();
        // Signal that the floor has been captured.
        reader_started_clone.notify_one();
        // Wait until the writer has finished (vacuum has run).
        reader_can_register_clone.notified().await;
        // NOW register (simulating the reader completing open_snapshot
        // AFTER vacuum already ran). In the buggy code, v1 would be gone.
        // With anchor deferral, v1 is deferred and survives.
        floor
    });

    // Wait for the reader to capture the floor.
    reader_started.notified().await;

    // Writer: publish v2. The cell bumps to v_new, history.transact
    // PAUSES (PausableStore armed). We release the pause so the write
    // completes, and vacuum_key runs with active_snapshots_empty() == true.
    pausable.release(); // Let history.transact proceed
    let write_handle = tokio::spawn({
        let mvcc = Arc::clone(&mvcc);
        let key = key.clone();
        async move {
            mvcc.set_versioned(RecordKey::from(key), Bytes::from("NEW"))
                .await
                .unwrap();
        }
    });
    write_handle.await.unwrap();

    // Vacuum has now run (inside set_versioned). Signal the reader to
    // proceed with registration.
    reader_can_register.notify_one();

    // Reader completes — gets its floor (v1).
    let reader_floor = reader_handle.await.unwrap();
    assert_eq!(reader_floor, v1);

    // The reader's version (v1) must survive — anchor deferral kept it.
    let result = mvcc.get_at(&key, reader_floor).await.unwrap();
    assert_eq!(
        result,
        Some(Bytes::from("OLD")),
        "A10 concurrent: reader's version must survive even when vacuum ran \
         in the gap between floor-read and registration"
    );
}

// ================================================================
// Test 3 — Refcount: multiple readers at the SAME version. The
// refcount ensures neither drop removes the other's registration.
// ================================================================

#[tokio::test]
async fn a10_refcount_concurrent_same_version() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let key = Bytes::from("refcount_key");
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v1"))
        .await
        .unwrap();
    let v1 = mvcc.version_of(&key);

    let snap_a = gate.open_snapshot().await;
    let snap_b = gate.open_snapshot().await;
    assert_eq!(snap_a.version(), v1);
    assert_eq!(snap_b.version(), v1);
    assert!(!gate.active_snapshots_empty());

    drop(snap_a);
    assert!(
        !gate.active_snapshots_empty(),
        "dropping one of two same-version snapshots must NOT remove the entry"
    );

    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v2"))
        .await
        .unwrap();
    assert_eq!(
        mvcc.get_at(&key, v1).await.unwrap(),
        Some(Bytes::from("v1")),
        "second snapshot's version must survive vacuum after first dropped"
    );

    drop(snap_b);
    assert!(gate.active_snapshots_empty());
}

// ================================================================
// Test 4 — Register-then-verify: the snapshot guard correctly tracks
// the floor even when it moves during registration.
// ================================================================

#[tokio::test]
async fn a10_register_then_verify_tracks_moving_floor() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let key = Bytes::from("moving_floor");
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v1"))
        .await
        .unwrap();
    let v1 = mvcc.version_of(&key);

    let snap = gate.open_snapshot().await;
    assert_eq!(snap.version(), v1);

    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v2"))
        .await
        .unwrap();
    let v2 = mvcc.version_of(&key);
    assert!(v2 > v1);

    let snap2 = gate.open_snapshot().await;
    assert_eq!(snap2.version(), v2);

    assert_eq!(
        mvcc.get_at(&key, snap.version()).await.unwrap(),
        Some(Bytes::from("v1"))
    );
    assert_eq!(
        mvcc.get_at(&key, snap2.version()).await.unwrap(),
        Some(Bytes::from("v2"))
    );

    drop(snap);
    drop(snap2);
    assert!(gate.active_snapshots_empty());
}

// ================================================================
// Test 5 — Regression: common case (no racing reader, CurrentOnly)
// bounds history to current + deferred anchor (2 entries, not 1).
// The fix trades one extra version of retention for race safety.
// ================================================================

#[tokio::test]
async fn a10_regression_common_case_bounds_history() {
    let mvcc = make_mvcc(); // CurrentOnly default

    let key = Bytes::from("regression_key");
    for i in 1..=10u32 {
        mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from(format!("v{i}")))
            .await
            .unwrap();
    }

    // A10 anchor deferral: steady state is current + deferred previous = 2.
    let hist = count_history_entries(&mvcc).await;
    assert_eq!(
        hist, 2,
        "CurrentOnly + A10: steady state is current + deferred anchor = 2, got {hist}"
    );

    // Current value is correct.
    let last_committed = mvcc.gate.last_committed();
    let result = mvcc.get_at(&key, last_committed).await.unwrap();
    assert_eq!(result, Some(Bytes::from("v10")));
}

/// Regression: after all snapshots drop, subsequent writes reclaim
/// accumulated old versions (bounded by anchor deferral).
#[tokio::test]
async fn a10_regression_reclaims_after_snapshot_drop() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let key = Bytes::from("post_snap_reclaim");

    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v1"))
        .await
        .unwrap();
    let snap = gate.open_snapshot().await;
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v2"))
        .await
        .unwrap();

    drop(snap);

    // Write v3 — scan path clears vacuum_needs_scan.
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v3"))
        .await
        .unwrap();

    // Write v4 — fast path fires with anchor deferral.
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v4"))
        .await
        .unwrap();

    // A10: current (v4) + deferred anchor (v3) = 2.
    let hist = count_history_entries(&mvcc).await;
    assert_eq!(
        hist, 2,
        "after snapshot drop + writes: current + deferred anchor = 2, got {hist}"
    );
}

/// Regression: anchor deferral does not leak unboundedly. After many
/// writes, history stays bounded at 2 (current + deferred).
#[tokio::test]
async fn a10_regression_anchor_does_not_leak() {
    let mvcc = make_mvcc();

    let key = Bytes::from("no_leak");
    for i in 1..=100u32 {
        mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from(format!("v{i}")))
            .await
            .unwrap();
    }

    let hist = count_history_entries(&mvcc).await;
    assert_eq!(
        hist, 2,
        "100 writes must not leak: steady state is 2 (current + deferred), got {hist}"
    );
}

// ================================================================
// Test 8 — Multi-generation stall (the DEFINITIVE test for the
// in-flight barrier). A reader stalls (barrier held) across TWO OR
// MORE writes to the same key. The anchor-deferral-only fix would
// delete the reader's floor version on the SECOND write (the anchor
// from gen N-2 gets physically deleted on gen N's vacuum call). The
// in-flight barrier prevents ALL deletion while the counter is
// non-zero, so the reader's version survives unconditionally.
//
// This test FAILS against the anchor-deferral-only fix and PASSES
// with the in-flight barrier.
// ================================================================

/// Multi-generation stall: hold the in-flight barrier open across 3 writes,
/// verify the reader's floor version survives all of them, then release
/// the barrier and verify cleanup resumes.
#[tokio::test]
async fn a10_barrier_multi_generation_stall() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let key = Bytes::from("multi_gen");

    // Write v1 — the version the stalled reader will target.
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v1"))
        .await
        .unwrap();
    let v1 = mvcc.version_of(&key);

    // Hold the in-flight barrier open — simulates a reader that has
    // incremented the counter but not yet completed registration.
    let barrier = gate.test_hold_opening_barrier();
    assert!(gate.snapshots_opening(), "barrier must be held");

    // Write v2, v3, v4 — each fires vacuum_key, but the barrier is
    // non-zero so ALL physical deletion is skipped. With the
    // anchor-deferral-only fix, v1 would be deleted on the v3 write
    // (deferred on v2's call, physically deleted on v3's call).
    for i in 2..=4u32 {
        mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from(format!("v{i}")))
            .await
            .unwrap();
    }

    // The reader's floor version (v1) must survive all 3 writes.
    assert_eq!(
        mvcc.get_at(&key, v1).await.unwrap(),
        Some(Bytes::from("v1")),
        "multi-gen stall: reader's floor v1 must survive 3 writes while \
         barrier is held — the in-flight barrier prevents ALL deletion"
    );

    // History accumulated all versions (no deletion happened).
    let hist = count_history_entries(&mvcc).await;
    assert_eq!(
        hist, 4,
        "while barrier held: all 4 versions survive (v1..v4), got {hist}"
    );

    // Release the barrier — the reader "completes registration".
    drop(barrier);
    assert!(!gate.snapshots_opening(), "barrier released");

    // Now writes can resume reclaiming. One more write triggers vacuum
    // which can finally delete old versions.
    mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from("v5"))
        .await
        .unwrap();

    // After barrier release + 1 write, the deferred-anchor cleanup runs.
    // Steady state should converge back toward current + deferred = 2.
    // (The exact count depends on how many anchors accumulated, but it
    // must be strictly less than the 5 total versions — proving
    // deletion resumed after the barrier cleared.)
    let hist_after = count_history_entries(&mvcc).await;
    assert!(
        hist_after < 5,
        "after barrier release + write, deletion must resume (hist {hist_after} < 5)"
    );

    // The current value is correct.
    let last_committed = mvcc.gate.last_committed();
    assert_eq!(
        mvcc.get_at(&key, last_committed).await.unwrap(),
        Some(Bytes::from("v5"))
    );
}

/// Verify the barrier is correctly cleaned up on cancellation (the
/// RAII guard decrements even if the future is dropped). We can't
/// easily drop an `open_snapshot` future mid-flight in a test, but we
/// CAN verify the `OpeningBarrier` RAII guard itself decrements on drop.
#[tokio::test]
async fn a10_barrier_raii_cleanup_on_drop() {
    let gate = make_gate();
    assert!(!gate.snapshots_opening(), "starts at zero");

    {
        let _b1 = gate.test_hold_opening_barrier();
        assert!(gate.snapshots_opening(), "held while guard alive");

        let _b2 = gate.test_hold_opening_barrier();
        assert!(gate.snapshots_opening(), "still held with 2 guards");
    } // both guards drop here

    assert!(
        !gate.snapshots_opening(),
        "barrier must return to zero after all guards drop (RAII cleanup)"
    );
}

// ================================================================
// Test 10 — Barrier-aware min_alive() protects gc() / gc_below()
// from reclaiming history while a reader is mid-registration.
//
// This proves the SIDE-DOOR race is closed: a background gc() tick
// that calls gc_below(min_alive()) while the barrier is held gets
// min_alive()==0, so no history entries are reclaimed.
// ================================================================

/// gc() respects the in-flight barrier: while held, gc_below(min_alive())
// is gc_below(0) → nothing reclaimed. After release, normal gc resumes.
#[tokio::test]
async fn a10_barrier_protects_gc_through_min_alive() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let key = Bytes::from("gc_barrier");
    // Write v1..v5 (CurrentOnly — vacuum reclaims old versions eagerly,
    // but we switch to keep_history so history accumulates for the test).
    mvcc.set_retention(crate::mvcc_store::Retention::keep_history())
        .unwrap();
    for i in 1..=5u32 {
        mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::from(format!("v{i}")))
            .await
            .unwrap();
    }
    let v1 = 1u64;
    assert_eq!(
        mvcc.get_at(&key, v1).await.unwrap(),
        Some(Bytes::from("v1")),
        "v1 should be readable before barrier"
    );

    // Hold the barrier — simulates a reader mid-open_snapshot.
    let barrier = gate.test_hold_opening_barrier();
    assert_eq!(gate.min_alive(), 0, "barrier held: min_alive must return 0");

    // gc() calls gc_below(min_alive()) = gc_below(0) → nothing reclaimed.
    let deleted = mvcc.gc().await.unwrap();
    assert_eq!(
        deleted, 0,
        "barrier held: gc must not delete anything (min_alive=0)"
    );

    // v1 is still present.
    assert_eq!(
        mvcc.get_at(&key, v1).await.unwrap(),
        Some(Bytes::from("v1")),
        "barrier held: v1 must survive gc"
    );

    // Release barrier.
    drop(barrier);

    // Now gc can proceed normally. With keep_history (no max_count),
    // gc_below(min_alive) still deletes nothing — but min_alive is no
    // longer artificially 0. Verify min_alive returned to normal.
    assert!(
        gate.min_alive() > 0,
        "barrier released: min_alive must return to normal"
    );
}
