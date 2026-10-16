//! S5c-X PR6b-1 — publication-bound MVCC history primitives.
//! (moved from `crates/shamir-tx/src/tests/publication_bound/` — write_receipt-scoped layout ratchet.)
//!
//! Pins the split between [`MvccStore::materialize_committed_to_history`]
//! (durable rows + ts, NO visibility) and [`MvccStore::publish_materialized`]
//! (cell seeds + floor raise, infallible), and the legacy wrapper
//! [`MvccStore::write_committed_to_history`] that keeps the pre-split
//! end-to-end ordering.

use bytes::Bytes;
use futures::StreamExt;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{KvOp, RecordKey, Store as _};
use std::sync::Arc;

use crate::mvcc_store::MvccStore;
use crate::repo_tx_gate::RepoTxGate;
use crate::version_codec::encode_version_key;

fn make_mvcc() -> (MvccStore, Arc<RepoTxGate>) {
    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = MvccStore::new(Arc::new(InMemoryStore::new()), Arc::clone(&gate));
    (mvcc, gate)
}

fn set_op(key: &[u8], val: &[u8]) -> KvOp {
    KvOp::Set(
        RecordKey::from(Bytes::copy_from_slice(key)),
        Bytes::copy_from_slice(val),
    )
}

/// Collect `current_stream` into a sorted key→value map.
async fn collect_current(mvcc: &MvccStore) -> Vec<(Vec<u8>, Bytes)> {
    let mut out = Vec::new();
    let stream = mvcc.current_stream(64);
    futures::pin_mut!(stream);
    while let Some(batch) = stream.next().await {
        for (k, v) in batch.unwrap() {
            out.push((k.to_vec(), v));
        }
    }
    out
}

/// A version materialized but NOT published must be invisible: the durable
/// history row exists, but `get_current` / `current_stream` cap at
/// `gate.last_committed()` (the published floor), so the row cannot be read
/// until `publish_materialized` raises the floor and seeds the cell.
#[tokio::test]
async fn materialized_without_publish_is_invisible() {
    let (mvcc, gate) = make_mvcc();
    // I3 fix: floor 0 no longer means "bootstrap, no restriction" — it means
    // nothing is published. Raise it so the non-zero-floor bound is exercised
    // alongside the zero-floor test below.
    gate.raise_published(1);

    let ops = [set_op(b"k", b"v2")];
    mvcc.materialize_committed_to_history(&ops, 2)
        .await
        .unwrap();

    // Cell absent, cold path capped at the floor via `get_at`.
    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from_static(b"k")))
            .await
            .unwrap(),
        None
    );
    assert_eq!(mvcc.get_current_bytes(b"k").await.unwrap(), None);

    let current = collect_current(&mvcc).await;
    assert!(
        !current.iter().any(|(k, _)| k.as_slice() == b"k"),
        "unpublished version must not appear in current_stream: {current:?}"
    );

    // Publish → visible.
    mvcc.publish_materialized(&ops, 2);
    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from_static(b"k")))
            .await
            .unwrap(),
        Some(Bytes::from_static(b"v2"))
    );
    let current = collect_current(&mvcc).await;
    assert!(
        current
            .iter()
            .any(|(k, v)| k.as_slice() == b"k" && v.as_ref() == b"v2"),
        "published version must appear in current_stream: {current:?}"
    );
}

/// The materialize half is durable: the version-key row and the commit-ts
/// entry are in history and the ts index is updated BEFORE any publish.
/// Negative control: reading history directly (without the publication
/// bound) exposes the row even while it is invisible to the MVCC read paths.
#[tokio::test]
async fn materialized_rows_and_ts_are_durable_before_publish() {
    let (mvcc, gate) = make_mvcc();
    gate.raise_published(1);

    let ops = [set_op(b"k", b"v2")];
    mvcc.materialize_committed_to_history(&ops, 2)
        .await
        .unwrap();

    // Row landed in history (raw read bypasses the visibility floor).
    assert_eq!(
        mvcc.history_store()
            .get(encode_version_key(b"k", 2).into())
            .await
            .unwrap(),
        Bytes::from_static(b"v2")
    );
    // Commit ts landed in the ts index (materialize updates it).
    assert_eq!(mvcc.version_at_or_before_ts(u64::MAX).await, Some(2));

    // Publishing must not disturb the durable state.
    mvcc.publish_materialized(&ops, 2);
    assert_eq!(
        mvcc.history_store()
            .get(encode_version_key(b"k", 2).into())
            .await
            .unwrap(),
        Bytes::from_static(b"v2")
    );
    assert_eq!(mvcc.version_at_or_before_ts(u64::MAX).await, Some(2));
}

/// A tombstone materializes as an empty history row: invisible before
/// publish, and STILL reported as absent by `get_current` after publish
/// (tombstones collapse to `None`), while the tombstone stream shows the key.
#[tokio::test]
async fn tombstone_materialize_roundtrip() {
    let (mvcc, gate) = make_mvcc();
    gate.raise_published(1);

    let ops = [KvOp::Remove(RecordKey::from(Bytes::from_static(b"k")))];
    mvcc.materialize_committed_to_history(&ops, 2)
        .await
        .unwrap();
    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from_static(b"k")))
            .await
            .unwrap(),
        None
    );

    mvcc.publish_materialized(&ops, 2);
    // Tombstone suppressed on the normal read path…
    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from_static(b"k")))
            .await
            .unwrap(),
        None
    );
    // …but visible on the tombstone stream, and the durable row is empty.
    let mut found = false;
    let stream = mvcc.current_stream_with_tombstones(64);
    futures::pin_mut!(stream);
    while let Some(batch) = stream.next().await {
        for (k, v) in batch.unwrap() {
            if k.as_ref() == b"k" {
                assert!(v.is_empty(), "tombstone row must be empty: {v:?}");
                found = true;
            }
        }
    }
    assert!(found, "tombstoned key must appear in the tombstone stream");
}

/// The legacy wrapper keeps the pre-split end-to-end behaviour: one call
/// both makes the rows durable AND publishes (cell seeded + floor raised),
/// so the value is visible immediately.
#[tokio::test]
async fn legacy_wrapper_visible_immediately() {
    let (mvcc, gate) = make_mvcc();
    gate.raise_published(2);

    let ops = [set_op(b"k", b"v3")];
    mvcc.write_committed_to_history(&ops, 3).await.unwrap();

    // The wrapper raised the published floor.
    assert!(gate.last_committed() >= 3);
    // …and seeded the cell, so the value is readable right away.
    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from_static(b"k")))
            .await
            .unwrap(),
        Some(Bytes::from_static(b"v3"))
    );
}

/// Publishing is idempotent: a second `publish_materialized` call changes
/// nothing and never regresses the floor (`raise_published` is monotonic).
#[tokio::test]
async fn publish_is_idempotent() {
    let (mvcc, gate) = make_mvcc();
    gate.raise_published(1);

    let ops = [set_op(b"k", b"v2")];
    mvcc.materialize_committed_to_history(&ops, 2)
        .await
        .unwrap();
    mvcc.publish_materialized(&ops, 2);
    let floor_after_first = gate.last_committed();
    assert!(floor_after_first >= 2);

    mvcc.publish_materialized(&ops, 2);
    assert_eq!(gate.last_committed(), floor_after_first);
    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from_static(b"k")))
            .await
            .unwrap(),
        Some(Bytes::from_static(b"v2"))
    );
}

/// S5c-X PR6b-2 round 3, review finding 6 — the zero-floor hazard.
///
/// A fresh gate (`RepoTxGate::fresh()`) has floor 0, and the follower receipt
/// pipeline durably materializes history rows BEFORE publishing (raising the
/// floor). During that materialize→publish window the gate floor is still 0 —
/// invariant I3 (history readers must not expose materialized-but-unpublished
/// versions) must hold EVEN at floor 0. Every read path must report the row
/// invisible; the raw history read is the negative control proving the row is
/// durable and would have been exposed pre-fix.
#[tokio::test]
async fn materialized_unpublished_invisible_at_zero_floor() {
    let (mvcc, gate) = make_mvcc();
    assert_eq!(gate.last_committed(), 0, "fresh gate starts at floor 0");

    let ops = [set_op(b"k", b"v1")];
    mvcc.materialize_committed_to_history(&ops, 1)
        .await
        .unwrap();

    // Negative control: the row IS durable in history while the floor is
    // still 0 — a pre-fix read path (unrestricted cold scan at floor 0) would
    // have returned exactly this value.
    assert_eq!(
        mvcc.history_store()
            .get(encode_version_key(b"k", 1).into())
            .await
            .unwrap(),
        Bytes::from_static(b"v1")
    );

    // Point reads (hot and cold path — cell absent, so both take the cold
    // scan) must NOT expose the unpublished version.
    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from_static(b"k")))
            .await
            .unwrap(),
        None
    );
    assert_eq!(mvcc.get_current_bytes(b"k").await.unwrap(), None);

    // Snapshot reads pinned at the floor (the only legitimate pin while
    // floor == 0) must not expose it either.
    assert_eq!(mvcc.get_at(b"k", 0).await.unwrap(), None);
    assert_eq!(
        mvcc.get_at_many(&[Bytes::from_static(b"k")], 0)
            .await
            .unwrap(),
        vec![None]
    );

    // The current stream must not emit the key.
    let current = collect_current(&mvcc).await;
    assert!(
        !current.iter().any(|(k, _)| k.as_slice() == b"k"),
        "unpublished version must not appear in current_stream at floor 0: {current:?}"
    );

    // Publish → visible on every path.
    mvcc.publish_materialized(&ops, 1);
    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from_static(b"k")))
            .await
            .unwrap(),
        Some(Bytes::from_static(b"v1"))
    );
    assert_eq!(
        mvcc.get_current_bytes(b"k").await.unwrap(),
        Some(Bytes::from_static(b"v1"))
    );
    assert_eq!(
        mvcc.get_at(b"k", 1).await.unwrap(),
        Some(Bytes::from_static(b"v1"))
    );
    let current = collect_current(&mvcc).await;
    assert!(
        current
            .iter()
            .any(|(k, v)| k.as_slice() == b"k" && v.as_ref() == b"v1"),
        "published version must appear in current_stream: {current:?}"
    );
}

/// A tombstone materialized at floor 0 must likewise stay invisible (and
/// absent — not a false "value"), and the cold path must not seed the cell
/// above the floor: after publishing a NEWER value the point read resolves
/// through the cell without resurrecting the unpublished row.
#[tokio::test]
async fn zero_floor_cold_path_does_not_seed_unpublished_cell() {
    let (mvcc, gate) = make_mvcc();
    assert_eq!(gate.last_committed(), 0);

    let ops = [set_op(b"k", b"v1")];
    mvcc.materialize_committed_to_history(&ops, 1)
        .await
        .unwrap();

    // Cold-path read while unpublished: must return None…
    assert_eq!(mvcc.get_current_bytes(b"k").await.unwrap(), None);
    // …and must NOT have cached the unpublished version as the key's current
    // version: a follow-up read must still be None (pre-fix, the cold scan
    // seeded the cell with the unpublished version and the guard was skipped
    // at floor 0, serving the row on the second read too).
    assert_eq!(mvcc.get_current_bytes(b"k").await.unwrap(), None);
    assert_eq!(mvcc.get_at(b"k", 0).await.unwrap(), None);

    mvcc.publish_materialized(&ops, 1);
    assert_eq!(
        mvcc.get_current_bytes(b"k").await.unwrap(),
        Some(Bytes::from_static(b"v1"))
    );
}

/// S5c-X PR6b-2 round 4 — THE zero-floor visibility regression test (round-3
/// review P1 #3): the round-3 fix post-filtered the GLOBAL max history version
/// version was materialized-but-UNPUBLISHED, `hist_v` collapsed to `None` and
/// the cold path returned `None` WITHOUT consulting older PUBLISHED history —
/// an older published row read as absent whenever a newer unpublished version
/// existed. The fix bounds the SCAN RANGE at the floor
/// (`seek_latest_published`), so the newest PUBLISHED version (v1 here) is
/// found and served while the unpublished v2 stays invisible on every path.
#[tokio::test]
async fn older_published_history_visible_beside_unpublished_version() {
    let (mvcc, gate) = make_mvcc();
    assert_eq!(gate.last_committed(), 0, "fresh gate starts at floor 0");

    // v1: materialize AND publish → floor 1, cell seeded.
    let ops1 = [set_op(b"k", b"v1")];
    mvcc.materialize_committed_to_history(&ops1, 1)
        .await
        .unwrap();
    mvcc.publish_materialized(&ops1, 1);
    assert_eq!(gate.last_committed(), 1);

    // v2: materialize WITHOUT publishing (the follower receipt's
    // materialize→publish window). The global max version is now 2 > floor 1.
    let ops2 = [set_op(b"k", b"v2")];
    mvcc.materialize_committed_to_history(&ops2, 2)
        .await
        .unwrap();

    // REGRESSION: the point read must return the PUBLISHED v1 value, not
    // None (round-3 post-filter made this None).
    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from_static(b"k")))
            .await
            .unwrap(),
        Some(Bytes::from_static(b"v1"))
    );
    assert_eq!(
        mvcc.get_current_bytes(b"k").await.unwrap(),
        Some(Bytes::from_static(b"v1"))
    );

    // Snapshot reads at the floor agree.
    assert_eq!(
        mvcc.get_at(b"k", 1).await.unwrap(),
        Some(Bytes::from_static(b"v1"))
    );
    assert_eq!(
        mvcc.get_at_many(&[Bytes::from_static(b"k")], 1)
            .await
            .unwrap(),
        vec![Some(Bytes::from_static(b"v1"))]
    );

    // The current stream emits the key with the v1 value (unpublished v2
    // skipped by the `v > floor` group-by filter in version_entry.rs).
    let current = collect_current(&mvcc).await;
    assert!(
        current
            .iter()
            .any(|(k, v)| k.as_slice() == b"k" && v.as_ref() == b"v1"),
        "published v1 must appear in current_stream beside unpublished v2: {current:?}"
    );
    assert!(
        !current
            .iter()
            .any(|(k, v)| k.as_slice() == b"k" && v.as_ref() == b"v2"),
        "unpublished v2 must not appear in current_stream: {current:?}"
    );

    // The ts index is publication-unaware by design: it resolves the newest
    // COMMIT-ts entry (including materialized v2) and the read paths apply
    // the floor on top. It must still resolve.
    assert_eq!(
        mvcc.version_at_or_before_ts(u64::MAX).await,
        Some(2),
        "ts index sees the materialized commit ts; the read paths bound it"
    );

    // Publish v2 → every path returns v2.
    mvcc.publish_materialized(&ops2, 2);
    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from_static(b"k")))
            .await
            .unwrap(),
        Some(Bytes::from_static(b"v2"))
    );
    assert_eq!(
        mvcc.get_current_bytes(b"k").await.unwrap(),
        Some(Bytes::from_static(b"v2"))
    );
    assert_eq!(
        mvcc.get_at(b"k", 2).await.unwrap(),
        Some(Bytes::from_static(b"v2"))
    );
    let current = collect_current(&mvcc).await;
    assert!(
        current
            .iter()
            .any(|(k, v)| k.as_slice() == b"k" && v.as_ref() == b"v2"),
        "published v2 must appear in current_stream: {current:?}"
    );
}

/// S5c-X PR6b-2 round 4 — the round-3 zero-floor property must survive the
/// bounded-scan fix: an UNPUBLISHED-ONLY key (v1 materialized, never
/// published, floor still 0) stays invisible on every read path. The bounded
/// scan returns `None` because the key has no version ≤ floor 0 — the same
/// answer as before, but now obtained by the range bound instead of a
/// post-filter that also broke older published rows.
#[tokio::test]
async fn zero_floor_hides_unpublished_only() {
    let (mvcc, gate) = make_mvcc();
    assert_eq!(gate.last_committed(), 0, "fresh gate starts at floor 0");

    let ops = [set_op(b"k2", b"v1")];
    mvcc.materialize_committed_to_history(&ops, 1)
        .await
        .unwrap();

    // Negative control: the row IS durable while the floor is still 0.
    assert_eq!(
        mvcc.history_store()
            .get(encode_version_key(b"k2", 1).into())
            .await
            .unwrap(),
        Bytes::from_static(b"v1")
    );

    // Point reads (cell absent → cold bounded scan).
    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from_static(b"k2")))
            .await
            .unwrap(),
        None
    );
    assert_eq!(mvcc.get_current_bytes(b"k2").await.unwrap(), None);

    // Snapshot reads pinned at the floor.
    assert_eq!(mvcc.get_at(b"k2", 0).await.unwrap(), None);
    assert_eq!(
        mvcc.get_at_many(&[Bytes::from_static(b"k2")], 0)
            .await
            .unwrap(),
        vec![None]
    );

    // The current stream must not emit the key.
    let current = collect_current(&mvcc).await;
    assert!(
        !current.iter().any(|(k, _)| k.as_slice() == b"k2"),
        "unpublished-only key must not appear in current_stream at floor 0: {current:?}"
    );

    // Publish → visible on every path.
    mvcc.publish_materialized(&ops, 1);
    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from_static(b"k2")))
            .await
            .unwrap(),
        Some(Bytes::from_static(b"v1"))
    );
    assert_eq!(
        mvcc.get_current_bytes(b"k2").await.unwrap(),
        Some(Bytes::from_static(b"v1"))
    );
    let current = collect_current(&mvcc).await;
    assert!(
        current
            .iter()
            .any(|(k, v)| k.as_slice() == b"k2" && v.as_ref() == b"v1"),
        "published version must appear in current_stream: {current:?}"
    );
}

/// S5c-X PR6b-2 round 5 (review P2 #7) — COLD-BRANCH regression.
///
/// `older_published_history_visible_beside_unpublished_version` above calls
/// `publish_materialized(&ops1, 1)`, which SEEDS the key's cell — so its
/// point reads take the WARM path (`current_version > 0`) and never reach
/// the cold `seek_latest_published` scan at all. Reverting the cold-scan
/// fix would still pass that test. This test forces the COLD path: after
/// publishing v1 (cell seeded) and materializing unpublished v2, the cell
/// is evicted via the test-only `evict_current_cell_for_test` helper, so
/// the next point read must resolve v1 through `seek_latest_published`
/// (the range-bounded scan). The round-3 post-filter behavior (global max
/// post-filtered at the floor) would collapse `hist_v` to `None` here and
/// return None for the point read — exactly what this test pins against.
#[tokio::test]
async fn cold_scan_finds_published_v1_beside_unpublished_v2() {
    let (mvcc, gate) = make_mvcc();

    // v1: materialize AND publish → floor 1, cell seeded (warm state).
    let ops1 = [set_op(b"k", b"v1")];
    mvcc.materialize_committed_to_history(&ops1, 1)
        .await
        .unwrap();
    mvcc.publish_materialized(&ops1, 1);
    assert_eq!(gate.last_committed(), 1);
    assert_eq!(
        mvcc.get_current_bytes(b"k").await.unwrap(),
        Some(Bytes::from_static(b"v1")),
        "warm path sanity: published v1 visible while the cell is seeded"
    );

    // v2: materialize WITHOUT publishing (global max version is now 2 > floor 1).
    let ops2 = [set_op(b"k", b"v2")];
    mvcc.materialize_committed_to_history(&ops2, 2)
        .await
        .unwrap();

    // Evict the cell → the next point read MUST take the cold path
    // (`current_version == 0` → `seek_latest_published`).
    mvcc.evict_current_cell_for_test(b"k");
    assert_eq!(
        mvcc.current_version(b"k"),
        0,
        "eviction must make the cell absent so the cold scan is exercised"
    );

    // REGRESSION: the cold bounded scan finds the newest PUBLISHED version
    // (v1) and serves it; the unpublished v2 stays invisible. The round-3
    // post-filter defect would return None here (global max 2 > floor 1,
    // filtered out with no fallback to older published history).
    assert_eq!(
        mvcc.get_current(RecordKey::from(Bytes::from_static(b"k")))
            .await
            .unwrap(),
        Some(Bytes::from_static(b"v1")),
        "cold scan must return published v1 beside unpublished v2 (post-filter defect returned None)"
    );

    // The cold read re-seeded the cell with the published version.
    assert_eq!(
        mvcc.current_version(b"k"),
        1,
        "cold path must seed the cell with the published version, not the unpublished one"
    );

    // The re-seeded warm read agrees.
    assert_eq!(
        mvcc.get_current_bytes(b"k").await.unwrap(),
        Some(Bytes::from_static(b"v1"))
    );

    // Snapshot read at the floor agrees (also cold-anchored via get_at's
    // range scan for snapshots below the cell version).
    assert_eq!(
        mvcc.get_at(b"k", 1).await.unwrap(),
        Some(Bytes::from_static(b"v1"))
    );
}
