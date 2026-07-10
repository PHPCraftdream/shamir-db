use super::helpers::{count_history_entries, make_gate, make_mvcc, make_mvcc_with_gate};
use crate::mvcc_store::Retention;
use bytes::Bytes;

// ================================================================
// T1b.1 — eager vacuum (CurrentOnly default) tests.
// ================================================================

/// CurrentOnly store, no snapshots: write the same key 5 times; eager
/// vacuum reclaims superseded history on every write, so `history` holds
/// 0 old versions afterward while `get_at` at the floor still returns
/// the current value.
#[tokio::test]
async fn eager_vacuum_currentonly_bounds_history() {
    let mvcc = make_mvcc();

    let key = Bytes::from("vacuum_key");
    for i in 1..=5u32 {
        mvcc.set_versioned(key.clone(), Bytes::from(format!("v{i}")))
            .await
            .unwrap();
    }

    // C1: the current version lives in the log and is SACRED (cur_v guard).
    // A10 anchor deferral: the immediately-prior version (v4) is kept as a
    // deferred anchor — 2 entries total (current v5 + deferred v4).
    let hist = count_history_entries(&mvcc).await;
    assert_eq!(
        hist, 2,
        "C1+A10: CurrentOnly eager vacuum leaves current + deferred anchor = 2, got {hist}"
    );

    // The current value is still readable at the floor.
    let last_committed = mvcc.gate.last_committed();
    let result = mvcc.get_at(&key, last_committed).await.unwrap();
    assert_eq!(
        result,
        Some(Bytes::from("v5")),
        "get_at at last_committed must return the current value"
    );
}

/// A live snapshot pins the version it needs: overwriting the key does
/// NOT reclaim the version the snapshot may still read. Dropping the
/// snapshot unpins it, and a subsequent write's eager vacuum reclaims.
#[tokio::test]
async fn eager_vacuum_keeps_versions_pinned_by_live_snapshot() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let key = Bytes::from("pinned_key");

    // Write v1 (no snapshot) — publishes last_committed = v1.
    mvcc.set_versioned(key.clone(), Bytes::from("v1"))
        .await
        .unwrap();
    let v1 = mvcc.version_of(&key);

    // Open a snapshot at v1 — pins min_alive to v1.
    let snap = gate.open_snapshot().await;
    let snap_v = snap.version();
    assert_eq!(snap_v, v1);

    // Overwrite with v2. Eager vacuum runs but min_alive == v1 protects
    // the v1 history entry (gc_below only removes < min_alive).
    mvcc.set_versioned(key.clone(), Bytes::from("v2"))
        .await
        .unwrap();

    // The snapshot at v1 still reads v1 via log range-scan.
    let result = mvcc.get_at(&key, snap_v).await.unwrap();
    assert_eq!(
        result,
        Some(Bytes::from("v1")),
        "live snapshot must still read the pinned prior version"
    );

    // Drop the snapshot → min_alive advances. A further write's eager
    // vacuum can now reclaim the unpinned old version.
    drop(snap);
    mvcc.set_versioned(key.clone(), Bytes::from("v3"))
        .await
        .unwrap();

    // After reclaim, a read at the OLD snapshot version (v1) should no
    // longer find the reclaimed entry (it was < min_alive). The current
    // value is still correct.
    let last_committed = mvcc.gate.last_committed();
    let current = mvcc.get_at(&key, last_committed).await.unwrap();
    assert_eq!(current, Some(Bytes::from("v3")));
}

/// Deterministic interleaving (PausableStore): a write+eager-reclaim
/// interleaved with an `open_snapshot`. The just-opened snapshot must
/// NEVER read `None` for a version it should see — the register-before-use
/// ordering + min_alive floor protect it. (§4.1-class race; loom sweep
/// deferred to T1d.)
///
/// PausableStore is wired as `history` (the sole write target). The pause
/// fires inside `history.set` — AFTER `publish_cell` advances the cell to
/// v_new. A snapshot opened inside this window sees `cur_v = v_new > snap_v`
/// → log range-scan → finds OLD.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn eager_vacuum_race_open_snapshot() {
    use super::test_stores::pausable_store::PausableStore;
    use crate::mvcc_store::MvccStore;
    use shamir_storage::types::Store;
    use std::sync::Arc;

    let key = Bytes::from("race_key");
    let old_val = Bytes::from("OLD");
    let new_val = Bytes::from("NEW");

    // PausableStore wired as `history` (sole write target).
    // CurrentOnly default — eager vacuum active.
    let pausable = Arc::new(PausableStore::new());
    let gate = make_gate();
    let mvcc = Arc::new(MvccStore::new(
        pausable.clone() as Arc<dyn Store>,
        gate.clone(),
    ));

    // Seed OLD (no snapshot) — lands in the log (sole write), cell published.
    mvcc.set_versioned(key.clone(), old_val.clone())
        .await
        .unwrap();
    let v_seed = mvcc.version_of(&key);
    gate.publish_committed(v_seed);

    // Arm: the next `history.set` (inside the NEW write) will pause BEFORE
    // the physical log write. The write sequence is:
    //   publish_cell(v_new) → history.set(PAUSE) → publish_committed_max → eager_vacuum
    // When `entered` fires, the cell already carries v_new but NEW is
    // not yet in the log.
    pausable.arm();

    let mvcc_w = Arc::clone(&mvcc);
    let key_w = key.clone();
    let new_val_w = new_val.clone();
    let write_handle = tokio::spawn(async move {
        mvcc_w.set_versioned(key_w, new_val_w).await.unwrap();
    });

    // Wait until the write is paused inside history.set — cell already
    // advanced to v_new, but NEW not yet committed to the log.
    pausable.entered.notified().await;

    // Open a snapshot HERE — interleaved between publish_cell and the
    // log write landing. The snapshot registers in active_snapshots
    // BEFORE it is usable, so the eager vacuum's min_alive will include
    // this snapshot's version.
    let snap = gate.open_snapshot().await;
    let snap_v = snap.version();

    // Release: history.set commits NEW to the log, then
    // publish_committed_max, then eager_vacuum runs with min_alive
    // that now includes snap_v.
    pausable.release();
    write_handle.await.unwrap();

    // The snapshot must NOT read None for a version it should see.
    // snap_v == v_seed (the published floor before the NEW write).
    // The snapshot predates NEW; cur_v = v_new > snap_v → log range-scan
    // → finds OLD at v_seed (pinned by min_alive).
    let seen = mvcc.get_at(&key, snap_v).await.unwrap();
    assert!(
        seen.is_some(),
        "snapshot opened mid-write must never read None for a version it should see"
    );
    assert_eq!(
        seen,
        Some(old_val),
        "snapshot predating NEW must see OLD (archived, pinned by min_alive)"
    );
}

/// KeepHistory store: write the key 5 times; all old versions remain in
/// history (no eager reclaim).
#[tokio::test]
async fn keephistory_no_eager_vacuum() {
    let mvcc = make_mvcc();
    mvcc.set_retention(Retention::keep_history()).unwrap();

    let key = Bytes::from("keep_key");
    for i in 1..=5u32 {
        mvcc.set_versioned(key.clone(), Bytes::from(format!("v{i}")))
            .await
            .unwrap();
    }

    // C1: all 5 versions in the log (v1..v4 prior + v5 current), no eager vacuum.
    let hist = count_history_entries(&mvcc).await;
    assert_eq!(
        hist, 5,
        "C1: KeepHistory retains all 5 versions in the log (4 prior + current), got {hist}"
    );

    // Current value still correct.
    let last_committed = mvcc.gate.last_committed();
    let result = mvcc.get_at(&key, last_committed).await.unwrap();
    assert_eq!(result, Some(Bytes::from("v5")));
}

// ================================================================
// T1b.2 — orthogonal retention knobs (per-key count vacuum).
//
// All counts below assume NO live snapshot unless the test opens one.
// With no snapshot: min_alive == last_committed == current, so the
// anchor is None (a fresh snapshot opens at `current` and reads the log directly).
// ================================================================

/// 1. `max_count: Some(3)`, 6 writes (5 old), no snapshot → exactly 3 old
/// remain; the 2 oldest are reclaimed; kept versions are reachable.
#[tokio::test]
async fn retention_count_only_keeps_last_n() {
    let mvcc = make_mvcc();
    mvcc.set_retention(Retention {
        max_age_secs: None,
        max_count: Some(3),
        min_count: None,
    })
    .unwrap();

    let key = Bytes::from("count_key");
    let mut versions = Vec::new();
    for i in 1..=6u32 {
        mvcc.set_versioned(key.clone(), Bytes::from(format!("v{i}")))
            .await
            .unwrap();
        versions.push(mvcc.version_of(&key));
    }

    // C1: 6 versions in the log. cur_v=v6 occupies idx 0 (skipped by guard).
    // max_count=3 keeps idx 1,2 (v5, v4). v3..v1 (idx 3..5) reclaimed.
    // Total: v6 (cur_v) + v5, v4 = 3.
    let hist = count_history_entries(&mvcc).await;
    assert_eq!(
        hist, 3,
        "C1: max_count=3 keeps current + 2 within window = 3"
    );

    // The newest 3 are reachable via get_at.
    for &v in &versions[versions.len() - 3..] {
        let result = mvcc.get_at(&key, v).await.unwrap();
        assert!(result.is_some(), "kept version {v} must be reachable");
    }
    // Current value is correct.
    let last_committed = mvcc.gate.last_committed();
    assert_eq!(
        mvcc.get_at(&key, last_committed).await.unwrap(),
        Some(Bytes::from("v6"))
    );
}

/// 2. Default `max_count: Some(0)` (CurrentOnly), 4 writes, no snapshot →
/// 0 old versions remain (only the current entry in the log).
#[tokio::test]
async fn retention_current_only_is_max_count_zero() {
    let mvcc = make_mvcc(); // default = CurrentOnly

    let key = Bytes::from("co_key");
    for i in 1..=4u32 {
        mvcc.set_versioned(key.clone(), Bytes::from(format!("v{i}")))
            .await
            .unwrap();
    }

    // C1: the current version (v4) survives in the log (cur_v guard).
    // A10 anchor deferral: v3 is kept as deferred anchor — 2 entries total.
    let hist = count_history_entries(&mvcc).await;
    assert_eq!(
        hist, 2,
        "C1+A10: max_count=0 + deferred anchor = current + previous = 2"
    );

    let last_committed = mvcc.gate.last_committed();
    assert_eq!(
        mvcc.get_at(&key, last_committed).await.unwrap(),
        Some(Bytes::from("v4"))
    );
}

/// 3. `max_count: None, min_count: Some(2)`, 5 writes (4 old) → all 4 old
/// remain. No count cap → early return; `min_count` alone is a satisfied
/// floor, not a reclaimer.
#[tokio::test]
async fn retention_min_count_standalone_keeps_all() {
    let mvcc = make_mvcc();
    mvcc.set_retention(Retention {
        max_age_secs: None,
        max_count: None,
        min_count: Some(2),
    })
    .unwrap();

    let key = Bytes::from("floor_key");
    for i in 1..=5u32 {
        mvcc.set_versioned(key.clone(), Bytes::from(format!("v{i}")))
            .await
            .unwrap();
    }

    // C1: max_count=None → vacuum_key returns early. All 5 versions in the
    // log survive (4 prior + 1 current).
    let hist = count_history_entries(&mvcc).await;
    assert_eq!(
        hist, 5,
        "C1: max_count=None keeps ALL 5 versions in the log (4 prior + current)"
    );
}

/// 4. `max_count: Some(0)` with a live snapshot: the versions the snapshot
/// may read survive (protected by `>= min_alive`, branch (b)). After
/// dropping the snapshot, a further write reclaims everything (current
/// only). No anchor is needed in either phase of this scenario.
#[tokio::test]
async fn retention_keeps_pinned_and_anchor_with_snapshot() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let key = Bytes::from("pin_key");
    // Write v1 — log has v1 as the current entry, last_committed=v1.
    mvcc.set_versioned(key.clone(), Bytes::from("v1"))
        .await
        .unwrap();
    let v1 = mvcc.version_of(&key);

    // Open a snapshot at v1 — pins min_alive=v1.
    let snap = gate.open_snapshot().await;
    let snap_v = snap.version();
    assert_eq!(snap_v, v1);

    // Overwrite to v2, v3. Each vacuum runs with min_alive=v1: v1 (then
    // v1,v2) are `>= min_alive` → sacred (branch b), never reclaimed.
    mvcc.set_versioned(key.clone(), Bytes::from("v2"))
        .await
        .unwrap();
    mvcc.set_versioned(key.clone(), Bytes::from("v3"))
        .await
        .unwrap();

    // The snapshot at v1 still reads v1 via log range-scan.
    let result = mvcc.get_at(&key, snap_v).await.unwrap();
    assert_eq!(
        result,
        Some(Bytes::from("v1")),
        "live snapshot must still read the pinned prior version"
    );

    // Drop the snapshot → min_alive advances to last_committed. A further
    // write's vacuum now reclaims every old version (no anchor: no live
    // snapshot remains).
    drop(snap);
    mvcc.set_versioned(key.clone(), Bytes::from("v4"))
        .await
        .unwrap();

    let hist = count_history_entries(&mvcc).await;
    assert_eq!(
        hist, 1,
        "C1: unpinned old versions reclaimed, current v4 stays in the log (1 entry)"
    );
}

/// 5. The case the anchor is FOR: with `max_count: Some(0)` and a snapshot
/// pinned BELOW the most recent write, the single largest version
/// `< min_alive` (the anchor) survives alongside every version
/// `>= min_alive`; strictly-below-anchor versions are reclaimed; the
/// snapshot reads the correct value at its version.
#[tokio::test]
async fn retention_anchor_below_min_alive() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let key = Bytes::from("anchor_key");
    // Accumulate full history first under KeepHistory (the default
    // CurrentOnly policy would reclaim on every write, leaving nothing to
    // exercise the anchor path).
    mvcc.set_retention(Retention::keep_history()).unwrap();
    // Write 10 times → log has all 10 versions (v1..v10),
    // last_committed=10.
    for i in 1..=10u32 {
        mvcc.set_versioned(key.clone(), Bytes::from(format!("v{i}")))
            .await
            .unwrap();
    }
    // C1: history has v1..v10 (all written as current).
    let hist_before = count_history_entries(&mvcc).await;
    assert_eq!(
        hist_before, 10,
        "C1: KeepHistory retains all 10 versions in the log (including current v10)"
    );

    // Open a snapshot at v10 — pins min_alive=10.
    let snap = gate.open_snapshot().await;
    let snap_v = snap.version();
    assert_eq!(snap_v, 10);

    // Switch to the aggressive policy JUST before the reclaiming write.
    mvcc.set_retention(Retention {
        max_age_secs: None,
        max_count: Some(0),
        min_count: None,
    })
    .unwrap();

    // Overwrite once more: archives v10 → history@v10, assigns v11.
    // Vacuum: keep_n=0, min_alive=10, live snapshot → anchor = max<10 = v9.
    //   v10: >= min_alive → kept (b)
    //   v9: == anchor → kept (c)
    //   v8..v1: reclaimed
    mvcc.set_versioned(key.clone(), Bytes::from("v11"))
        .await
        .unwrap();

    // C1: 3 entries survive — current v11 (cur_v guard), pinned v10 (≥ min_alive),
    // and anchor v9.
    let hist = count_history_entries(&mvcc).await;
    assert_eq!(
        hist, 3,
        "C1: max_count=0 + live snapshot: current + pinned (>=min_alive) + anchor"
    );

    // The snapshot at v10 reads v10 (the value current at its version).
    let result = mvcc.get_at(&key, snap_v).await.unwrap();
    assert_eq!(
        result,
        Some(Bytes::from("v10")),
        "snapshot at v10 must read v10 (pinned by min_alive)"
    );

    // Verify the surviving entries are exactly {v9, v10} by reading them.
    let v9 = 9u64;
    let v10 = 10u64;
    assert!(
        mvcc.get_at(&key, v9).await.unwrap().is_some(),
        "anchor v9 (largest < min_alive) must survive"
    );
    assert!(
        mvcc.get_at(&key, v10).await.unwrap().is_some(),
        "pinned v10 (>= min_alive) must survive"
    );
    // A strictly-below-anchor version was reclaimed.
    let v8 = 8u64;
    assert!(
        mvcc.get_at(&key, v8).await.unwrap().is_none(),
        "v8 (below anchor) must be reclaimed"
    );
}

/// 6. Deterministic interleaving (PausableStore): a write+count-vacuum
/// interleaved with an `open_snapshot`. The snapshot must NEVER read None
/// for a version it should see — the min_alive floor holds above the count
/// knobs. (§4.1-class race; loom sweep deferred to T1d.)
///
/// PausableStore is wired as `history` (the sole write target). The pause
/// fires inside `history.set` — AFTER `publish_cell` advances the cell to
/// v_new. A snapshot opened inside this window sees `cur_v = v_new > snap_v`
/// → log range-scan → finds OLD.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retention_race_open_snapshot_with_count() {
    use super::test_stores::pausable_store::PausableStore;
    use crate::mvcc_store::MvccStore;
    use shamir_storage::types::Store;
    use std::sync::Arc;

    let key = Bytes::from("race_key");
    let old_val = Bytes::from("OLD");
    let new_val = Bytes::from("NEW");

    // PausableStore wired as `history` (sole write target).
    let pausable = Arc::new(PausableStore::new());
    let gate = make_gate();
    let mvcc = Arc::new(MvccStore::new(
        pausable.clone() as Arc<dyn Store>,
        gate.clone(),
    ));
    // Aggressive vacuum — but min_alive must still protect pinned versions.
    mvcc.set_retention(Retention {
        max_age_secs: None,
        max_count: Some(0),
        min_count: None,
    })
    .unwrap();

    // Seed OLD — lands in the log (sole write), cell published.
    mvcc.set_versioned(key.clone(), old_val.clone())
        .await
        .unwrap();
    let v_seed = mvcc.version_of(&key);
    gate.publish_committed(v_seed);

    // Arm: the next `history.set` (inside the NEW write) will pause.
    // The write sequence is:
    //   publish_cell(v_new) → history.set(PAUSE) → publish_committed_max → vacuum_key
    pausable.arm();

    let mvcc_w = Arc::clone(&mvcc);
    let key_w = key.clone();
    let new_val_w = new_val.clone();
    let write_handle = tokio::spawn(async move {
        mvcc_w.set_versioned(key_w, new_val_w).await.unwrap();
    });

    // Wait for the write to pause inside history.set (after publish_cell,
    // before publish_committed_max + vacuum_key).
    pausable.entered.notified().await;

    // Open a snapshot mid-write — registers before usable.
    let snap = gate.open_snapshot().await;
    let snap_v = snap.version();

    // Release: history.set commits NEW → publish_committed_max →
    // vacuum_key runs with min_alive that now includes snap_v.
    pausable.release();
    write_handle.await.unwrap();

    let seen = mvcc.get_at(&key, snap_v).await.unwrap();
    assert!(
        seen.is_some(),
        "snapshot opened mid-write must never read None for a version it should see"
    );
    assert_eq!(
        seen,
        Some(old_val),
        "snapshot predating NEW must see OLD (pinned by min_alive above max_count=0)"
    );
}

/// 7. `set_retention` swaps the whole struct; `retention()` reflects it;
/// `validate` rejects `min_count > max_count`.
#[tokio::test]
async fn retention_patch_independent() {
    let mvcc = make_mvcc();

    // Default is CurrentOnly (max_count: Some(0)).
    assert_eq!(**mvcc.retention(), Retention::current_only());

    // Swap to keep_history.
    mvcc.set_retention(Retention::keep_history()).unwrap();
    assert_eq!(**mvcc.retention(), Retention::keep_history());

    // Swap to a custom policy.
    let custom = Retention {
        max_age_secs: Some(3600),
        max_count: Some(10),
        min_count: Some(2),
    };
    mvcc.set_retention(custom).unwrap();
    assert_eq!(**mvcc.retention(), custom);

    // validate rejects min_count > max_count — old policy kept.
    let invalid = Retention {
        max_age_secs: None,
        max_count: Some(1),
        min_count: Some(5),
    };
    assert!(mvcc.set_retention(invalid).is_err(), "min>max must reject");
    // Previous (custom) policy survives the rejected swap.
    assert_eq!(
        **mvcc.retention(),
        custom,
        "rejected swap must not change policy"
    );
}

// ================================================================
// T1c — per-version commit timestamp + max_age retention (AGE axis).
//
// All tests freeze the clock via `set_test_now` for determinism.
// `count_history_entries` counts only version-keys (ts-keys excluded).
// ================================================================

/// 1. `max_age_secs: Some(60)` (KeepHistory base, no count cap), no
/// snapshot: a version written 100s ago (ts=0, now=100_000) is reclaimed;
/// versions within the 60s window are kept.
#[tokio::test]
async fn max_age_reclaims_old_versions() {
    let mvcc = make_mvcc();
    mvcc.set_retention(Retention {
        max_age_secs: Some(60),
        max_count: None,
        min_count: None,
    })
    .unwrap();

    let key = Bytes::from("age_key");
    // v1 at an early frozen time (1ms — 0 is the "real clock" sentinel).
    mvcc.set_test_now(1);
    mvcc.set_versioned(key.clone(), Bytes::from("old"))
        .await
        .unwrap();
    let v1 = mvcc.version_of(&key);

    // 100s later (100_001ms) — v1 (ts=1) is now 100s old (> 60s cap).
    mvcc.set_test_now(100_001);
    mvcc.set_versioned(key.clone(), Bytes::from("new1"))
        .await
        .unwrap();
    mvcc.set_versioned(key.clone(), Bytes::from("new2"))
        .await
        .unwrap();

    // C1: v3 (current, cur_v guard) + v2 (age-kept) = 2 entries. v1 reclaimed by age.
    let hist = count_history_entries(&mvcc).await;
    assert_eq!(
        hist, 2,
        "C1: max_age=60s reclaims v1; v2 age-kept + v3 current = 2"
    );

    // The reclaimed v1 is no longer reachable via get_at.
    let stale = mvcc.get_at(&key, v1).await.unwrap();
    assert!(stale.is_none(), "reclaimed v1 must not be reachable");

    // The kept v2 is reachable; current is correct.
    let last_committed = mvcc.gate.last_committed();
    assert_eq!(
        mvcc.get_at(&key, last_committed).await.unwrap(),
        Some(Bytes::from("new2"))
    );
}

/// 2. `min_count` FLOOR overrides the age cap: `max_age_secs: Some(1)`,
/// `min_count: Some(3)` — write 5 versions all well past the age cap; the
/// newest 3 survive (min_count protects them from the age cap).
#[tokio::test]
async fn min_count_floor_overrides_age() {
    let mvcc = make_mvcc();
    mvcc.set_retention(Retention {
        max_age_secs: Some(1),
        max_count: None,
        min_count: Some(3),
    })
    .unwrap();

    let key = Bytes::from("floor_age_key");
    // Write v1..v5 all at frozen ts=1 (0 is the "real clock" sentinel).
    // Vacuum runs after each write, but with clock=1 the age cutoff
    // saturates to 0 (1 - 1000 = 0) and ts=1 is not < 0, so nothing is
    // reclaimed yet — history accumulates v1..v4.
    mvcc.set_test_now(1);
    for i in 1..=5u32 {
        mvcc.set_versioned(key.clone(), Bytes::from(format!("v{i}")))
            .await
            .unwrap();
    }

    // Advance the clock to 10s (10x the 1s cap) and write once more to
    // trigger a vacuum. Now v1..v5 (ts=1) are all past the 1s cap.
    mvcc.set_test_now(10_000);
    mvcc.set_versioned(key.clone(), Bytes::from("v6"))
        .await
        .unwrap();

    // C1: cur_v=v6 at idx 0 (sacred). min_count=3 protects idx 0..2, but idx 0
    // is already sacred. v5(idx 1), v4(idx 2) protected by floor. v3..v1 past
    // age cap and beyond floor → reclaimed. Total: v6 + v5 + v4 = 3.
    let hist = count_history_entries(&mvcc).await;
    assert_eq!(
        hist, 3,
        "C1: min_count=3 floor + current = 3 (v6 cur_v, v5, v4 floor), past age cap"
    );
}

/// 3. age ∧ count intersection: a version is reclaimed only when BOTH
/// caps agree to drop it (the KEEP condition is OR — within either cap's
/// window). Two phases:
///   (a) beyond count BUT within age window → KEPT (age protects);
///   (b) within count BUT beyond age window → KEPT (count protects).
#[tokio::test]
async fn age_and_count_intersect_tighter_prunes() {
    // Phase (a): a version beyond the count cap but within the age window
    // is KEPT — age protects it from the count cap.
    let mvcc = make_mvcc();
    mvcc.set_retention(Retention {
        max_age_secs: Some(60),
        max_count: Some(1),
        min_count: None,
    })
    .unwrap();
    let key = Bytes::from("intersect_a");
    // Write 3 versions all at the same frozen time. During accumulation
    // the age cutoff saturates to 0 (now=50_000, cutoff=0), so age keeps
    // everything and the count cap alone can't reclaim (AND semantics).
    mvcc.set_test_now(50_000);
    for i in 1..=3u32 {
        mvcc.set_versioned(key.clone(), Bytes::from(format!("a{i}")))
            .await
            .unwrap();
    }
    // C1: a3 (current, cur_v guard) + a1, a2 (age-kept) = 3 entries.
    let hist_a = count_history_entries(&mvcc).await;
    assert_eq!(
        hist_a, 3,
        "C1: phase (a): current + age-kept versions = 3 (age protects beyond count)"
    );

    // Phase (b): a version within the count cap but beyond the age window
    // is KEPT — count protects it from the age cap.
    let mvcc2 = make_mvcc();
    mvcc2
        .set_retention(Retention {
            max_age_secs: Some(60),
            max_count: Some(3),
            min_count: None,
        })
        .unwrap();
    let key2 = Bytes::from("intersect_b");
    // v1 at frozen ts=1 (0 is the "real clock" sentinel).
    mvcc2.set_test_now(1);
    mvcc2
        .set_versioned(key2.clone(), Bytes::from("old"))
        .await
        .unwrap();
    // 100s later: overwrite. v1 (ts=1) is now 100s old (> 60s cap).
    mvcc2.set_test_now(100_001);
    mvcc2
        .set_versioned(key2.clone(), Bytes::from("new"))
        .await
        .unwrap();
    // C1: v2 (current, cur_v guard) + v1 (count-protected) = 2 entries.
    let hist_b = count_history_entries(&mvcc2).await;
    assert_eq!(
        hist_b, 2,
        "C1: phase (b): current + count-protected version = 2"
    );

    // Phase (c): a version beyond BOTH caps is reclaimed.
    let mvcc3 = make_mvcc();
    mvcc3
        .set_retention(Retention {
            max_age_secs: Some(60),
            max_count: Some(1),
            min_count: None,
        })
        .unwrap();
    let key3 = Bytes::from("intersect_c");
    // v1, v2 at ts=1 (old); v3 at ts=100_001.
    mvcc3.set_test_now(1);
    mvcc3
        .set_versioned(key3.clone(), Bytes::from("v1"))
        .await
        .unwrap();
    mvcc3
        .set_versioned(key3.clone(), Bytes::from("v2"))
        .await
        .unwrap();
    mvcc3.set_test_now(100_001);
    mvcc3
        .set_versioned(key3.clone(), Bytes::from("v3"))
        .await
        .unwrap();
    // After v3: entries desc = [v2(ts=1), v1(ts=1)].
    //   v2(idx0): within count (idx<1)? No. age: ts=1 < cutoff(40_001)? Yes.
    //     Both drop → reclaim.
    //   v1(idx1): beyond count. age drops. Both drop → reclaim.
    // Wait — both reclaimed? Then hist=0. But v2 is idx0 < max_count=1? 0 < 1 = yes!
    // Let me re-trace: max_count=1.
    //   v2(idx0): idx<1? YES → keep (within count window).
    //   v1(idx1): idx<1? No → count drops. age: ts=1<40001? Yes → age drops. → reclaim.
    // hist == 1 (v2 kept by count, v1 reclaimed by both).
    // C1: v3 (current, cur_v guard) = 1. v1, v2 both caps drop → reclaimed.
    let hist_c = count_history_entries(&mvcc3).await;
    assert_eq!(
        hist_c, 1,
        "C1: phase (c): current survives (cur_v guard); v1, v2 beyond both caps → reclaimed"
    );
}

/// 4. Under a live snapshot, an old-by-age version that is `>= min_alive`
/// or is the anchor is NOT reclaimed despite the age cap.
#[tokio::test]
async fn age_keeps_pinned_and_anchor() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    mvcc.set_retention(Retention {
        max_age_secs: Some(1),
        max_count: Some(0),
        min_count: None,
    })
    .unwrap();

    let key = Bytes::from("age_pinned_key");
    // Accumulate history under KeepHistory first (the aggressive policy
    // would reclaim on every write, leaving nothing to exercise the path).
    mvcc.set_retention(Retention::keep_history()).unwrap();
    mvcc.set_test_now(1);
    for i in 1..=5u32 {
        mvcc.set_versioned(key.clone(), Bytes::from(format!("v{i}")))
            .await
            .unwrap();
    }
    // 4 history entries (v1..v4), all ts=1.

    // Open a snapshot at v5 (last_committed) — pins min_alive=5.
    let snap = gate.open_snapshot().await;
    let snap_v = snap.version();
    assert_eq!(snap_v, 5);

    // Switch to the aggressive age+count policy and overwrite once.
    mvcc.set_retention(Retention {
        max_age_secs: Some(1),
        max_count: Some(0),
        min_count: None,
    })
    .unwrap();
    mvcc.set_test_now(10_000); // 10s later — v1..v4 all past the 1s cap.
    mvcc.set_versioned(key.clone(), Bytes::from("v6"))
        .await
        .unwrap();

    // C1: v6 (current, cur_v guard) + v5 (≥ min_alive) + v4 (anchor) = 3.
    // v3..v1: past age, beyond count, < min_alive, not anchor → reclaimed.
    let hist = count_history_entries(&mvcc).await;
    assert_eq!(
        hist, 3,
        "C1: age cap honors current + sacred floor + anchor: {{v6 current, v5 pinned, v4 anchor}}"
    );

    // The snapshot at v5 reads v5 (correct value at its version).
    let result = mvcc.get_at(&key, snap_v).await.unwrap();
    assert_eq!(
        result,
        Some(Bytes::from("v5")),
        "snapshot at v5 must read v5 (protected by min_alive despite age cap)"
    );
}

/// 5. A version with no recorded ts is conservatively KEPT by the age axis
/// (unknown age → do not reclaim by age).
#[tokio::test]
async fn unknown_ts_not_reclaimed_by_age() {
    use crate::mvcc_store::ts_key;

    let mvcc = make_mvcc();
    // Only an age cap (no count cap) — so the age axis is the ONLY potential
    // reclaimer. With no count cap, the only way to reclaim is age.
    mvcc.set_retention(Retention {
        max_age_secs: Some(1),
        max_count: None,
        min_count: None,
    })
    .unwrap();

    let key = Bytes::from("no_ts_key");
    // Write v1, v2 with a frozen clock so they get real ts entries.
    mvcc.set_test_now(1);
    mvcc.set_versioned(key.clone(), Bytes::from("v1"))
        .await
        .unwrap();
    let v1 = mvcc.version_of(&key);
    mvcc.set_versioned(key.clone(), Bytes::from("v2"))
        .await
        .unwrap();

    // Manually delete v1's ts entry — simulate a pre-T1c version with no ts.
    let _ = mvcc.history_store().remove(ts_key(v1).into()).await;

    // Advance the clock well past the age cap and write once more.
    mvcc.set_test_now(10_000);
    mvcc.set_versioned(key.clone(), Bytes::from("v3"))
        .await
        .unwrap();

    // C1: v3 (current, cur_v guard) + v1 (unknown ts, kept) = 2.
    // v2 (ts=1, age 10s > 1s) → reclaimed by age.
    let hist = count_history_entries(&mvcc).await;
    assert_eq!(
        hist, 2,
        "C1: unknown-ts v1 kept + current v3 = 2; v2 reclaimed by age"
    );
    // v1 is still reachable (its version entry survived).
    assert!(
        mvcc.get_at(&key, v1).await.unwrap().is_some(),
        "unknown-ts v1 must survive (conservatively kept by age axis)"
    );
}

/// 6. After writing version V, `history.get(ts_key(V))` returns the frozen
/// now. Confirms ts is recorded per write and is decodable.
#[tokio::test]
async fn ts_recorded_on_write() {
    use crate::mvcc_store::{decode_ts_key, ts_key};
    use crate::version_codec::encode_version_key;

    let mvcc = make_mvcc();
    mvcc.set_test_now(4242);

    let key = Bytes::from("ts_record_key");
    mvcc.set_versioned(key.clone(), Bytes::from("v1"))
        .await
        .unwrap();
    let v1 = mvcc.version_of(&key);

    // The ts-key for v1 holds the frozen now (4242), little-endian u64.
    let ts_val = mvcc.history_store().get(ts_key(v1).into()).await.unwrap();
    assert_eq!(ts_val.len(), 8, "ts value must be 8 bytes (u64 LE)");
    let ms = u64::from_le_bytes(ts_val.as_ref().try_into().unwrap());
    assert_eq!(ms, 4242, "recorded ts must equal the frozen now");

    // decode_ts_key round-trips for the same version.
    assert_eq!(decode_ts_key(&ts_key(v1)), Some(v1));
    // And a version-key is NOT mistaken for a ts-key.
    let vk = encode_version_key(&key, v1);
    assert!(
        decode_ts_key(&vk).is_none(),
        "version-key must not decode as ts-key"
    );

    mvcc.set_test_now(0); // restore real clock (hygiene)
}
