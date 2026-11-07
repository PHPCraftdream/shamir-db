//! RV-TX-D54D3EE5 — batch-abort watermark oracles for the REAL batched
//! write APIs (`set_versioned_many` / `set_versioned_many_append_only`).
//!
//! Failure ordering confirmed in `store.rs::set_versioned_many_inner` /
//! `set_versioned_many_append_only_inner`: guards are assigned per key
//! BEFORE the batched `history.transact`; on transact error the attempted
//! version+ts keys are compensated away (`compensate_failed_write`) and the
//! guard vector drops un-committed → every batch version settles Aborted,
//! so the contiguous watermark advances PAST the whole failed batch. The
//! overlay is only populated AFTER a successful transact, so a failed batch
//! leaves no overlay entries.

use bytes::Bytes;
use shamir_storage::types::RecordKey;
use std::sync::atomic::Ordering;

use crate::mvcc_store::tests::gc::helpers::make_gate;
use crate::mvcc_store::tests::gc::test_stores::make_failing_history_mvcc;

/// RV-TX-D54D3EE5 — a failed `set_versioned_many` (existing key `k` seeded
/// at v5 + a fresh key) must: return Err; settle both failed batch versions
/// (v6, v7) as Aborted so `last_committed()` ADVANCES PAST them; leave no
/// failed (key, version) pair in the overlay; keep
/// `durable_watermark() <= last_committed()`; and let a subsequent write
/// allocate and commit the NEXT version (proof the floor really moved).
///
/// FAILS ON HEAD if the batch error path never settles the guards as
/// Aborted — e.g. `mem::forget`-ing the guard vector or committing them in
/// `set_versioned_many_inner`'s `if let Err(e) = self.history.transact(...)`
/// arm: the contiguous watermark wedges below the failed versions, so the
/// `last_committed() >= failed_max` and `next == failed_max + 1`
/// assertions fail (the follow-up write hangs in strict_ack instead).
#[tokio::test]
async fn rv_tx_d54d3ee5_failed_set_versioned_many_aborts_and_advances_watermark() {
    let gate = make_gate();
    let (mvcc, history) = make_failing_history_mvcc(gate.clone());

    let k = RecordKey::from(Bytes::from_static(b"k"));
    let seed_v = mvcc
        .set_versioned(k.clone(), Bytes::from_static(b"v5"))
        .await
        .unwrap();
    let fresh = RecordKey::from(Bytes::from_static(b"fresh"));

    history.fail_set.store(true, Ordering::Relaxed);
    let err = mvcc
        .set_versioned_many(vec![
            (k.clone(), Bytes::from_static(b"v6")),
            (fresh.clone(), Bytes::from_static(b"fv")),
        ])
        .await;
    assert!(err.is_err(), "the injected fault must fail the batch");
    history.fail_set.store(false, Ordering::Relaxed);

    // Failed batch versions: seed_v+1 (k) and seed_v+2 (fresh). Both guards
    // dropped un-committed → Aborted → the contiguous floor moves past BOTH.
    let failed_max = seed_v + 2;
    assert!(
        gate.last_committed() >= failed_max,
        "batch-abort: contiguous watermark must advance past the whole failed \
         batch (>= {failed_max}), got {}",
        gate.last_committed()
    );
    assert!(
        gate.durable_watermark() <= gate.last_committed(),
        "durable_watermark must never exceed last_committed"
    );

    // No failed pair in the overlay: only the seeded entry survives.
    assert_eq!(
        mvcc.overlay_len(),
        1,
        "overlay must not hold any failed batch entry"
    );
    assert_eq!(
        mvcc.get_current(k.clone()).await.unwrap().as_deref(),
        Some(&b"v5"[..]),
        "the prior committed value must stay visible"
    );
    assert_eq!(
        mvcc.get_current(fresh.clone()).await.unwrap(),
        None,
        "the fresh failed key must be absent"
    );

    // Advancement proof: the next write allocates exactly the version after
    // the failed batch and commits normally (strict_ack completes).
    let next = mvcc
        .set_versioned(
            RecordKey::from(Bytes::from_static(b"after")),
            Bytes::from_static(b"ok"),
        )
        .await
        .unwrap();
    assert_eq!(
        next,
        failed_max + 1,
        "the floor must have advanced past the failed batch versions"
    );
}

/// RV-TX-D54D3EE5 — the append-only batch variant behaves identically on
/// fault: Err, watermark past the failed versions, empty overlay for the
/// failed fresh keys, durable <= last_committed.
///
/// FAILS ON HEAD under the same single revert as
/// `rv_tx_d54d3ee5_failed_set_versioned_many_aborts_and_advances_watermark`
/// applied to `set_versioned_many_append_only_inner` (guards forgotten /
/// settled non-Aborted on the transact-error path): the watermark wedges
/// and the `last_committed() >= failed_max` assertion fails.
#[tokio::test]
async fn rv_tx_d54d3ee5_failed_append_only_batch_aborts_and_advances_watermark() {
    let gate = make_gate();
    let (mvcc, history) = make_failing_history_mvcc(gate.clone());

    let seed = RecordKey::from(Bytes::from_static(b"seed"));
    let seed_v = mvcc
        .set_versioned(seed, Bytes::from_static(b"v5"))
        .await
        .unwrap();

    let f1 = RecordKey::from(Bytes::from_static(b"f1"));
    let f2 = RecordKey::from(Bytes::from_static(b"f2"));

    history.fail_set.store(true, Ordering::Relaxed);
    let err = mvcc
        .set_versioned_many_append_only(vec![
            (f1.clone(), Bytes::from_static(b"a")),
            (f2.clone(), Bytes::from_static(b"b")),
        ])
        .await;
    assert!(err.is_err(), "the injected fault must fail the batch");
    history.fail_set.store(false, Ordering::Relaxed);

    let failed_max = seed_v + 2;
    assert!(
        gate.last_committed() >= failed_max,
        "batch-abort: contiguous watermark must advance past the failed \
         append-only batch (>= {failed_max}), got {}",
        gate.last_committed()
    );
    assert!(gate.durable_watermark() <= gate.last_committed());
    assert_eq!(
        mvcc.overlay_len(),
        1,
        "overlay must hold only the seeded entry, no failed batch entries"
    );
    assert_eq!(mvcc.get_current(f1).await.unwrap(), None);
    assert_eq!(mvcc.get_current(f2).await.unwrap(), None);
}
