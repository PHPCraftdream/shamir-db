#![allow(deprecated)]

use crate::error::DbResult;
use crate::storage_in_memory::InMemoryStore;
use crate::storage_mirrored::{is_durable_table_config, MirroredStore};
use crate::tests::types_tests::collect_stream;
use crate::types::{KvOp, RecordKey, RecordStream, Store};
use async_trait::async_trait;
use bytes::Bytes;
use shamir_types::types::record_id::RecordId;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use super::common::{
    classified_key, classified_key_2, install_capturing_logger, FailingTransactMirror,
};

/// **Test 3 — hydration re-runs the classifier and skips drifted keys
/// (with the diagnostic logged).**
///
/// Simulates classifier drift: write a key DIRECTLY into the underlying
/// mirror store (bypassing `MirroredStore`'s own classify-gated `set`)
/// that would NOT pass the CURRENT classifier, then construct a FRESH
/// `MirroredStore` over it. Assert:
/// - (behaviour) the drifted key is SKIPPED — not loaded into `primary`
///   (a fresh `get` through the facade misses), while a classified key
///   in the same mirror IS loaded;
/// - (diagnostic) exactly one `warn!` naming the hydration-skip path
///   was emitted.
///
/// The drifted key uses the `MetaKey::Count` tag (`"count"`) — a real
/// 16-byte system record the CURRENT classifier deliberately EXCLUDES
/// because it is derived from row data. This is the canonical
/// "classifier drift" shape: a key an OLD classifier might have accepted
/// as durable, present in the mirror, that the new one rejects.
#[tokio::test]
async fn f41_hydration_re_filters_against_current_classifier_and_skips_drifted_keys() {
    let captured = install_capturing_logger();
    captured.lock().unwrap().clear();

    let mirror: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    // A classified key — would hydrate normally.
    let durable_key = classified_key();
    // A drifted key — `MetaKey::Count`-shaped (`"count"`): a real 16-byte
    // system record the current classifier EXCLUDES. Written directly to
    // the mirror, bypassing `MirroredStore`'s gated `set`.
    let drifted_key = RecordKey::from_slice(RecordId::system("count").as_bytes());
    assert!(
        !is_durable_table_config(&drifted_key),
        "drifted key must FAIL the current classifier (Count is excluded)"
    );
    assert!(
        is_durable_table_config(&durable_key),
        "durable control key must PASS the current classifier"
    );

    mirror
        .set(drifted_key.clone(), Bytes::from_static(b"stale-count"))
        .await
        .unwrap();
    mirror
        .set(durable_key.clone(), Bytes::from_static(b"cfg"))
        .await
        .unwrap();

    // Construct a FRESH MirroredStore — hydration runs the classifier
    // over every streamed entry.
    let reopened = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();

    // Behaviour: durable key loaded, drifted key SKIPPED.
    assert_eq!(
        reopened.get(durable_key.clone()).await.unwrap(),
        Bytes::from_static(b"cfg"),
        "classified key in mirror must hydrate into primary"
    );
    assert!(
        reopened.get(drifted_key.clone()).await.is_err(),
        "drifted (classifier-rejected) key must be SKIPPED during hydration, not loaded into primary"
    );

    // Diagnostic: exactly one hydration-skip warn was emitted.
    let msgs = captured.lock().unwrap();
    let skip_msgs: Vec<&String> = msgs
        .iter()
        .filter(|m| m.contains("MirroredStore hydration"))
        .collect();
    assert_eq!(
        skip_msgs.len(),
        1,
        "expected exactly one hydration-skip diagnostic, got: {:?}",
        *msgs
    );
    assert!(
        skip_msgs[0].contains("skipping"),
        "diagnostic must name the skip action: {:?}",
        skip_msgs[0]
    );
}

// ============================================================================
// F-49: MirroredStore::transact mirror-first ordering for the durable subset
// ============================================================================

/// **F-49 — `transact` mirror-first ordering: a failed durable mirror
/// write must leave NO durable-subset effects visible in `primary` via
/// the live read path (`get` / `iter_stream`), matching the same
/// error-atomicity guarantee F-41 already gives single-key `set` /
/// `remove`.**
///
/// Before F-49, `transact` applied durable ops to `primary` (old Phase 2)
/// BEFORE committing them to `mirror` atomically (old Phase 3). If the
/// mirror write failed, the caller saw `Err` — but `primary` was already
/// mutated, so the live process served state that was never durably
/// committed (exactly the bug class F-41 closed for `set`/`remove`,
/// reopened here for the transact batch).
///
/// This test is RED on the pre-F-49 code (the live-read assertions fail
/// because primary was mutated before the mirror failure) and GREEN after
/// the mirror-first reorder. It verifies BOTH immediate live reads (the
/// review's explicit ask — not just reopen behavior, which F-39's test 4
/// already covers) AND the reopen path.
#[tokio::test]
async fn f49_transact_mirror_failure_leaves_no_durable_effects_in_primary_live_read() {
    let mirror_inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    // Seed a pre-existing durable key into the mirror so we can prove a
    // failed transact's durable Remove op does NOT remove it from primary
    // either (the mirror failure aborts before primary is touched —
    // symmetric to the Set case).
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

    // Confirm the pre-existing key hydrated into primary.
    assert_eq!(
        store.get(pre_existing.clone()).await.unwrap(),
        Bytes::from_static(b"pre-existing")
    );

    let durable_set_key = classified_key();
    assert!(is_durable_table_config(&durable_set_key));
    assert!(is_durable_table_config(&pre_existing));

    // The transact's durable subset: a Set of a new key AND a Remove of
    // the pre-existing key. Both are classified durable, so both land in
    // the durable subset that goes through mirror.transact.
    let result = store
        .transact(vec![
            KvOp::Set(durable_set_key.clone(), Bytes::from_static(b"new-durable")),
            KvOp::Remove(pre_existing.clone()),
        ])
        .await;
    assert!(result.is_err(), "mirror transact failure must propagate");

    // === IMMEDIATE LIVE READ via MirroredStore's own `get` ===
    // The durable Set op must NOT be visible in primary — the mirror
    // failed, so primary was never touched for the durable subset.
    assert!(
        store.get(durable_set_key.clone()).await.is_err(),
        "durable Set op must NOT be visible in primary after mirror \
         failure (mirror-first: primary untouched on durable mirror failure)"
    );
    // The durable Remove op must NOT have removed the pre-existing key
    // from primary — same reason.
    assert_eq!(
        store.get(pre_existing.clone()).await.unwrap(),
        Bytes::from_static(b"pre-existing"),
        "durable Remove op must NOT have taken effect in primary after \
         mirror failure"
    );

    // === IMMEDIATE LIVE READ via MirroredStore's own `iter_stream` ===
    // Collect all keys visible through the facade. The durable Set op's
    // key must not appear; the pre-existing key must still be there.
    let all_entries = collect_stream(store.iter_stream(16)).await.unwrap();
    let has_set_key = all_entries.iter().any(|(k, _)| *k == durable_set_key);
    let has_pre_existing = all_entries.iter().any(|(k, _)| *k == pre_existing);
    assert!(
        !has_set_key,
        "iter_stream must NOT surface the durable Set op's key after \
         mirror failure"
    );
    assert!(
        has_pre_existing,
        "iter_stream must STILL surface the pre-existing key (durable \
         Remove was aborted)"
    );

    // === REOPEN behavior ===
    // A FRESH MirroredStore over the same mirror hydrates from mirror
    // only — the failed transact changed nothing durably, so neither the
    // Set nor the Remove survives.
    let reopened = MirroredStore::new(mirror_inner, is_durable_table_config)
        .await
        .unwrap();
    assert!(
        reopened.get(durable_set_key).await.is_err(),
        "reopened store must not see the failed durable Set op"
    );
    assert_eq!(
        reopened.get(pre_existing).await.unwrap(),
        Bytes::from_static(b"pre-existing"),
        "reopened store must still see the pre-existing key (durable \
         Remove was aborted at the mirror)"
    );
}

// ============================================================================
// F-77 (#904): supports_atomic_transact honesty tests
// ============================================================================

/// **F-77 — the `supports_atomic_transact` capability flag is HONEST
/// for every backend in the read/write stack.**
///
/// `MirroredStore` overrides `transact` (so by the OLD trait contract
/// it implicitly advertised whole-batch visibility atomicity) but its
/// ephemeral subset is applied per-key to a lock-free `InMemoryStore`
/// primary — a concurrent reader can observe a partially-applied batch.
/// F-77 replaces the overpromising prose contract with a
/// machine-checkable flag. This test pins the flag's value for every
/// backend so a future change that silently flips it (or forgets to
/// delegate it through a wrapper) is caught here, not by a reader
/// observing torn state in production.
#[tokio::test]
async fn f77_supports_atomic_transact_is_honest_for_every_backend() {
    // --- MirroredStore: FALSE -----------------------------------------
    // The primary is a non-atomic InMemoryStore regardless of what the
    // mirror backend is, so the flag must be false even when the mirror
    // itself is genuinely atomic (here a second MirroredStore whose
    // mirror is an InMemoryStore — the production hybrid repo uses a
    // fjall mirror, but the flag is about the PRIMARY's publish
    // atomicity, not the mirror's).
    let mirror: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mirrored = MirroredStore::new(mirror, is_durable_table_config)
        .await
        .unwrap();
    assert!(
        !mirrored.supports_atomic_transact(),
        "MirroredStore must report false: its ephemeral subset is applied \
         per-key to a lock-free InMemoryStore primary with no multi-key \
         publish primitive, so a concurrent reader can observe a \
         partially-applied batch"
    );
    // Same answer through the `Arc<dyn Store>` erase boundary (the only
    // shape a caller actually holds in production).
    let mirrored_dyn: Arc<dyn Store> = Arc::new(mirrored);
    assert!(
        !mirrored_dyn.supports_atomic_transact(),
        "the flag must survive type erasure to Arc<dyn Store>"
    );

    // --- InMemoryStore: FALSE (inherits the trait default) ------------
    // No transact override, no multi-key primitive.
    let in_mem: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    assert!(
        !in_mem.supports_atomic_transact(),
        "InMemoryStore must report false (default): it inherits the \
         per-op transact loop with no cross-op atomicity"
    );

    // --- A genuinely-atomic mock backend: TRUE surfaces correctly -----
    // A mock that claims real atomicity (mimicking FjallStore's
    // OwnedWriteBatch) overrides the flag to true. This proves the flag
    // is not hard-wired false — a backend that DOES deliver whole-batch
    // visibility atomicity is accurately advertised as such, so callers
    // checking the flag get a trustworthy signal in both directions.
    let atomic_mock: Arc<dyn Store> = Arc::new(AtomicTransactMock {
        inner: Arc::new(InMemoryStore::new()),
    });
    assert!(
        atomic_mock.supports_atomic_transact(),
        "a backend that genuinely delivers whole-batch visibility \
         atomicity must be able to report true (proves the flag is not \
        hard-wired false and the honesty signal works both ways)"
    );

    // --- CachedStore: capability WITHDRAWN at the cache layer --------
    // RV-STORAGE-007DEF6B: the cache layer republishes a committed
    // transaction's cache entries ONE KEY AT A TIME while cache-hit reads
    // trust an unfenced mirror probe, so whole-batch visibility is NOT
    // guaranteed even when the inner backend is genuinely atomic. The
    // flag is therefore NOT re-derived from inner: CachedStore reports
    // `false` for every inner (F-77 honesty contract, cache layer).
    use crate::storage_cached::CachedStore;
    let cached_over_mem: Arc<dyn Store> = Arc::new(
        CachedStore::new_sync(Arc::new(InMemoryStore::new()))
            .await
            .unwrap(),
    );
    assert!(
        !cached_over_mem.supports_atomic_transact(),
        "CachedStore(InMemoryStore) must report false"
    );
    let cached_over_atomic: Arc<dyn Store> =
        Arc::new(CachedStore::new_sync(atomic_mock.clone()).await.unwrap());
    assert!(
        !cached_over_atomic.supports_atomic_transact(),
        "CachedStore must NOT advertise atomic transact visibility even \
         over a genuinely-atomic inner: the cache layer itself publishes \
         the batch per-key with unfenced cache-hit reads"
    );

    // --- The mirror-inside-MirroredStore's flag is IRRELEVANT --------
    // MirroredStore reports based on its OWN primary (InMemoryStore),
    // NOT on the mirror backend. Even a genuinely-atomic mirror does
    // not make MirroredStore atomic, because the ephemeral subset is
    // still published per-key to the non-atomic primary. This pins the
    // documented design decision so a future "delegate to mirror's
    // flag" regression is caught.
    let atomic_mirror: Arc<dyn Store> = Arc::new(AtomicTransactMock {
        inner: Arc::new(InMemoryStore::new()),
    });
    let mirrored_over_atomic = MirroredStore::new(atomic_mirror, is_durable_table_config)
        .await
        .unwrap();
    assert!(
        !mirrored_over_atomic.supports_atomic_transact(),
        "MirroredStore must report false even when its mirror IS \
         genuinely atomic — the flag reflects the PRIMARY's publish \
         atomicity, and the ephemeral subset is still per-op on the \
         InMemoryStore primary"
    );
}

/// Test-only `Store` that stands in for a genuinely-atomic backend
/// (like `FjallStore`'s `OwnedWriteBatch`): it overrides
/// `supports_atomic_transact` to `true` and delegates everything else
/// to an inner `InMemoryStore`. Used only by the F-77 honesty test to
/// prove the flag can surface `true` and is not hard-wired `false`.
struct AtomicTransactMock {
    inner: Arc<dyn Store>,
}

#[async_trait]
impl Store for AtomicTransactMock {
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
        self.inner.transact(ops).await
    }
    fn supports_atomic_transact(&self) -> bool {
        true
    }
    fn iter_stream(&self, batch_size: usize) -> RecordStream {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> RecordStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}
