use super::helpers::{count_history_entries, make_gate, make_mvcc, make_mvcc_with_gate};
use bytes::Bytes;

#[tokio::test]
async fn gc_below_removes_old_versions_keeps_anchor() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    let _guard = gate.open_snapshot().await;

    let key = Bytes::from("gc_test");
    // Write 5 versions
    for i in 1..=5u32 {
        mvcc.set_versioned(key.clone(), Bytes::from(format!("v{i}")))
            .await
            .unwrap();
    }

    // C1: history has 5 entries (v1..v4 from when they were current,
    // plus v5 which IS the current version written into the log).
    let count_before = count_history_entries(&mvcc).await;
    assert_eq!(count_before, 5, "C1: 5 entries (4 old + current v5)");

    // GC below version 3: versions < 3 in history are v1 and v2.
    // Anchor = highest < 3 = v2, older one (v1) deleted.
    let deleted = mvcc.gc_below(3).await.unwrap();
    assert!(deleted >= 1, "should delete at least 1 old entry");

    let count_after = count_history_entries(&mvcc).await;
    assert!(count_after < count_before, "history should shrink");
}

#[tokio::test]
async fn gc_below_zero_deletes_nothing() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    let _guard = gate.open_snapshot().await;

    let key = Bytes::from("gc_noop");
    mvcc.set_versioned(key.clone(), Bytes::from("v1"))
        .await
        .unwrap();
    mvcc.set_versioned(key.clone(), Bytes::from("v2"))
        .await
        .unwrap();

    let deleted = mvcc.gc_below(0).await.unwrap();
    assert_eq!(deleted, 0);
}

#[tokio::test]
async fn gc_convenience_uses_min_alive() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    // No snapshots open → min_alive = last_committed = 0
    // Write some data without snapshot (no history archived)
    mvcc.set_versioned(Bytes::from("k"), Bytes::from("v"))
        .await
        .unwrap();

    let deleted = mvcc.gc().await.unwrap();
    assert_eq!(deleted, 0, "nothing to GC when no history");
}

// ----------------------------------------------------------------
// III.3 — GC prunes stale version_cache entries.
// ----------------------------------------------------------------

/// Core eviction: keys whose cached version is `< min_alive` are dropped
/// (so `version_of` returns 0) while the current entry in the log — and
/// therefore `get_at` — stays correct. A key at/above `min_alive` is
/// NOT evicted.
#[tokio::test]
async fn gc_evicts_stale_version_cache_entries() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    // Keep one snapshot open the whole time so writes go through the
    // versioned (history-archiving) path and populate version_cache.
    let _guard = gate.open_snapshot().await;

    // Two "old" keys, written early (low versions).
    let old_a = Bytes::from("old_a");
    let old_b = Bytes::from("old_b");
    mvcc.set_versioned(old_a.clone(), Bytes::from("a_val"))
        .await
        .unwrap();
    mvcc.set_versioned(old_b.clone(), Bytes::from("b_val"))
        .await
        .unwrap();
    let v_old_a = mvcc.version_of(&old_a);
    let v_old_b = mvcc.version_of(&old_b);
    assert!(v_old_a > 0 && v_old_b > 0);

    // A "fresh" key written at a high version.
    let fresh = Bytes::from("fresh");
    // Advance the version counter so `fresh` lands well above the olds.
    for _ in 0..10 {
        gate.assign_next_version();
    }
    mvcc.set_versioned(fresh.clone(), Bytes::from("f_val"))
        .await
        .unwrap();
    let v_fresh = mvcc.version_of(&fresh);
    assert!(v_fresh > v_old_a && v_fresh > v_old_b);

    // Cache currently holds all three.
    assert_eq!(mvcc.cells.len(), 3);

    // Advance min_alive to sit strictly above the old keys but at/below
    // the fresh key. With the snapshot still open at version 0, min_alive
    // would be 0 — so drop it first, then publish a committed marker so
    // min_alive == last_committed.
    let min_alive_target = v_old_b + 1; // > both olds, <= v_fresh
    assert!(min_alive_target <= v_fresh);
    drop(_guard); // no live snapshots → min_alive == last_committed
    gate.publish_committed(min_alive_target);
    assert_eq!(gate.min_alive(), min_alive_target);

    // GC. The history threshold here is irrelevant to cache pruning,
    // which always uses min_alive.
    mvcc.gc().await.unwrap();

    // Old keys (cv < min_alive) evicted → version_of == 0.
    assert_eq!(
        mvcc.version_of(&old_a),
        0,
        "stale old_a should be evicted from version_cache"
    );
    assert_eq!(
        mvcc.version_of(&old_b),
        0,
        "stale old_b should be evicted from version_cache"
    );
    // Fresh key (cv >= min_alive) retained.
    assert_eq!(
        mvcc.version_of(&fresh),
        v_fresh,
        "fresh key (cv >= min_alive) must NOT be evicted"
    );
    assert_eq!(mvcc.cells.len(), 1, "cache should have shrunk to 1");

    // Current values remain readable from the log after eviction. With the
    // cache entries gone, get_at uses the direct-read path (cur_v=0 ≤ snap).
    let snap = min_alive_target + 100;
    assert_eq!(
        mvcc.get_at(old_a.as_ref(), snap).await.unwrap(),
        Some(Bytes::from("a_val")),
        "evicting the cache entry must not change the current value"
    );
    assert_eq!(
        mvcc.get_at(old_b.as_ref(), snap).await.unwrap(),
        Some(Bytes::from("b_val"))
    );
    assert_eq!(
        mvcc.get_at(fresh.as_ref(), snap).await.unwrap(),
        Some(Bytes::from("f_val"))
    );
}

/// Boundary: a key whose cached version equals `min_alive` is retained
/// (the rule is strict `<`, not `<=`).
#[tokio::test]
async fn gc_keeps_version_cache_entry_at_min_alive_boundary() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    let guard = gate.open_snapshot().await;

    let key = Bytes::from("boundary");
    mvcc.set_versioned(key.clone(), Bytes::from("v"))
        .await
        .unwrap();
    let cv = mvcc.version_of(&key);
    assert!(cv > 0);

    // Set min_alive EXACTLY to cv: entry must survive (cv >= min_alive).
    drop(guard);
    gate.publish_committed(cv);
    assert_eq!(gate.min_alive(), cv);

    mvcc.gc().await.unwrap();

    assert_eq!(
        mvcc.version_of(&key),
        cv,
        "entry at the min_alive boundary must be retained (strict < eviction)"
    );
}

/// The dangerous case the invariant protects: an entry is NOT evicted
/// while a live snapshot below its version still needs the history value.
/// We open a snapshot between v1 and v2, advance last_committed, run GC,
/// and confirm (a) the entry survives (a live snapshot is older than its
/// version), and (b) `get_at` at that old snapshot STILL returns the
/// archived v1 from history — the MVCC visibility contract holds.
#[tokio::test]
async fn gc_preserves_visibility_for_live_snapshot_below_cached_version() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    // Bootstrap snapshot at v0 — activates the versioned write-path for
    // the v1 write (so version_cache gets populated). Dropped once the
    // real reader is open, so it does not drag min_alive below v1.
    let bootstrap = gate.open_snapshot().await;

    let key = Bytes::from("vis");

    // v1 committed.
    mvcc.set_versioned(key.clone(), Bytes::from("v1"))
        .await
        .unwrap();
    let v1 = mvcc.version_of(&key);

    // Publish v1 so a snapshot can open AT v1, then open it: this snapshot
    // is the live reader that still needs v1 after v2 lands.
    gate.publish_committed(v1);
    let reader_snap = gate.open_snapshot().await;
    assert_eq!(reader_snap.version(), v1);
    // Now drop the v0 bootstrap so the reader at v1 is the oldest snapshot.
    drop(bootstrap);

    // v2 overwrites; v1 is archived to history (the reader snapshot at v1
    // is still active).
    mvcc.set_versioned(key.clone(), Bytes::from("v2"))
        .await
        .unwrap();
    let v2 = mvcc.version_of(&key);
    assert!(v2 > v1);

    // Advance last_committed past v2. But the live reader snapshot pins
    // min_alive down to v1, so the cache entry (cv == v2) is NOT < v1
    // and must NOT be evicted.
    gate.publish_committed(v2 + 50);
    assert_eq!(
        gate.min_alive(),
        v1,
        "live reader snapshot must pin min_alive to v1"
    );

    mvcc.gc().await.unwrap();

    // Entry retained (cv == v2 >= min_alive == v1).
    assert_eq!(
        mvcc.version_of(&key),
        v2,
        "entry needed by a live snapshot below its version must survive GC"
    );

    // Visibility contract: the live snapshot at v1 still sees v1 (slow
    // path, because cur_v (v2) > snapshot (v1) → scan history).
    assert_eq!(
        mvcc.get_at(key.as_ref(), reader_snap.version())
            .await
            .unwrap(),
        Some(Bytes::from("v1")),
        "snapshot below the cached version must still read the archived v1"
    );

    // And a snapshot at/after v2 sees v2 (direct-read path from log).
    assert_eq!(
        mvcc.get_at(key.as_ref(), v2).await.unwrap(),
        Some(Bytes::from("v2"))
    );
}

/// Even after an entry is legitimately evicted, a (hypothetical) read at
/// an OLD snapshot below the real version is still answered correctly,
/// BECAUSE eviction only ever happens once no such live snapshot exists.
/// This test exercises the "evict, then read at the boundary snapshot"
/// path to prove the post-eviction read returns the correct log entry
/// (the post-eviction contract) rather than a stale/incorrect one.
#[tokio::test]
async fn gc_evicted_key_read_at_boundary_snapshot_returns_main() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    let guard = gate.open_snapshot().await;

    let key = Bytes::from("evict_then_read");
    // Two versions: v1 (prior entry) and v2 (current entry), both in the log.
    mvcc.set_versioned(key.clone(), Bytes::from("v1"))
        .await
        .unwrap();
    mvcc.set_versioned(key.clone(), Bytes::from("v2"))
        .await
        .unwrap();
    let v2 = mvcc.version_of(&key);

    // Move min_alive strictly above v2 (no live snapshots), making the
    // entry redundant: every surviving/ future snapshot is >= min_alive
    // > v2, so none can observe v1.
    drop(guard);
    gate.publish_committed(v2 + 1);
    assert_eq!(gate.min_alive(), v2 + 1);

    mvcc.gc().await.unwrap();
    assert_eq!(mvcc.version_of(&key), 0, "redundant entry evicted");

    // A read at exactly min_alive (the lowest a fresh snapshot could be):
    // cur_v = 0 (cell evicted) → log range-scan → finds v2
    // (the anchor, still in the log). The log is the sole source of truth;
    // v2 survives as the anchor.
    assert_eq!(
        mvcc.get_at(key.as_ref(), v2 + 1).await.unwrap(),
        Some(Bytes::from("v2")),
        "post-eviction read returns v2 from the log (anchor kept by gc)"
    );
}

// ================================================================
// C1 — vacuum-guard tests.
// ================================================================

/// C1 guarantee: vacuum never reclaims the current version.
#[tokio::test]
async fn c1_vacuum_never_reclaims_current() {
    use crate::version_codec::encode_version_key;

    let mvcc = make_mvcc(); // CurrentOnly — max_count=0

    let key = Bytes::from("c1_sacred");
    let mut latest_val = Bytes::new();
    for i in 1..=5u32 {
        latest_val = Bytes::from(format!("v{i}"));
        mvcc.set_versioned(key.clone(), latest_val.clone())
            .await
            .unwrap();
    }

    // After 5 writes with eager vacuum, only the current version survives.
    let cur_v = mvcc.version_of(&key);
    let log_val = mvcc
        .history_store()
        .get(encode_version_key(&key, cur_v))
        .await
        .unwrap();
    assert_eq!(
        log_val, latest_val,
        "C1: current version must survive vacuum"
    );

    // No other versions survive.
    let hist = count_history_entries(&mvcc).await;
    assert_eq!(hist, 1, "C1: only the current version survives vacuum");
}

/// C1 guarantee: the tombstone (current after delete) survives vacuum.
#[tokio::test]
async fn c1_vacuum_keeps_tombstone_current() {
    use crate::version_codec::encode_version_key;

    let mvcc = make_mvcc(); // CurrentOnly

    let key = Bytes::from("c1_tomb");
    mvcc.set_versioned(key.clone(), Bytes::from("val"))
        .await
        .unwrap();
    let del_v = mvcc.delete_versioned(key.clone()).await.unwrap();

    // The tombstone is the current version for this key — it must survive.
    let tombstone = mvcc
        .history_store()
        .get(encode_version_key(&key, del_v))
        .await
        .unwrap();
    assert_eq!(
        tombstone,
        Bytes::new(),
        "C1: delete tombstone must survive vacuum"
    );
}
