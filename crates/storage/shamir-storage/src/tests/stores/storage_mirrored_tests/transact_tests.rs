#![allow(deprecated)]

use crate::error::DbResult;
use crate::storage_in_memory::InMemoryStore;
use crate::storage_mirrored::{is_durable_table_config, MirroredStore};
use crate::types::{KvOp, RecordKey, RecordStream, Store};
use async_trait::async_trait;
use bytes::Bytes;
use shamir_types::types::record_id::RecordId;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use super::common::{classified_key, classified_key_2, unclassified_key, FailingTransactMirror};

/// Test-only mirror wrapper that, during `transact`, reads `primary`
/// through a late-bound handle to the wrapping `MirroredStore` and records
/// the observed value for a configured key. This deterministically
/// demonstrates what a concurrent reader sees while
/// `MirroredStore::transact` is in its durable (mirror-commit) phase.
/// Under F-59 the ephemeral loop runs AFTER the mirror commit, so at this
/// observation point ephemeral ops are NOT yet applied to `primary` — the
/// reader sees no ephemeral state during the durable phase (the inverse of
/// the pre-F-59 window, where ephemeral was applied first).
struct ObservingMirror {
    inner: Arc<dyn Store>,
    /// Late-bound handle to the `MirroredStore` wrapping this mirror.
    store_slot: Mutex<Option<Arc<dyn Store>>>,
    /// Key to read from `primary` (via the wrapping `MirroredStore`) during
    /// `transact`.
    observe_key: RecordKey,
    /// Value observed during `transact` (`None` if not yet observed or the
    /// key was absent in `primary` at that point).
    observed: Mutex<Option<Bytes>>,
}

#[async_trait]
impl Store for ObservingMirror {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        self.inner.set(key, value).await
    }
    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }
    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }
    async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
        // Read `primary` through the wrapping `MirroredStore` to observe
        // what a concurrent reader sees at this exact point — during the
        // durable mirror-commit phase. F-59 moved the ephemeral loop to
        // AFTER this point, so a reader here sees NO ephemeral state yet
        // (pre-F-59, ephemeral was applied before this and was visible
        // here). Clone the Arc out of the Mutex BEFORE awaiting so the
        // guard is not held across `.await` (Send requirement).
        let store_opt = self.store_slot.lock().unwrap().clone();
        if let Some(store) = store_opt {
            if let Ok(val) = store.get(self.observe_key.clone()).await {
                *self.observed.lock().unwrap() = Some(val);
            }
        }
        self.inner.transact(ops).await
    }
    fn iter_stream(&self, batch_size: usize) -> RecordStream {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> RecordStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

/// **Test 1 — durable-subset atomicity, happy path.**
///
/// A `transact` with 2+ classified (durable) ops applied together; confirm
/// both land in `mirror` (the durable backend) and in `primary` (reads go
/// to primary only).
#[tokio::test]
async fn transact_durable_subset_lands_atomically_in_mirror() {
    let mirror: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let store = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();

    let k1 = classified_key();
    let k2 = classified_key_2();
    assert!(is_durable_table_config(&k1));
    assert!(is_durable_table_config(&k2));

    store
        .transact(vec![
            KvOp::Set(k1.clone(), Bytes::from_static(b"v1")),
            KvOp::Set(k2.clone(), Bytes::from_static(b"v2")),
        ])
        .await
        .unwrap();

    // Both durable ops landed in mirror.
    assert_eq!(
        mirror.get(k1.clone()).await.unwrap(),
        Bytes::from_static(b"v1")
    );
    assert_eq!(
        mirror.get(k2.clone()).await.unwrap(),
        Bytes::from_static(b"v2")
    );

    // Both also visible through the facade (primary got them in Phase 2).
    assert_eq!(store.get(k1).await.unwrap(), Bytes::from_static(b"v1"));
    assert_eq!(store.get(k2).await.unwrap(), Bytes::from_static(b"v2"));
}

/// **Test 2 — durable-subset atomicity, injected failure.**
///
/// A `FailingTransactMirror` wraps the mirror so its `transact` returns
/// `Err` without applying ANY ops (simulating an atomic backend's batch-
/// commit failure). Confirm: NO partial durable state exists in the mirror
/// after the failure (the actual atomicity proof — not just "an error was
/// returned"), AND (F-49) `primary` does NOT hold the durable ops either —
/// the mirror-first reorder means the mirror failure aborts before primary
/// is ever touched for the durable subset.
#[tokio::test]
async fn transact_durable_subset_failure_leaves_no_partial_durable_state() {
    let mirror_inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    // Seed the mirror with a pre-existing durable key so we can verify
    // it survives unchanged (proving the failure didn't corrupt prior
    // state) alongside confirming the NEW ops didn't land.
    let pre_existing = classified_key_2();
    mirror_inner
        .set(pre_existing.clone(), Bytes::from_static(b"pre-existing"))
        .await
        .unwrap();

    let failing_mirror = Arc::new(FailingTransactMirror {
        inner: mirror_inner.clone(),
        fail_transact: AtomicBool::new(true),
    });
    let mirror_dyn: Arc<dyn Store> = failing_mirror.clone();

    let store = MirroredStore::new(mirror_dyn, is_durable_table_config)
        .await
        .unwrap();

    let k1 = classified_key();
    let k2 = RecordKey::from_slice(RecordId::system("_m.wal").as_bytes());
    assert!(is_durable_table_config(&k2));

    let result = store
        .transact(vec![
            KvOp::Set(k1.clone(), Bytes::from_static(b"v1")),
            KvOp::Set(k2.clone(), Bytes::from_static(b"v2")),
        ])
        .await;

    // The durable write failed (propagated to caller).
    assert!(result.is_err(), "durable transact failure should propagate");

    // NO partial durable state: neither of the two new durable ops landed
    // in the mirror. This is the all-or-nothing atomicity proof.
    assert!(
        mirror_inner.get(k1.clone()).await.is_err(),
        "k1 must NOT be in mirror after atomic failure"
    );
    assert!(
        mirror_inner.get(k2.clone()).await.is_err(),
        "k2 must NOT be in mirror after atomic failure"
    );

    // Pre-existing durable state is unchanged.
    assert_eq!(
        mirror_inner.get(pre_existing.clone()).await.unwrap(),
        Bytes::from_static(b"pre-existing"),
        "prior durable state must survive the failed transact unchanged"
    );

    // Primary does NOT hold the durable ops (F-49/F-59: the mirror
    // transact — the only fallible step — runs FIRST, so its failure
    // aborts before `primary` is touched for the durable subset).
    assert!(
        store.get(k1).await.is_err(),
        "primary must NOT hold durable op k1 after mirror failure (mirror-first)"
    );
    assert!(
        store.get(k2).await.is_err(),
        "primary must NOT hold durable op k2 after mirror failure (mirror-first)"
    );
}

/// **Test 3 — mixed ephemeral + durable batch routing.**
///
/// A single `transact` call with BOTH ephemeral and durable ops. Confirm
/// ephemeral ops land in `primary` ONLY (not `mirror`), and durable ops
/// land in BOTH `primary` and `mirror`.
#[tokio::test]
async fn transact_mixed_batch_routes_ephemeral_to_primary_and_durable_to_both() {
    let mirror: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let store = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();

    let ephemeral = unclassified_key();
    let durable = classified_key();
    assert!(!is_durable_table_config(&ephemeral));
    assert!(is_durable_table_config(&durable));

    store
        .transact(vec![
            KvOp::Set(ephemeral.clone(), Bytes::from_static(b"eph")),
            KvOp::Set(durable.clone(), Bytes::from_static(b"dur")),
        ])
        .await
        .unwrap();

    // Ephemeral: in primary (visible via facade), NOT in mirror.
    assert_eq!(
        store.get(ephemeral.clone()).await.unwrap(),
        Bytes::from_static(b"eph")
    );
    assert!(
        mirror.get(ephemeral).await.is_err(),
        "ephemeral op must NOT reach mirror"
    );

    // Durable: in BOTH primary (facade) and mirror.
    assert_eq!(
        store.get(durable.clone()).await.unwrap(),
        Bytes::from_static(b"dur")
    );
    assert_eq!(
        mirror.get(durable).await.unwrap(),
        Bytes::from_static(b"dur"),
        "durable op must reach mirror"
    );
}

/// **Test 4 — whole-batch error atomicity (F-59 mirror-first-for-both).**
///
/// Force the durable half to fail (same `FailingTransactMirror` technique
/// as test 2) while the ephemeral half would otherwise succeed. Confirm
/// F-59's all-or-nothing guarantee for the WHOLE mixed batch:
/// - `primary` does NOT reflect the ephemeral ops (F-59: the ephemeral
///   loop now runs AFTER the mirror commit, so a mirror failure aborts
///   before `primary` is touched for EITHER subset). Pre-F-59, the
///   ephemeral loop ran BEFORE the mirror commit and landed in `primary`
///   unconditionally — the caller then saw `Err` despite part of the
///   batch being externally visible, the bug this test now guards
///   against regressing.
/// - `primary` does NOT reflect the durable ops (same reason — the only
///   fallible step is the mirror commit, which runs first).
/// - `mirror` does NOT reflect the durable ops (correctly rolled back to
///   nothing by the mirror backend's own atomic failure).
///
/// This test is RED on the pre-F-59 ordering (the ephemeral assertion
/// fails because the ephemeral loop ran first and mutated `primary`
/// before the mirror failure) and GREEN after the F-59 reorder.
#[tokio::test]
async fn transact_neither_subset_applied_on_mirror_failure() {
    let mirror_inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let failing_mirror = Arc::new(FailingTransactMirror {
        inner: mirror_inner.clone(),
        fail_transact: AtomicBool::new(true),
    });
    let mirror_dyn: Arc<dyn Store> = failing_mirror.clone();

    let store = MirroredStore::new(mirror_dyn, is_durable_table_config)
        .await
        .unwrap();

    let ephemeral = unclassified_key();
    let durable = classified_key();

    let result = store
        .transact(vec![
            KvOp::Set(ephemeral.clone(), Bytes::from_static(b"eph")),
            KvOp::Set(durable.clone(), Bytes::from_static(b"dur")),
        ])
        .await;
    assert!(result.is_err(), "durable failure should propagate");

    // F-59: primary reflects NEITHER subset — the mirror commit (the only
    // fallible step) runs FIRST, so its failure aborts before `primary` is
    // touched for ephemeral OR durable. The ephemeral assertion is the one
    // that flips from the pre-F-59 behavior (it used to land in primary
    // before the mirror commit was even attempted).
    assert!(
        store.get(ephemeral.clone()).await.is_err(),
        "primary must NOT reflect ephemeral ops after mirror failure \
         (F-59: ephemeral loop runs AFTER the mirror commit, so a mirror \
         failure aborts before primary is touched for either subset)"
    );
    assert!(
        store.get(durable.clone()).await.is_err(),
        "primary must NOT reflect durable ops after mirror failure \
         (F-59: mirror commit runs first — primary untouched on failure)"
    );

    // Mirror does NOT reflect the durable ops — the mirror's transact
    // failed atomically (nothing applied).
    assert!(
        mirror_inner.get(durable.clone()).await.is_err(),
        "mirror must NOT reflect durable ops after atomic failure"
    );

    // And of course ephemeral never touches mirror at all.
    assert!(
        mirror_inner.get(ephemeral).await.is_err(),
        "ephemeral ops never reach mirror"
    );

    // Self-heal proof: a FRESH MirroredStore over the same mirror hydrates
    // from mirror only (unchanged by the failed transact) — neither the
    // ephemeral NOR the durable ops from the failed transact survive.
    let reopened = MirroredStore::new(mirror_inner, is_durable_table_config)
        .await
        .unwrap();
    assert!(
        reopened.get(durable).await.is_err(),
        "reopened store must not see failed-transact durable ops"
    );
}

/// **Test 5 — concurrent-reader visibility during transact (honest test,
/// updated for F-59).**
///
/// An `ObservingMirror` reads `primary` (via the wrapping `MirroredStore`'s
/// `get`) at the exact moment the durable mirror-commit phase runs. This
/// test documents the visibility boundary at that point.
///
/// **F-59 changed this boundary.** Pre-F-59 the ephemeral loop ran BEFORE
/// the mirror commit, so a reader observing during the durable phase saw
/// ephemeral state already applied to `primary` — a concurrency window the
/// old version of this test asserted as `Some(ephemeral_val)`. F-59 moves
/// the ephemeral loop to AFTER the mirror commit (the same reorder that
/// closes the F-59 error-atomicity bug), so a reader observing during the
/// durable mirror-commit phase now sees NO ephemeral state yet (`None`).
/// This is a genuine narrowing of the observable window, and the flipped
/// assertion is a regression guard proving the F-59 reorder is in effect.
///
/// This test does NOT assert the finer-grained "concurrent-reader
/// residual" still documented in `MirroredStore::transact`'s doc comment —
/// a reader seeing PARTIAL ephemeral state (some but not all ephemeral ops)
/// mid-ephemeral-loop. That residual is UNCHANGED by F-59 (the ephemeral
/// loop still applies ops one at a time with no cross-op atomicity) but is
/// not deterministically testable here: `InMemoryStore::set` completes
/// synchronously with no yield point between ephemeral ops for a concurrent
/// reader to interleave at. Asserting it would claim a guarantee stronger
/// than what is implemented — exactly what the brief says NOT to do.
#[tokio::test]
async fn transact_concurrent_reader_during_mirror_commit_sees_no_ephemeral_state() {
    let mirror_inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let ephemeral_key = unclassified_key();
    let ephemeral_val = Bytes::from_static(b"visible-mid-transact");

    let observing = Arc::new(ObservingMirror {
        inner: mirror_inner.clone(),
        store_slot: Mutex::new(None),
        observe_key: ephemeral_key.clone(),
        observed: Mutex::new(None),
    });
    let mirror_dyn: Arc<dyn Store> = observing.clone();

    let store = Arc::new(
        MirroredStore::new(mirror_dyn, is_durable_table_config)
            .await
            .unwrap(),
    );

    // Late-bind: give the observing mirror a handle to the wrapping
    // MirroredStore so it can read `primary` during its own `transact`.
    let store_dyn: Arc<dyn Store> = store.clone();
    *observing.store_slot.lock().unwrap() = Some(store_dyn);

    let durable_key = classified_key();

    store
        .transact(vec![
            KvOp::Set(ephemeral_key.clone(), ephemeral_val.clone()),
            KvOp::Set(durable_key.clone(), Bytes::from_static(b"dur")),
        ])
        .await
        .unwrap();

    // The observing mirror read `primary` during the durable mirror-commit
    // phase. F-59: the ephemeral loop runs AFTER the mirror commit, so at
    // this observation point ephemeral state is NOT yet in `primary` — the
    // reader sees `None`. (Pre-F-59 this was `Some(ephemeral_val)` because
    // ephemeral was applied before the mirror commit; the flip is the F-59
    // regression guard.)
    let observed = observing.observed.lock().unwrap().clone();
    assert!(
        observed.is_none(),
        "a reader during the durable mirror-commit phase must NOT see \
         ephemeral state yet — F-59 applies the ephemeral loop AFTER the \
         mirror commit (pre-F-59 this window showed the ephemeral value); \
         observed = {observed:?}"
    );

    // The durable op also landed correctly (transact completed).
    assert_eq!(
        mirror_inner.get(durable_key).await.unwrap(),
        Bytes::from_static(b"dur")
    );
}

// ============================================================================
// F-41: MirroredStore set/remove mirror-first write-ordering + hydration
// classifier re-filter tests (concern 1 + concern 2)
// ============================================================================
