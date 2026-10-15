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
