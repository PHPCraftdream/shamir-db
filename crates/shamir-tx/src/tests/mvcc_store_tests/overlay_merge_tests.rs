//! P1b — overlay-aware read seams (`resolve_read`/`get_at`, `get_current`,
//! `current_stream`).
//!
//! In production the overlay is NOT populated in P1b (filled in P1c), so every
//! real read falls through to history and behaviour is byte-identical. These
//! tests instead hand-place entries into `store.overlay()` to prove the merge /
//! probe logic is already correct: overlay wins when newer, history wins when
//! newer, tombstones suppress, overlay-only keys surface, and — critically —
//! an EMPTY overlay reproduces history-only output bit-for-bit (regression
//! guard for the probe overhead).

use bytes::Bytes;
use futures::StreamExt;

use super::helpers::make_mvcc_with_gate;
use crate::repo_tx_gate::RepoTxGate;
use crate::version_codec::encode_version_key;
use shamir_storage::types::RecordKey;
use std::sync::Arc;

fn make_gate() -> Arc<RepoTxGate> {
    Arc::new(RepoTxGate::fresh())
}

/// Write a raw version-keyed entry straight into the durable history log,
/// bypassing the version-allocation path (so the test fully controls the
/// (version, value) pair without disturbing the gate counter).
async fn put_history(mvcc: &crate::mvcc_store::MvccStore, key: &[u8], version: u64, val: Bytes) {
    mvcc.history_store()
        .set(encode_version_key(key, version).into(), val)
        .await
        .unwrap();
}

async fn collect_stream(mvcc: &crate::mvcc_store::MvccStore, batch: usize) -> Vec<(Bytes, Bytes)> {
    let mut out = Vec::new();
    let stream = mvcc.current_stream(batch);
    futures::pin_mut!(stream);
    while let Some(b) = stream.next().await {
        out.extend(b.unwrap());
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

// ============================================================================
// resolve_read / get_at — overlay precedence
// ============================================================================

/// Overlay holds a NEWER version than history → the overlay value is visible.
#[tokio::test]
async fn get_at_overlay_newer_than_history_wins() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    // Durable history at v3; overlay at v7. Floor high enough to see both.
    put_history(&mvcc, b"k", 3, Bytes::from_static(b"hist-v3")).await;
    mvcc.overlay().insert(
        RecordKey::from(Bytes::from_static(b"k")),
        7,
        Bytes::from_static(b"ov-v7"),
    );
    gate.publish_committed_max(10);

    // snapshot 10, fallback path (cell absent → cur_v == 0).
    assert_eq!(
        mvcc.get_at(b"k", 10).await.unwrap(),
        Some(Bytes::from_static(b"ov-v7")),
    );
}

/// Overlay tombstone NEWER than a history write → key reads as deleted (None).
#[tokio::test]
async fn get_at_overlay_tombstone_newer_suppresses() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    put_history(&mvcc, b"k", 3, Bytes::from_static(b"hist-v3")).await;
    // newer overlay version is a tombstone (empty).
    mvcc.overlay()
        .insert(RecordKey::from(Bytes::from_static(b"k")), 7, Bytes::new());
    gate.publish_committed_max(10);

    assert_eq!(mvcc.get_at(b"k", 10).await.unwrap(), None);
}

/// Overlay version is OLDER than the live history version (and above-snapshot
/// overlay entries are cut off) → history wins.
#[tokio::test]
async fn get_at_overlay_older_than_history_history_wins() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    put_history(&mvcc, b"k", 8, Bytes::from_static(b"hist-v8")).await;
    mvcc.overlay().insert(
        RecordKey::from(Bytes::from_static(b"k")),
        4,
        Bytes::from_static(b"ov-v4"),
    );
    gate.publish_committed_max(10);

    assert_eq!(
        mvcc.get_at(b"k", 10).await.unwrap(),
        Some(Bytes::from_static(b"hist-v8")),
    );
}

/// Direct path (cell version cached, ≤ snapshot): overlay.get hits the exact
/// (key, cur_v) and supersedes history at that version.
#[tokio::test]
async fn get_at_direct_path_overlay_hit() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    // history at v5 with one value; overlay at the SAME v5 with another. The
    // cell points at v5. Overlay probe must win on the direct path.
    put_history(&mvcc, b"k", 5, Bytes::from_static(b"hist-v5")).await;
    mvcc.overlay().insert(
        RecordKey::from(Bytes::from_static(b"k")),
        5,
        Bytes::from_static(b"ov-v5"),
    );
    mvcc.seed_version(RecordKey::from(Bytes::from_static(b"k")), 5)
        .await;
    gate.publish_committed_max(10);

    assert_eq!(
        mvcc.get_at(b"k", 5).await.unwrap(),
        Some(Bytes::from_static(b"ov-v5")),
    );
}

/// Empty-overlay invariant: get_at is identical to history-only resolution.
#[tokio::test]
async fn get_at_empty_overlay_identical_to_history() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    put_history(&mvcc, b"k", 2, Bytes::from_static(b"a")).await;
    put_history(&mvcc, b"k", 6, Bytes::from_static(b"b")).await;
    gate.publish_committed_max(10);
    assert!(mvcc.overlay().is_empty());

    // newest ≤ 4 is v2; newest ≤ 10 is v6.
    assert_eq!(
        mvcc.get_at(b"k", 4).await.unwrap(),
        Some(Bytes::from_static(b"a"))
    );
    assert_eq!(
        mvcc.get_at(b"k", 10).await.unwrap(),
        Some(Bytes::from_static(b"b"))
    );
    assert_eq!(mvcc.get_at(b"missing", 10).await.unwrap(), None);
}

// ============================================================================
// get_current — overlay precedence
// ============================================================================

/// get_current with cached cell: overlay newer than history at the same cell
/// version → overlay value.
#[tokio::test]
async fn get_current_overlay_supersedes_history() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    put_history(&mvcc, b"k", 5, Bytes::from_static(b"hist-v5")).await;
    mvcc.overlay().insert(
        RecordKey::from(Bytes::from_static(b"k")),
        5,
        Bytes::from_static(b"ov-v5"),
    );
    mvcc.seed_version(RecordKey::from(Bytes::from_static(b"k")), 5)
        .await;
    gate.publish_committed_max(5);

    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from_static(b"k")))
            .await
            .unwrap(),
        Some(Bytes::from_static(b"ov-v5")),
    );
}

/// Overlay-only key (NOT in history, cell absent) is found by get_current via
/// the cold-start overlay probe.
#[tokio::test]
async fn get_current_overlay_only_key_found() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    // No history entry at all; key lives only in the overlay.
    mvcc.overlay().insert(
        RecordKey::from(Bytes::from_static(b"ghost")),
        4,
        Bytes::from_static(b"ov-only"),
    );
    gate.publish_committed_max(10);

    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from_static(b"ghost")))
            .await
            .unwrap(),
        Some(Bytes::from_static(b"ov-only")),
    );
}

/// Overlay-only tombstone → get_current returns None (deleted, not durable).
#[tokio::test]
async fn get_current_overlay_only_tombstone_none() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    mvcc.overlay().insert(
        RecordKey::from(Bytes::from_static(b"ghost")),
        4,
        Bytes::new(),
    );
    gate.publish_committed_max(10);

    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from_static(b"ghost")))
            .await
            .unwrap(),
        None,
    );
}

/// Empty-overlay invariant: get_current matches history-only behaviour.
#[tokio::test]
async fn get_current_empty_overlay_identical_to_history() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    put_history(&mvcc, b"k", 5, Bytes::from_static(b"hist-v5")).await;
    mvcc.seed_version(RecordKey::from(Bytes::from_static(b"k")), 5)
        .await;
    gate.publish_committed_max(5);
    assert!(mvcc.overlay().is_empty());

    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from_static(b"k")))
            .await
            .unwrap(),
        Some(Bytes::from_static(b"hist-v5")),
    );
    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from_static(b"absent")))
            .await
            .unwrap(),
        None,
    );
}

// ============================================================================
// current_stream — merge-join
// ============================================================================

/// (a) Overlay overrides a history key with a newer version.
#[tokio::test]
async fn current_stream_overlay_overrides_history_key() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    put_history(&mvcc, b"a", 2, Bytes::from_static(b"a-hist")).await;
    put_history(&mvcc, b"b", 3, Bytes::from_static(b"b-hist")).await;
    // overlay holds a newer version of `a`.
    mvcc.overlay().insert(
        RecordKey::from(Bytes::from_static(b"a")),
        8,
        Bytes::from_static(b"a-ov"),
    );
    gate.publish_committed_max(10);

    let got = collect_stream(&mvcc, 16).await;
    assert_eq!(
        got,
        vec![
            (Bytes::from_static(b"a"), Bytes::from_static(b"a-ov")),
            (Bytes::from_static(b"b"), Bytes::from_static(b"b-hist")),
        ]
    );
}

/// (b) Overlay-only key is appended after the history stream drains.
#[tokio::test]
async fn current_stream_overlay_only_key_emitted() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    put_history(&mvcc, b"a", 2, Bytes::from_static(b"a-hist")).await;
    mvcc.overlay().insert(
        RecordKey::from(Bytes::from_static(b"z")),
        5,
        Bytes::from_static(b"z-ov"),
    );
    gate.publish_committed_max(10);

    let got = collect_stream(&mvcc, 16).await;
    assert_eq!(
        got,
        vec![
            (Bytes::from_static(b"a"), Bytes::from_static(b"a-hist")),
            (Bytes::from_static(b"z"), Bytes::from_static(b"z-ov")),
        ]
    );
}

/// (c) Overlay tombstone (newer) suppresses a history key.
#[tokio::test]
async fn current_stream_overlay_tombstone_suppresses_history_key() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    put_history(&mvcc, b"a", 2, Bytes::from_static(b"a-hist")).await;
    put_history(&mvcc, b"b", 3, Bytes::from_static(b"b-hist")).await;
    // newer overlay tombstone for `a`.
    mvcc.overlay()
        .insert(RecordKey::from(Bytes::from_static(b"a")), 8, Bytes::new());
    gate.publish_committed_max(10);

    let got = collect_stream(&mvcc, 16).await;
    assert_eq!(
        got,
        vec![(Bytes::from_static(b"b"), Bytes::from_static(b"b-hist"))]
    );
}

/// Overlay-only tombstone is NOT emitted (suppressed in the drain phase).
#[tokio::test]
async fn current_stream_overlay_only_tombstone_not_emitted() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    put_history(&mvcc, b"a", 2, Bytes::from_static(b"a-hist")).await;
    mvcc.overlay()
        .insert(RecordKey::from(Bytes::from_static(b"z")), 5, Bytes::new());
    gate.publish_committed_max(10);

    let got = collect_stream(&mvcc, 16).await;
    assert_eq!(
        got,
        vec![(Bytes::from_static(b"a"), Bytes::from_static(b"a-hist"))]
    );
}

/// History newer than the overlay for the same key → history wins in the merge.
#[tokio::test]
async fn current_stream_history_newer_than_overlay_wins() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    put_history(&mvcc, b"a", 9, Bytes::from_static(b"a-hist-v9")).await;
    mvcc.overlay().insert(
        RecordKey::from(Bytes::from_static(b"a")),
        4,
        Bytes::from_static(b"a-ov-v4"),
    );
    gate.publish_committed_max(10);

    let got = collect_stream(&mvcc, 16).await;
    assert_eq!(
        got,
        vec![(Bytes::from_static(b"a"), Bytes::from_static(b"a-hist-v9"))]
    );
}

/// (d) Empty overlay → stream output is identical to history-only. Regression
/// guard covering batch boundaries (small batch forces multiple drains).
#[tokio::test]
async fn current_stream_empty_overlay_identical_to_history() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    // Several keys, one with multiple versions, one tombstoned at its latest.
    put_history(&mvcc, b"a", 1, Bytes::from_static(b"a-old")).await;
    put_history(&mvcc, b"a", 4, Bytes::from_static(b"a-new")).await;
    put_history(&mvcc, b"b", 2, Bytes::from_static(b"b-val")).await;
    put_history(&mvcc, b"c", 3, Bytes::from_static(b"c-val")).await;
    put_history(&mvcc, b"d", 5, Bytes::new()).await; // tombstone latest → dropped
    gate.publish_committed_max(10);
    assert!(mvcc.overlay().is_empty());

    // batch=1 forces the multi-pull drain path.
    let got = collect_stream(&mvcc, 1).await;
    assert_eq!(
        got,
        vec![
            (Bytes::from_static(b"a"), Bytes::from_static(b"a-new")),
            (Bytes::from_static(b"b"), Bytes::from_static(b"b-val")),
            (Bytes::from_static(b"c"), Bytes::from_static(b"c-val")),
        ]
    );
}

/// Overlay entries ABOVE the floor are excluded from the stream (R3 visibility
/// cap honoured by `snapshot_le(floor)`).
#[tokio::test]
async fn current_stream_overlay_above_floor_excluded() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());
    put_history(&mvcc, b"a", 2, Bytes::from_static(b"a-hist")).await;
    // overlay version 20 is ABOVE the floor of 5 → invisible.
    mvcc.overlay().insert(
        RecordKey::from(Bytes::from_static(b"a")),
        20,
        Bytes::from_static(b"a-future"),
    );
    gate.publish_committed_max(5);

    let got = collect_stream(&mvcc, 16).await;
    assert_eq!(
        got,
        vec![(Bytes::from_static(b"a"), Bytes::from_static(b"a-hist"))]
    );
}
