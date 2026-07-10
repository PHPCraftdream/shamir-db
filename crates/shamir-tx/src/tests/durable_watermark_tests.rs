//! P1d-1 — durable_watermark machinery (additive, zero behavior change).
//!
//! Asserts the two invariants that justify the durable tracker as a drop-in
//! for the upcoming P1d-2 cutover:
//!
//! - Under the current inline-materialize path, the durable watermark must
//!   end every commit sequence equal to the visibility watermark
//!   (`gate.durable_watermark() == gate.last_committed()`). Inline-materialize
//!   means data + index land in `history` synchronously on the ack-path, so
//!   "durable in history" and "reader-visible" coincide.
//! - `durable_watermark() <= last_committed()` at every observation point
//!   throughout the sequence — durable must never lead visibility. The
//!   implementation enforces this by ordering: every site marks visibility
//!   first (`guard.commit()`) and durable second (`mark_durable`), and the
//!   abort path in `VersionGuard::drop` marks BOTH trackers Aborted so the
//!   contiguous prefix on the durable tracker keeps pace with visibility.
//!
//! Tx-path coverage (full commit through `commit_tx` / group-commit /
//! AsyncIndex) lives in `shamir-engine::tx::tests::durable_watermark_tests` —
//! it needs the engine-side `RepoInstance`/`commit_tx` machinery.

use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;

use crate::mvcc_store::MvccStore;
use crate::repo_tx_gate::RepoTxGate;
use shamir_storage::types::RecordKey;

fn make_store_with_gate() -> (Arc<MvccStore>, Arc<RepoTxGate>) {
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let gate = Arc::new(RepoTxGate::fresh());
    let store = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    (store, gate)
}

/// Fresh gate seeded at 0 starts with both watermarks at 0 — the durable
/// tracker is constructed with the same `with_watermark(last_committed)` seed
/// as visibility (see `RepoTxGate::new`), so a freshly opened repo's durable
/// floor is identical to its visibility floor.
#[test]
fn fresh_gate_durable_equals_visibility_at_zero() {
    let gate = RepoTxGate::fresh();
    assert_eq!(gate.durable_watermark(), 0);
    assert_eq!(gate.last_committed(), 0);
    assert_eq!(gate.durable_watermark(), gate.last_committed());
}

/// A gate seeded by recovery (`RepoTxGate::new(v, _)`) reproduces the
/// invariant at open time: visibility recovered → so was durability.
#[test]
fn seeded_gate_durable_matches_seed() {
    let gate = RepoTxGate::new(42, 1);
    assert_eq!(gate.durable_watermark(), 42);
    assert_eq!(gate.last_committed(), 42);
}

/// After N non-tx `set_versioned` calls, durable watermark catches up to the
/// visibility watermark. The inline history write lands BEFORE the
/// `guard.commit()` → `mark_durable` pair, so by the time the call returns
/// both watermarks have moved to the same version.
#[tokio::test]
async fn nontx_set_versioned_advances_durable_in_lockstep() {
    let (store, gate) = make_store_with_gate();
    for i in 0..5u8 {
        let key = Bytes::from(vec![b'k', i]);
        let val = Bytes::from(vec![b'v', i]);
        let v = store.set_versioned(RecordKey::from(key), val).await.unwrap();
        let dur = gate.durable_watermark();
        let vis = gate.last_committed();
        // Invariant: durable never leads visibility.
        assert!(
            dur <= vis,
            "durable={dur} must not lead visibility={vis} after set_versioned v={v}"
        );
        // Under inline-materialize on the happy path they end equal at the
        // version just assigned.
        assert_eq!(dur, vis, "inline materialize → durable == visibility");
        assert_eq!(vis, v, "visibility equals just-assigned version");
    }
    assert_eq!(gate.durable_watermark(), 5);
}

/// Batched non-tx write (`set_versioned_many`): every version in the batch
/// is durable AFTER the single `history.transact` succeeds and every guard's
/// `commit()` lands; durable watermark catches up to the batch max.
#[tokio::test]
async fn nontx_set_versioned_many_advances_durable_to_batch_max() {
    let (store, gate) = make_store_with_gate();
    let batch: Vec<(Bytes, Bytes)> = (0..4u8)
        .map(|i| (Bytes::from(vec![b'k', i]), Bytes::from(vec![b'v', i])))
        .collect();
    let max_v = store.set_versioned_many(batch.into_iter().map(|(k, v)| (RecordKey::from(k), v)).collect::<Vec<_>>()).await.unwrap();
    assert_eq!(max_v, 4);
    assert_eq!(gate.last_committed(), max_v);
    assert_eq!(
        gate.durable_watermark(),
        gate.last_committed(),
        "durable catches up after the batched history.transact"
    );
}

/// Delete (`delete_versioned`) follows the same ordering — tombstone written,
/// `guard.commit()`, then `mark_durable`. Mixed with sets to exercise
/// monotonicity across operations.
#[tokio::test]
async fn nontx_delete_versioned_advances_durable() {
    let (store, gate) = make_store_with_gate();
    let _ = store
        .set_versioned(RecordKey::from(Bytes::from_static(b"k")), Bytes::from_static(b"v1"))
        .await
        .unwrap();
    let prev_dur = gate.durable_watermark();
    let prev_vis = gate.last_committed();
    assert_eq!(prev_dur, prev_vis);

    let v = store
        .delete_versioned(RecordKey::from(Bytes::from_static(b"k")))
        .await
        .unwrap();
    assert!(v > prev_vis);
    let dur = gate.durable_watermark();
    let vis = gate.last_committed();
    assert!(dur <= vis, "durable={dur} <= visibility={vis} after delete");
    assert_eq!(
        dur, vis,
        "delete: inline materialize → durable == visibility"
    );
    assert_eq!(vis, v);
}

/// Mixed sequence of set / delete / batched-set across multiple keys — at
/// every observation point durable <= visibility, and at the end they are
/// equal at the highest assigned version. Stresses the lock-step property
/// across interleaved single and batched non-tx paths.
#[tokio::test]
async fn nontx_mixed_sequence_durable_le_visibility_throughout() {
    let (store, gate) = make_store_with_gate();
    let observe = |gate: &Arc<RepoTxGate>, label: &str| {
        let d = gate.durable_watermark();
        let v = gate.last_committed();
        assert!(d <= v, "{label}: durable={d} must not lead visibility={v}");
        (d, v)
    };

    let _ = store
        .set_versioned(RecordKey::from(Bytes::from_static(b"a")), Bytes::from_static(b"1"))
        .await
        .unwrap();
    observe(&gate, "after set a=1");

    let _ = store
        .set_versioned_many(vec![
            (RecordKey::from_slice(b"b"), Bytes::from_static(b"2")),
            (RecordKey::from_slice(b"c"), Bytes::from_static(b"3")),
        ])
        .await
        .unwrap();
    observe(&gate, "after batched set {b,c}");

    let _ = store
        .delete_versioned(RecordKey::from(Bytes::from_static(b"a")))
        .await
        .unwrap();
    observe(&gate, "after delete a");

    let _ = store
        .set_versioned(RecordKey::from(Bytes::from_static(b"d")), Bytes::from_static(b"4"))
        .await
        .unwrap();
    let (d, v) = observe(&gate, "final");

    assert_eq!(
        d, v,
        "end of inline-materialize sequence: durable == visibility"
    );
    // 4 ops × {1, 2, 1, 1} versions = 5 total versions assigned.
    assert_eq!(v, 5);
}

/// Aborting a `VersionGuard` (drop without commit — emulating the
/// SSI/phantom/WAL-fail abort path) advances BOTH trackers' watermarks
/// past the burned version, so a subsequent successful commit lands the
/// durable watermark in lock-step with visibility — proves the durable
/// tracker does not wedge behind aborted versions.
#[test]
fn aborted_version_advances_both_watermarks() {
    let gate = RepoTxGate::fresh();

    // Burn version 1: assign + drop without commit.
    let g1 = gate.assign_next_version_guarded();
    assert_eq!(g1.version(), 1);
    drop(g1);

    // Visibility advanced via Drop → mark(Aborted). Durable likewise.
    assert_eq!(gate.last_committed(), 1, "Aborted advances visibility");
    assert_eq!(
        gate.durable_watermark(),
        1,
        "Aborted also advances durable so it does not wedge below visibility"
    );

    // Commit version 2 successfully + mark durable as the production sites do.
    let g2 = gate.assign_next_version_guarded();
    let v2 = g2.version();
    g2.commit();
    gate.mark_durable(v2);

    assert_eq!(gate.last_committed(), v2);
    assert_eq!(gate.durable_watermark(), v2);
    assert_eq!(gate.durable_watermark(), gate.last_committed());
}

/// `mark_durable` is idempotent and order-independent — guards against
/// double-mark hazards in P1d-2 (where the drain leader may mark a version
/// already marked by a transient inline path during the cutover).
#[test]
fn mark_durable_idempotent_and_order_independent() {
    let gate = RepoTxGate::fresh();
    let g1 = gate.assign_next_version_guarded();
    let g2 = gate.assign_next_version_guarded();
    let v1 = g1.version();
    let v2 = g2.version();

    // Commit visibility for both first so versions are visible.
    g1.commit();
    g2.commit();
    assert_eq!(gate.last_committed(), v2);

    // Mark durable out of order, twice. Watermark still advances to v2.
    gate.mark_durable(v2);
    gate.mark_durable(v1);
    gate.mark_durable(v2); // duplicate — no-op
    gate.mark_durable(v1); // duplicate — no-op
    assert_eq!(gate.durable_watermark(), v2);
    assert!(gate.durable_watermark() <= gate.last_committed());
}
