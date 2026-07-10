use super::helpers::{archived_count, make_gate, make_mvcc, make_mvcc_with_gate};
use crate::mvcc_store::Retention;
use bytes::Bytes;

// ========================================================================
// T4-history — history_of
// ========================================================================

#[tokio::test]
async fn history_of_returns_empty_for_unknown_key() {
    let mvcc = make_mvcc();
    let timeline = mvcc.history_of(b"absent").await.unwrap();
    assert!(timeline.is_empty(), "an unknown key has no timeline");
}

/// A key written three times must yield three timeline entries
/// (v1, v2, v3 — all from the single `history` log), ascending by
/// version, each carrying its value and its recorded commit timestamp.
#[tokio::test]
async fn history_of_three_writes_full_timeline_with_ts() {
    let mvcc = make_mvcc();
    // Default retention is CurrentOnly (max_count = 0) — vacuum
    // reclaims every archived version right after each write. To
    // observe a multi-version timeline we must opt into KeepHistory.
    mvcc.set_retention(Retention::keep_history()).unwrap();

    // Freeze the clock so each version gets a distinct, known ts.
    mvcc.set_test_now(1_000);
    mvcc.set_versioned(Bytes::from("k"), Bytes::from("v1"))
        .await
        .unwrap();
    mvcc.set_test_now(2_000);
    mvcc.set_versioned(Bytes::from("k"), Bytes::from("v2"))
        .await
        .unwrap();
    mvcc.set_test_now(3_000);
    mvcc.set_versioned(Bytes::from("k"), Bytes::from("v3"))
        .await
        .unwrap();

    let timeline = mvcc.history_of(b"k").await.unwrap();
    assert_eq!(
        timeline.len(),
        3,
        "three writes → three timeline entries (2 archived + 1 current)"
    );

    // Ascending by version.
    let versions: Vec<u64> = timeline.iter().map(|e| e.version).collect();
    assert_eq!(versions, vec![1, 2, 3]);
    assert!(
        versions.windows(2).all(|w| w[0] < w[1]),
        "timeline must be ascending by version"
    );

    // Values line up per version.
    let values: Vec<&[u8]> = timeline.iter().map(|e| e.value.as_ref()).collect();
    assert_eq!(values, vec![b"v1".as_slice(), b"v2", b"v3"]);

    // ts per version (T1c) — each matches the frozen clock at its write.
    let ts: Vec<Option<u64>> = timeline.iter().map(|e| e.ts_millis).collect();
    assert_eq!(ts, vec![Some(1_000), Some(2_000), Some(3_000)]);

    mvcc.set_test_now(0);
}

/// A deleted key contributes its prior versions plus the tombstone —
/// all from the log, in ascending version order.
#[tokio::test]
async fn history_of_deleted_key_keeps_prior_versions() {
    let mvcc = make_mvcc();
    mvcc.set_retention(Retention::keep_history()).unwrap();

    mvcc.set_versioned(Bytes::from("k"), Bytes::from("v1"))
        .await
        .unwrap();
    mvcc.set_versioned(Bytes::from("k"), Bytes::from("v2"))
        .await
        .unwrap();
    // C1: delete writes a tombstone (empty value) into the log.
    mvcc.delete_versioned(Bytes::from("k")).await.unwrap();

    let timeline = mvcc.history_of(b"k").await.unwrap();
    // v1 + v2 (prior log entries) + v3 (tombstone, also in the log).
    assert_eq!(timeline.len(), 3);
    let versions: Vec<u64> = timeline.iter().map(|e| e.version).collect();
    assert_eq!(versions, vec![1, 2, 3]);
    let values: Vec<&[u8]> = timeline.iter().map(|e| e.value.as_ref()).collect();
    assert_eq!(values, vec![b"v1".as_slice(), b"v2", b""]);
}

/// Two keys must not bleed into each other's timelines — a prefix
/// collision (`"k"` vs `"kk"`) must keep them separate.
#[tokio::test]
async fn history_of_isolates_prefix_collisions() {
    let mvcc = make_mvcc();
    mvcc.set_retention(Retention::keep_history()).unwrap();

    mvcc.set_versioned(Bytes::from("k"), Bytes::from("a"))
        .await
        .unwrap();
    mvcc.set_versioned(Bytes::from("kk"), Bytes::from("b"))
        .await
        .unwrap();

    let tl_k = mvcc.history_of(b"k").await.unwrap();
    let tl_kk = mvcc.history_of(b"kk").await.unwrap();

    assert_eq!(tl_k.len(), 1, "\"k\" has one entry");
    assert_eq!(tl_k[0].value, Bytes::from("a"));
    assert_eq!(tl_kk.len(), 1, "\"kk\" has one entry");
    assert_eq!(tl_kk[0].value, Bytes::from("b"));
}

// ========================================================================
// T4-purge — purge_below_ts
// ========================================================================

/// Core case: a key written three times (v1, v2 prior; v3 current).
/// Purging with a cutoff that falls BETWEEN v1's and v2's commit ts
/// reclaims v1 only — v2 (newer than cutoff) and v3 (current) survive.
#[tokio::test]
async fn purge_below_ts_reclaims_only_older_than_cutoff() {
    let mvcc = make_mvcc();
    mvcc.set_retention(Retention::keep_history()).unwrap();

    // v1 @ ts=1_000, v2 @ ts=2_000, v3 @ ts=3_000 (all same key).
    mvcc.set_test_now(1_000);
    mvcc.set_versioned(Bytes::from("k"), Bytes::from("v1"))
        .await
        .unwrap();
    mvcc.set_test_now(2_000);
    mvcc.set_versioned(Bytes::from("k"), Bytes::from("v2"))
        .await
        .unwrap();
    mvcc.set_test_now(3_000);
    mvcc.set_versioned(Bytes::from("k"), Bytes::from("v3"))
        .await
        .unwrap();

    // Before purge: two archived versions (v1, v2).
    assert_eq!(archived_count(&mvcc, b"k").await, 2);

    // Cutoff at 1_500: v1 (ts=1_000) is older, v2 (ts=2_000) is newer.
    let deleted = mvcc.purge_below_ts(1_500).await.unwrap();
    assert_eq!(deleted, 1, "only v1 is older than the cutoff");

    // v2 survives; v1 is gone.
    let timeline = mvcc.history_of(b"k").await.unwrap();
    let versions: Vec<u64> = timeline.iter().map(|e| e.version).collect();
    assert_eq!(versions, vec![2, 3], "v2 (prior) + v3 (current) remain");
    assert!(!versions.contains(&1), "v1 must be purged");

    mvcc.set_test_now(0);
}

/// Sacred floor: with a live snapshot pinning `min_alive`, purge
/// does NOT reclaim any snapshot-protected version — even one
/// whose ts is older than the cutoff.
#[tokio::test]
async fn purge_below_ts_respects_live_snapshot_floor() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    mvcc.set_retention(Retention::keep_history()).unwrap();

    // v1 @ ts=1_000.
    mvcc.set_test_now(1_000);
    mvcc.set_versioned(Bytes::from("k"), Bytes::from("v1"))
        .await
        .unwrap();
    // Open a snapshot NOW (at version 1) → min_alive = 1. Every
    // subsequent version is >= min_alive, hence sacred.
    let _guard = gate.open_snapshot().await;

    // v2 @ ts=2_000 archives v1 into history.
    mvcc.set_test_now(2_000);
    mvcc.set_versioned(Bytes::from("k"), Bytes::from("v2"))
        .await
        .unwrap();

    // Cutoff far in the future — would reclaim v1 by ts alone.
    let deleted = mvcc.purge_below_ts(u64::MAX / 2).await.unwrap();
    assert_eq!(
        deleted, 0,
        "live snapshot pins every version >= min_alive; nothing reclaimable"
    );

    // Both archived v1 and current v2 survive.
    let timeline = mvcc.history_of(b"k").await.unwrap();
    let versions: Vec<u64> = timeline.iter().map(|e| e.version).collect();
    assert_eq!(versions, vec![1, 2], "snapshot floor protects v1");

    mvcc.set_test_now(0);
}

/// Unknown-ts version: a version whose ts-key is missing is NEVER
/// purged (can't prove it's old enough).
#[tokio::test]
async fn purge_below_ts_keeps_unknown_ts_versions() {
    use crate::mvcc_store::ts_key;

    let mvcc = make_mvcc();
    mvcc.set_retention(Retention::keep_history()).unwrap();

    // v1, v2 (history archives v1; both get ts-keys via record_ts).
    mvcc.set_test_now(1_000);
    mvcc.set_versioned(Bytes::from("k"), Bytes::from("v1"))
        .await
        .unwrap();
    mvcc.set_test_now(2_000);
    mvcc.set_versioned(Bytes::from("k"), Bytes::from("v2"))
        .await
        .unwrap();

    // Surgically remove v1's ts-key so its age becomes unknown.
    let v1 = mvcc.version_of(b"k") - 1; // v1 is one below the current version
    let _ = mvcc.history_store().remove(ts_key(v1).into()).await;

    // Even an aggressive cutoff can't reclaim v1 — unknown age.
    let deleted = mvcc.purge_below_ts(u64::MAX / 2).await.unwrap();
    assert_eq!(
        deleted, 0,
        "unknown-ts version must be kept (can't prove it's old)"
    );

    mvcc.set_test_now(0);
}

/// Anchor protection: with two archived versions below min_alive,
/// the non-anchor (older) one is reclaimed but the anchor (newest
/// below min_alive) survives even though its ts is older than the
/// cutoff.
#[tokio::test]
async fn purge_below_ts_keeps_anchor_reclaims_older() {
    let mvcc = make_mvcc();
    mvcc.set_retention(Retention::keep_history()).unwrap();

    // v1 @ ts=1_000, v2 @ ts=2_000, v3 @ ts=3_000.
    // After three writes: log = {v1, v2, v3 (current)},
    // min_alive = last_committed = 3 (no snapshot).
    mvcc.set_test_now(1_000);
    mvcc.set_versioned(Bytes::from("k"), Bytes::from("v1"))
        .await
        .unwrap();
    mvcc.set_test_now(2_000);
    mvcc.set_versioned(Bytes::from("k"), Bytes::from("v2"))
        .await
        .unwrap();
    mvcc.set_test_now(3_000);
    mvcc.set_versioned(Bytes::from("k"), Bytes::from("v3"))
        .await
        .unwrap();

    // Cutoff far in the future: both v1 and v2 are older. v1 is
    // reclaimed, but v2 — the anchor (largest < min_alive=3) —
    // survives.
    let deleted = mvcc.purge_below_ts(u64::MAX / 2).await.unwrap();
    assert_eq!(deleted, 1, "v1 reclaimed, v2 kept as anchor");

    let timeline = mvcc.history_of(b"k").await.unwrap();
    let versions: Vec<u64> = timeline.iter().map(|e| e.version).collect();
    assert_eq!(versions, vec![2, 3], "anchor v2 + current v3 remain");

    mvcc.set_test_now(0);
}

/// An empty / future cutoff reclaims nothing.
#[tokio::test]
async fn purge_below_ts_zero_cutoff_reclaims_nothing() {
    let mvcc = make_mvcc();
    mvcc.set_retention(Retention::keep_history()).unwrap();

    mvcc.set_test_now(1_000);
    mvcc.set_versioned(Bytes::from("k"), Bytes::from("v1"))
        .await
        .unwrap();
    mvcc.set_test_now(2_000);
    mvcc.set_versioned(Bytes::from("k"), Bytes::from("v2"))
        .await
        .unwrap();

    // cutoff = 0: no version has ts < 0.
    let deleted = mvcc.purge_below_ts(0).await.unwrap();
    assert_eq!(deleted, 0);

    mvcc.set_test_now(0);
}
