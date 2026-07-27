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
use std::sync::{Arc, Mutex};

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
/// `MirroredStore::transact` is in its durable phase (after ephemeral ops
/// were applied to `primary`, before the mirror write completes).
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
        // what a concurrent reader sees at this exact point — after the
        // ephemeral loop completed, before the durable mirror write lands.
        // Clone the Arc out of the Mutex BEFORE awaiting so the guard is
        // not held across `.await` (Send requirement).
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
/// returned"), while `primary` correctly holds the ops (Phase 2 ran before
/// the Phase 3 mirror write was attempted and failed).
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

    // Primary DOES hold the ops (Phase 2 wrote them before Phase 3 failed) —
    // this is the documented "primary ahead of mirror" failure mode.
    assert_eq!(store.get(k1).await.unwrap(), Bytes::from_static(b"v1"));
    assert_eq!(store.get(k2).await.unwrap(), Bytes::from_static(b"v2"));
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

/// **Test 4 — ephemeral-succeeds-then-durable-fails ordering proof.**
///
/// Force the durable half to fail (same `FailingTransactMirror` technique
/// as test 2) while the ephemeral half would otherwise succeed. Confirm:
/// - `primary` DOES reflect the ephemeral ops (already applied in Phase 1).
/// - `primary` ALSO reflects the durable ops (applied in Phase 2, before
///   the Phase 3 mirror write failed) — the documented "primary ahead of
///   mirror" state.
/// - `mirror` does NOT reflect the durable ops (correctly rolled back to
///   nothing by the mirror backend's own atomic failure).
///
/// This is the precise failure-ordering story from the brief's section 2.
#[tokio::test]
async fn transact_ephemeral_succeeds_then_durable_fails_leaves_primary_ahead() {
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

    // Primary reflects BOTH the ephemeral and durable ops — fully applied
    // (Phase 1 + Phase 2 ran before Phase 3 failed). Primary is "ahead".
    assert_eq!(
        store.get(ephemeral.clone()).await.unwrap(),
        Bytes::from_static(b"eph"),
        "primary must reflect ephemeral ops (Phase 1)"
    );
    assert_eq!(
        store.get(durable.clone()).await.unwrap(),
        Bytes::from_static(b"dur"),
        "primary must reflect durable ops (Phase 2, before Phase 3 failed)"
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

/// **Test 5 — concurrent-reader visibility during transact (honest test).**
///
/// An `ObservingMirror` reads `primary` (via the wrapping `MirroredStore`'s
/// `get`) at the exact moment the durable phase begins — proving a
/// concurrent reader during `MirroredStore::transact` CAN observe ephemeral
/// state already applied to `primary` before the full batch completes.
///
/// This is the honest test of the concurrent-reader residual documented in
/// `MirroredStore::transact`'s doc comment. It demonstrates the window that
/// IS deterministically testable: a reader observing `primary` during the
/// durable phase sees all ephemeral ops.
///
/// It does NOT assert the finer-grained case — a reader seeing PARTIAL
/// ephemeral state (some but not all ephemeral ops) mid-ephemeral-loop.
/// That interleaving is the same inherited `InMemoryStore` characteristic
/// (lock-free `TreeIndex`, no multi-key atomicity), but it is not
/// deterministically testable here: `InMemoryStore::set` completes
/// synchronously with no yield point between ephemeral ops for a concurrent
/// reader to interleave at. Asserting it would claim a guarantee stronger
/// than what is implemented — exactly what the brief says NOT to do.
#[tokio::test]
async fn transact_concurrent_reader_can_observe_ephemeral_state_mid_batch() {
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

    // The observing mirror read `primary` during the durable phase and
    // saw the ephemeral value — proving a concurrent reader CAN observe
    // ephemeral state mid-transact (before the batch completes).
    let observed = observing.observed.lock().unwrap().clone();
    assert_eq!(
        observed,
        Some(ephemeral_val),
        "a reader during the durable phase must see the ephemeral op already \
         applied to primary — this is the concurrent-reader window"
    );

    // The durable op also landed correctly (transact completed).
    assert_eq!(
        mirror_inner.get(durable_key).await.unwrap(),
        Bytes::from_static(b"dur")
    );
}
