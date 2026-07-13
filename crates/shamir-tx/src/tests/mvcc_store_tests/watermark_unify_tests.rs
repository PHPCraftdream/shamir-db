//! P0b regression — non-tx writes unify on the `CompletionTracker`.
//!
//! Before P0b, the non-tx write path (`set_versioned` / `set_versioned_many`
//! / `delete_versioned`) bumped `last_committed_version` directly via
//! `publish_committed_max(new_v)` and NEVER touched the `CompletionTracker`.
//! Under that scheme each non-tx version is a permanent `Pending` hole in the
//! tracker, so `completion().watermark()` wedges at `new_v - 1` forever — any
//! tx that later commits a version ABOVE the non-tx write can never become
//! visible through the watermark gate (H5, the D2 BLOCKER).
//!
//! These tests pin the invariant that fixes H5: a non-tx write advances the
//! tracker watermark in lockstep with `last_committed`. On bare P0a (non-tx
//! not marking the tracker) the `watermark()` assertions below would fail
//! because the watermark would stay at the seed (0).

use super::helpers::{make_gate, make_mvcc_with_gate};
use bytes::Bytes;
use shamir_storage::types::RecordKey;

/// A single non-tx `set_versioned` advances the tracker watermark to the
/// assigned version — not just the `last_committed` atomic.
///
/// On P0a-alone this fails: `watermark()` stays 0 while `last_committed()`
/// moves to `new_v`.
#[tokio::test]
async fn nontx_set_advances_completion_watermark() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let new_v = mvcc
        .set_versioned(RecordKey::from(Bytes::from("k")), Bytes::from("v"))
        .await
        .unwrap();

    // last_committed moved (true on both P0a and P0b)...
    assert_eq!(gate.last_committed(), new_v);
    // ...AND the tracker watermark moved with it (the P0b fix; P0a-alone
    // leaves a Pending hole here so this is `new_v - 1`).
    assert_eq!(
        gate.completion().watermark(),
        new_v,
        "non-tx set must mark the CompletionTracker, not just last_committed",
    );
    assert_eq!(gate.completion().watermark(), gate.last_committed());
}

/// Two sequential non-tx writes keep watermark == last_committed and both
/// remain visible. Proves no Pending hole is left between the two writes.
#[tokio::test]
async fn two_nontx_writes_no_watermark_hole() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let v_a = mvcc
        .set_versioned(RecordKey::from(Bytes::from("a")), Bytes::from("va"))
        .await
        .unwrap();
    let v_b = mvcc
        .set_versioned(RecordKey::from(Bytes::from("b")), Bytes::from("vb"))
        .await
        .unwrap();

    assert!(v_b > v_a);
    // After two non-tx writes the contiguous watermark reached the latest
    // version with no gap — equal to last_committed.
    assert_eq!(gate.completion().watermark(), v_b);
    assert_eq!(gate.completion().watermark(), gate.last_committed());

    // Both writes are visible at the current floor.
    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from("a")))
            .await
            .unwrap(),
        Some(Bytes::from("va")),
    );
    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from("b")))
            .await
            .unwrap(),
        Some(Bytes::from("vb")),
    );
}

/// H5 scenario: a non-tx write at V_n is followed by a marked version V_t > V_n.
/// We model the second marked path with another non-tx write (which, post-P0b,
/// is itself a tracker-marked path). The watermark / last_committed must reach
/// V_t. On P0a-alone the V_n write left a permanent Pending hole → the
/// watermark would be stuck at V_n - 1 and never reach V_t.
#[tokio::test]
async fn marked_version_above_nontx_becomes_visible() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    // Non-tx write of key A → V_n.
    let v_n = mvcc
        .set_versioned(RecordKey::from(Bytes::from("A")), Bytes::from("a"))
        .await
        .unwrap();
    assert_eq!(gate.last_committed(), v_n);

    // A subsequent marked version V_t > V_n (here: a second non-tx write,
    // which post-P0b goes through the same CompletionTracker).
    let v_t = mvcc
        .set_versioned(RecordKey::from(Bytes::from("B")), Bytes::from("b"))
        .await
        .unwrap();
    assert!(v_t > v_n);

    // The watermark — the D2 visibility gate — advanced PAST the earlier
    // non-tx version all the way to V_t. Without P0b it would be wedged at
    // V_n - 1 because the V_n write never marked the tracker.
    assert!(
        gate.completion().watermark() >= v_t,
        "watermark must clear the earlier non-tx version and reach V_t",
    );
    assert!(gate.last_committed() >= v_t);
}

/// Batch `set_versioned_many`: every version in the batch is marked, so the
/// watermark reaches the batch max with no holes. On P0a-alone every batch
/// version is an un-marked Pending hole.
#[tokio::test]
async fn nontx_batch_advances_watermark_to_max() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    let items = vec![
        (Bytes::from("k1"), Bytes::from("v1")),
        (Bytes::from("k2"), Bytes::from("v2")),
        (Bytes::from("k3"), Bytes::from("v3")),
    ];
    let max_v = mvcc
        .set_versioned_many(
            items
                .into_iter()
                .map(|(k, v)| (RecordKey::from(k), v))
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();

    assert_eq!(gate.completion().watermark(), max_v);
    assert_eq!(gate.completion().watermark(), gate.last_committed());
}

/// `delete_versioned` is the third non-tx site; it too must mark the tracker.
#[tokio::test]
async fn nontx_delete_advances_completion_watermark() {
    let gate = make_gate();
    let mvcc = make_mvcc_with_gate(gate.clone());

    mvcc.set_versioned(RecordKey::from(Bytes::from("k")), Bytes::from("v"))
        .await
        .unwrap();
    let del_v = mvcc
        .delete_versioned(RecordKey::from(Bytes::from("k")))
        .await
        .unwrap();

    assert_eq!(gate.completion().watermark(), del_v);
    assert_eq!(gate.completion().watermark(), gate.last_committed());
}
