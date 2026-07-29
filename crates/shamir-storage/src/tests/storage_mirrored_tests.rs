#![allow(deprecated)]

use crate::error::{DbError, DbResult};
use crate::storage_in_memory::InMemoryStore;
use crate::storage_mirrored::{is_durable_table_config, MirroredStore};
use crate::tests::types_tests::collect_stream;
use crate::types::{KvOp, RecordKey, RecordStream, Store};
use async_trait::async_trait;
use bytes::Bytes;
use shamir_types::types::record_id::RecordId;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// A representative classified key — a `MetaKey::LegacyIndexes`-shaped tag
/// (`"indexes"`), matching the exact `RecordId::system` encoding.
fn classified_key() -> RecordKey {
    RecordKey::from_slice(RecordId::system("indexes").as_bytes())
}

/// A representative unclassified key — an ordinary (non-system) 16-byte
/// key, shaped like a real row id (`RecordId::new()`, non-zero
/// timestamp prefix) so it does NOT accidentally match the system-record
/// shape.
fn unclassified_key() -> RecordKey {
    RecordKey::from_slice(RecordId::new().as_bytes())
}

#[tokio::test]
async fn classified_key_set_visible_in_mirror_and_survives_hydration() {
    let mirror: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let store = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();

    let key = classified_key();
    let value = Bytes::from_static(b"classified-value");
    store.set(key.clone(), value.clone()).await.unwrap();

    // Visible via the facade.
    assert_eq!(store.get(key.clone()).await.unwrap(), value);

    // Visible in the underlying mirror directly, not just via the facade.
    assert_eq!(mirror.get(key.clone()).await.unwrap(), value);

    // A FRESH MirroredStore over the SAME mirror hydrates the value back.
    let reopened = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();
    assert_eq!(reopened.get(key).await.unwrap(), value);
}

#[tokio::test]
async fn unclassified_key_set_absent_from_mirror_and_gone_after_hydration() {
    let mirror: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let store = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();

    let key = unclassified_key();
    let value = Bytes::from_static(b"ephemeral-value");
    store.set(key.clone(), value.clone()).await.unwrap();

    // Visible via the facade (primary always gets the write).
    assert_eq!(store.get(key.clone()).await.unwrap(), value);

    // Absent from the underlying mirror directly.
    assert!(mirror.get(key.clone()).await.is_err());

    // A fresh MirroredStore over the same mirror does NOT see it —
    // it never persisted, so hydration cannot bring it back.
    let reopened = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();
    assert!(reopened.get(key).await.is_err());
}

#[tokio::test]
async fn remove_of_classified_key_removes_from_mirror_too() {
    let mirror: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let store = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();

    let key = classified_key();
    store
        .set(key.clone(), Bytes::from_static(b"v"))
        .await
        .unwrap();
    assert!(mirror.get(key.clone()).await.is_ok());

    let removed = store.remove(key.clone()).await.unwrap();
    assert!(removed);

    // Gone from the facade...
    assert!(store.get(key.clone()).await.is_err());
    // ...and gone from the mirror, so a dropped index definition can't
    // resurrect on the next hydration.
    assert!(mirror.get(key.clone()).await.is_err());

    let reopened = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();
    assert!(reopened.get(key).await.is_err());
}

#[tokio::test]
async fn scan_prefix_stream_after_hydration_returns_interner_chunks() {
    // Mirrors the interner_manager.rs usage: chunks live at
    // `RecordId::system("i.d" + 9-digit zero-padded index)`, scanned
    // via `scan_prefix_stream("\0\0\0\0i.d", ...)`.
    let mirror: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let store = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();

    let chunk0 = RecordKey::from_slice(RecordId::system("i.d000000000").as_bytes());
    let chunk1 = RecordKey::from_slice(RecordId::system("i.d000000001").as_bytes());
    store
        .set(chunk0.clone(), Bytes::from_static(b"chunk-0"))
        .await
        .unwrap();
    store
        .set(chunk1.clone(), Bytes::from_static(b"chunk-1"))
        .await
        .unwrap();

    // Sanity: both chunks are classified as durable and landed in mirror.
    assert!(mirror.get(chunk0.clone()).await.is_ok());
    assert!(mirror.get(chunk1.clone()).await.is_ok());

    let reopened = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();

    let mut prefix = vec![0u8, 0, 0, 0];
    prefix.extend_from_slice(b"i.d");
    let results = collect_stream(reopened.scan_prefix_stream(Bytes::from(prefix), 16))
        .await
        .unwrap();

    assert_eq!(results.len(), 2);
    assert!(results
        .iter()
        .any(|(k, v)| *k == chunk0 && v.as_ref() == b"chunk-0"));
    assert!(results
        .iter()
        .any(|(k, v)| *k == chunk1 && v.as_ref() == b"chunk-1"));
}

#[tokio::test]
async fn insert_never_writes_to_mirror() {
    // `InMemoryStore::insert` mints its key from `RecordId::new()`
    // (non-zero timestamp prefix) — it can never match the
    // `[0,0,0,0]`-prefixed system-record shape the classifier requires,
    // so `insert` must never cause a mirror write.
    let mirror: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let store = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();

    let key = store.insert(Bytes::from_static(b"row")).await.unwrap();
    assert!(!is_durable_table_config(&key));
    assert!(mirror.get(key.clone()).await.is_err());
    assert_eq!(store.get(key).await.unwrap(), Bytes::from_static(b"row"));
}

#[tokio::test]
async fn batch_apis_split_classified_and_unclassified_correctly() {
    // Exercises the trait's DEFAULT `set_many`/`remove_many`/`transact`
    // loops (MirroredStore does not override them) through a mixed
    // batch, proving the per-item classify-and-mirror behavior these
    // defaults inherit from `set`/`remove` composes correctly across a
    // batch call, not just single calls (tests 1-2 only exercise the
    // latter).
    let mirror: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let store = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();

    let classified = classified_key();
    let unclassified = unclassified_key();

    let items = vec![
        (classified.clone(), Bytes::from_static(b"cfg")),
        (unclassified.clone(), Bytes::from_static(b"row")),
    ];
    let flags = store.set_many(items).await.unwrap();
    assert_eq!(flags, vec![true, true]);

    assert!(mirror.get(classified.clone()).await.is_ok());
    assert!(mirror.get(unclassified.clone()).await.is_err());

    let remove_flags = store
        .remove_many(vec![classified.clone(), unclassified.clone()])
        .await
        .unwrap();
    assert_eq!(remove_flags, vec![true, true]);
    assert!(mirror.get(classified).await.is_err());
}

/// Mirrors EVERY `MetaKey` variant's `(name, tag, expected_durable)` from
/// `crates/shamir-engine/src/meta/namespace.rs` as of this test's
/// writing.
///
/// `shamir-storage` cannot import `shamir-engine`'s real `MetaKey` enum
/// here — `shamir-engine` depends on `shamir-storage`, so the reverse
/// import would be a crate dependency cycle. This hand-copied list is
/// therefore the closest exhaustiveness guard achievable from inside
/// `shamir-storage` alone.
///
/// **MAINTENANCE CONTRACT:** any time a `MetaKey` variant is added,
/// removed, or renamed in `namespace.rs`, this list MUST be updated in
/// the same change — that is what makes this guard meaningful. Cross-
/// checked variant-by-variant against `namespace.rs` at the time this
/// test was written (14 variants): `Indexes` ("_m.idx"), `Tables`
/// ("_m.tbl"), `Wal` ("_m.wal"), `Migrations` ("_m.mig"), `Internals`
/// ("internals"), `Count` ("count"), `BufferConfig` ("buffer_config"),
/// `SortedIndexes` ("sorted_indexes"), `LegacyIndexes` ("indexes"),
/// `LegacyIndexesUnique` ("indexes_unique"), `LastCommittedVersion`
/// ("_t.lcv"), `NextTxId` ("_t.nti"), `Validators` ("_m.val"),
/// `ReplicationBookmark` ("_t.rbm").
const ALL_META_KEY_TAGS: &[(&str, bool)] = &[
    ("_m.idx", true),         // MetaKey::Indexes
    ("_m.tbl", true),         // MetaKey::Tables
    ("_m.wal", true),         // MetaKey::Wal
    ("_m.mig", true),         // MetaKey::Migrations
    ("internals", true),      // MetaKey::Internals
    ("count", false),         // MetaKey::Count — derived from row data
    ("buffer_config", true),  // MetaKey::BufferConfig
    ("sorted_indexes", true), // MetaKey::SortedIndexes
    ("indexes", true),        // MetaKey::LegacyIndexes
    ("indexes_unique", true), // MetaKey::LegacyIndexesUnique
    // Derived recovery/replication bookkeeping tied to the committed
    // transaction history — same "derived from data, not
    // configuration" hazard as `Count`. See `is_durable_table_config`'s
    // doc for the full rationale.
    ("_t.lcv", false), // MetaKey::LastCommittedVersion
    ("_t.nti", false), // MetaKey::NextTxId
    ("_m.val", true),  // MetaKey::Validators
    ("_t.rbm", false), // MetaKey::ReplicationBookmark
];

#[tokio::test]
async fn classifier_exhaustiveness_guard_against_every_meta_key() {
    // The most important test in this module: for every known
    // `MetaKey` tag (see `ALL_META_KEY_TAGS`'s doc for why this can't
    // be the live enum), assert the classifier's verdict is correct —
    // especially that `Count` and the MVCC/replication recovery
    // markers are excluded. A future `MetaKey` addition that forgets
    // to update the classifier (and this list) will not be caught
    // automatically by this test alone — the maintenance contract
    // above is the enforcement mechanism.
    for &(tag, expect_durable) in ALL_META_KEY_TAGS {
        let key = RecordKey::from_slice(RecordId::system(tag).as_bytes());
        let verdict = is_durable_table_config(&key);
        assert_eq!(
            verdict, expect_durable,
            "tag {:?} classified as {}, expected {}",
            tag, verdict, expect_durable
        );
    }

    // Representative NON-system keys must all be `false` — none of
    // them are even 16 bytes with a `[0,0,0,0]` prefix.

    // A posting key shape: 4 (index_id LE u32) + 1 (type_tag) + value
    // bytes + 16 (RecordId) — never exactly 16 bytes total for any
    // non-empty value, and even a contrived 11-byte value making the
    // total 32 bytes still fails the length check.
    let mut posting_key = vec![0u8; 4]; // index_id = 0 (LE)
    posting_key.push(0); // type_tag
    posting_key.extend_from_slice(b"value"); // value bytes
    posting_key.extend_from_slice(RecordId::new().as_bytes()); // 16-byte rid
    assert!(!is_durable_table_config(&RecordKey::from_slice(
        &posting_key
    )));

    // A sorted-index / vector-snapshot key shape: ASCII string keys
    // like "<keyspace>.g0.data.000000" — never 16 bytes, never a
    // `[0,0,0,0]` prefix.
    let sorted_index_key = RecordKey::from_slice(b"my_index.g0.data.000000");
    assert!(!is_durable_table_config(&sorted_index_key));

    // `_m.idx.lfv` — the legacy posting-format version stamp
    // (`shamir_index::persistence`, ~line 44/94-123) — is NOT a `MetaKey`
    // variant (it's an ad-hoc `RecordId::system(...)` call directly in
    // `shamir-index`, outside `namespace.rs`), so `ALL_META_KEY_TAGS`
    // above doesn't cover it. Asserted separately: static config (marks
    // the on-disk posting format current), not derived from row data —
    // must be durable, same as the other index-definition tags.
    let lfv_key = RecordKey::from_slice(RecordId::system("_m.idx.lfv").as_bytes());
    assert!(
        is_durable_table_config(&lfv_key),
        "_m.idx.lfv (legacy posting-format version stamp) must be durable"
    );

    let vector_snapshot_key = RecordKey::from_slice(b"vec_idx.g3.sidecar");
    assert!(!is_durable_table_config(&vector_snapshot_key));

    // A plain ordinary row id (RecordId::new(), non-zero timestamp
    // prefix) must also be false — same length as a system key (16
    // bytes) but fails the `[0,0,0,0]` prefix check.
    let row_key = RecordKey::from_slice(RecordId::new().as_bytes());
    assert!(!is_durable_table_config(&row_key));
}

// ============================================================================
// F-39: MirroredStore::transact atomicity tests
// ============================================================================

/// Test-only mirror wrapper whose `transact` can be configured to fail
/// atomically — when `fail_transact` is set, it returns `Err` WITHOUT
/// applying ANY ops to the inner store, simulating what a genuinely
/// atomic backend (like `FjallStore`'s `OwnedWriteBatch`) does on a
/// batch-commit failure. All other `Store` methods delegate to `inner`
/// unchanged.
///
/// Proves that `MirroredStore::transact` delegates the ENTIRE durable
/// subset to ONE `mirror.transact` call (not per-op): when the mirror's
/// transact fails, NO durable op is partially committed.
struct FailingTransactMirror {
    inner: Arc<dyn Store>,
    /// When `true`, `transact` returns `Err` without delegating to inner.
    fail_transact: AtomicBool,
}

#[async_trait]
impl Store for FailingTransactMirror {
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
        if self.fail_transact.load(Ordering::Acquire) {
            return Err(DbError::Internal(
                "injected transact failure (FailingTransactMirror)".into(),
            ));
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

/// A second classified key — `MetaKey::Tables`-shaped tag (`"_m.tbl"`),
/// distinct from [`classified_key`]'s `"indexes"` tag so two durable ops
/// in one batch touch two different keys.
fn classified_key_2() -> RecordKey {
    RecordKey::from_slice(RecordId::system("_m.tbl").as_bytes())
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

/// Test-only mirror wrapper whose `set` / `remove` can be configured to
/// fail — when `fail_writes` is set, both return `Err` WITHOUT delegating
/// to the inner store. All other `Store` methods (including `transact`)
/// delegate to `inner` unchanged. Sibling of F-39's
/// `FailingTransactMirror` (which only fails `transact`); this one fails
/// `set`/`remove` specifically, to prove F-41's mirror-first ordering
/// leaves `primary` untouched on a mirror write failure.
struct FailingSetRemoveMirror {
    inner: Arc<dyn Store>,
    /// When `true`, `set` / `remove` return `Err` without delegating.
    fail_writes: AtomicBool,
}

#[async_trait]
impl Store for FailingSetRemoveMirror {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        if self.fail_writes.load(Ordering::Acquire) {
            return Err(DbError::Internal(
                "injected set failure (FailingSetRemoveMirror)".into(),
            ));
        }
        self.inner.set(key, value).await
    }
    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }
    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        if self.fail_writes.load(Ordering::Acquire) {
            return Err(DbError::Internal(
                "injected remove failure (FailingSetRemoveMirror)".into(),
            ));
        }
        self.inner.remove(key).await
    }
    async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
        self.inner.transact(ops).await
    }
    fn iter_stream(&self, batch_size: usize) -> RecordStream {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> RecordStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

/// **Test 1 — mirror-first ordering, happy path (no regression).**
///
/// After F-41's mirror-commit-before-primary-publish reordering, a
/// classified `set`/`remove` must STILL land in both `primary` and
/// `mirror` exactly as before — the reordering only changes WHICH store
/// is written first, not whether both are written on success. Exercises
/// both `set` and `remove` in one place (the pre-F-41 happy-path tests
/// above cover them separately).
#[tokio::test]
async fn f41_mirror_first_ordering_classified_set_and_remove_land_in_both() {
    let mirror: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let store = MirroredStore::new(Arc::clone(&mirror), is_durable_table_config)
        .await
        .unwrap();

    let key = classified_key();
    assert!(is_durable_table_config(&key));

    // set: both primary (via facade get) and mirror reflect the write.
    let created = store
        .set(key.clone(), Bytes::from_static(b"v1"))
        .await
        .unwrap();
    assert!(
        created,
        "fresh classified insert must report created=true (InMemoryStore::set semantics)"
    );
    assert_eq!(
        store.get(key.clone()).await.unwrap(),
        Bytes::from_static(b"v1")
    );
    assert_eq!(
        mirror.get(key.clone()).await.unwrap(),
        Bytes::from_static(b"v1")
    );

    // remove: both primary and mirror drop the key.
    let existed = store.remove(key.clone()).await.unwrap();
    assert!(
        existed,
        "remove of a present classified key must report existed=true (InMemoryStore::remove semantics)"
    );
    assert!(store.get(key.clone()).await.is_err());
    assert!(mirror.get(key.clone()).await.is_err());
}

/// **Test 2a — mirror `set` failure leaves `primary` untouched.**
///
/// The core F-41 proof. With a mirror that errors on `set`, a classified
/// `set` must (a) return `Err` to the caller AND (b) leave `primary`
/// holding its PRE-write value (here `"old"`, not the attempted `"new"`)
/// — "API says failed" and "live state" now agree, unlike before F-41
/// (where primary was mutated first and the live process behaved as if
/// the write had succeeded even as the caller saw `Err`).
#[tokio::test]
async fn f41_mirror_set_failure_leaves_primary_untouched() {
    let mirror_inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let failing_mirror = Arc::new(FailingSetRemoveMirror {
        inner: mirror_inner.clone(),
        fail_writes: AtomicBool::new(false),
    });
    let mirror_dyn: Arc<dyn Store> = failing_mirror.clone();

    let store = MirroredStore::new(mirror_dyn, is_durable_table_config)
        .await
        .unwrap();

    let key = classified_key();
    assert!(is_durable_table_config(&key));

    // Seed an OLD value into BOTH primary and mirror (writes succeed:
    // the flag is off, so the wrapper delegates to inner).
    store
        .set(key.clone(), Bytes::from_static(b"old"))
        .await
        .unwrap();

    // Flip the mirror to fail writes, then attempt an overwrite.
    failing_mirror.fail_writes.store(true, Ordering::Release);
    let result = store.set(key.clone(), Bytes::from_static(b"new")).await;
    assert!(
        result.is_err(),
        "classified set against a failing mirror must return Err"
    );

    // Primary must STILL hold the OLD value — the overwrite aborted at
    // the mirror BEFORE primary was touched. This is the core proof.
    assert_eq!(
        store.get(key.clone()).await.unwrap(),
        Bytes::from_static(b"old"),
        "primary must hold the pre-write value after a mirror set failure"
    );
    // And the mirror's own inner store is of course unchanged too.
    assert_eq!(
        mirror_inner.get(key).await.unwrap(),
        Bytes::from_static(b"old"),
        "mirror inner must be untouched after a failed set"
    );
}

/// **Test 2b — mirror `remove` failure leaves `primary` untouched.**
///
/// Symmetric to test 2a for `remove`: a classified `remove` against a
/// failing mirror returns `Err` and leaves `primary` (and the key's
/// presence there) unchanged.
#[tokio::test]
async fn f41_mirror_remove_failure_leaves_primary_untouched() {
    let mirror_inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let failing_mirror = Arc::new(FailingSetRemoveMirror {
        inner: mirror_inner.clone(),
        fail_writes: AtomicBool::new(false),
    });
    let mirror_dyn: Arc<dyn Store> = failing_mirror.clone();

    let store = MirroredStore::new(mirror_dyn, is_durable_table_config)
        .await
        .unwrap();

    let key = classified_key();
    // Seed the key into BOTH primary and mirror first (writes succeed:
    // flag off), so the subsequent `remove` has something to remove —
    // proving the failure aborts BEFORE the primary removal.
    store
        .set(key.clone(), Bytes::from_static(b"seeded"))
        .await
        .unwrap();
    assert_eq!(
        store.get(key.clone()).await.unwrap(),
        Bytes::from_static(b"seeded")
    );
    assert_eq!(
        mirror_inner.get(key.clone()).await.unwrap(),
        Bytes::from_static(b"seeded")
    );

    // Flip the mirror to fail writes, then attempt the removal.
    failing_mirror.fail_writes.store(true, Ordering::Release);
    let result = store.remove(key.clone()).await;
    assert!(
        result.is_err(),
        "classified remove against a failing mirror must return Err"
    );

    // Primary must STILL hold the key — the removal aborted at the
    // mirror before primary was touched.
    assert_eq!(
        store.get(key.clone()).await.unwrap(),
        Bytes::from_static(b"seeded"),
        "primary must still hold the key after a mirror remove failure"
    );
    assert_eq!(
        mirror_inner.get(key).await.unwrap(),
        Bytes::from_static(b"seeded"),
        "mirror inner must be untouched after a failed remove"
    );
}

// ---------------------------------------------------------------------------
// Test-only log capture for the hydration re-filter diagnostic assertion.
// ---------------------------------------------------------------------------

/// Process-global capture buffer for every record the test-only
/// [`CapturingLogger`] emits. nextest runs each test in its OWN process,
/// so this one-shot global is safe from cross-test interference.
static CAPTURED_LOGS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static CAPTURING_LOGGER: CapturingLogger = CapturingLogger;

/// Minimal `log::Log` impl that records every emitted record's
/// `[LEVEL] args` into [`CAPTURED_LOGS`]. Lets the F-41 hydration test
/// ASSERT the diagnostic `warn!` actually fires, rather than only
/// asserting the behavioural consequence of the classify branch.
struct CapturingLogger;

impl log::Log for CapturingLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }
    fn log(&self, record: &log::Record) {
        if let Some(cell) = CAPTURED_LOGS.get() {
            cell.lock()
                .unwrap()
                .push(format!("[{}] {}", record.level(), record.args()));
        }
    }
    fn flush(&self) {}
}

/// Install the capturing logger (idempotent within a process) and return
/// a handle to the captured-messages cell. Callers `.lock().unwrap()`
/// `.clear()` before the code under test so only its emissions are kept.
fn install_capturing_logger() -> &'static Mutex<Vec<String>> {
    let cell = CAPTURED_LOGS.get_or_init(|| Mutex::new(Vec::new()));
    // `set_logger` is process-global and one-shot. nextest's
    // process-per-test means the first call in this test's own process
    // wins; a repeat call in the same process is a no-op we ignore.
    let _ = log::set_logger(&CAPTURING_LOGGER);
    log::set_max_level(log::LevelFilter::Warn);
    cell
}

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
