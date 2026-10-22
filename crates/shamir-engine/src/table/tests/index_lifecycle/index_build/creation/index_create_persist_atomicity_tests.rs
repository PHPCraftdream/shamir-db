//! F-42 (#850) — index-create persist-atomicity tests.
//!
//! `create_index` / `create_unique_index_locked` /
//! `create_sorted_index_with_include` were reordered so
//! `self.interner.persist()` runs BEFORE the new index is published
//! (registered). Pre-F-42 the order was register-then-persist: if the
//! durable persist itself failed, the DDL returned `Err` but the index
//! was already LIVE in `index_manager`/`sorted_indexes` — a live index
//! whose interner ids may not survive a restart, reopening the F-33
//! corruption class at the failure path.
//!
//! These tests prove the actual atomicity property: a persist failure
//! must leave NO live index afterward (not just "an error was
//! returned"). The fault-injection seam is a togglable `Store` wrapper
//! whose `set` returns `Err` while armed — `InternerManager::persist`
//! writes its delta chunk via `info_store.set(chunk_key, bytes)`, so
//! arming the wrapper immediately before the DDL call makes that ONE
//! persist fail. The wrapper delegates every other op (and every op
//! while disarmed) to the inner store, so `TableManager::create` (which
//! itself calls `info_store.set` for the `_m.idx.lfv` legacy-format
//! marker) succeeds normally during setup.

use crate::table::TableManager;
use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::Arc;

type TestStream = Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>;

/// `Store` wrapper whose `set` can be armed to deterministically fail.
///
/// When `armed` is `true`, `set` returns `Err` WITHOUT delegating to
/// `inner` (and without mutating `inner`); every other op (`get`,
/// `remove`, `insert`, streams, `flush`, batch defaults) delegates
/// straight through. The flag is `AtomicBool` so the test can arm it
/// between two awaited calls on the same wrapper.
///
/// Mirrors the established `FailingSetStore` pattern
/// (`interner_manager_tests.rs`, F-27a) and the `FailingTransactMirror`
/// pattern (`storage_mirrored_tests.rs`, F-39), adapted to this layer:
/// the older `FailingSetStore` fails `set` UNCONDITIONALLY, which would
/// also break the `TableManager::create` setup write
/// (`save_legacy_index_version`); this wrapper's togglable arming lets
/// setup succeed and fails only the `interner.persist()` call inside
/// the DDL we are testing.
struct FailableSetStore {
    inner: Arc<dyn Store>,
    armed: Arc<AtomicBool>,
}

impl FailableSetStore {
    fn new(inner: Arc<dyn Store>) -> Self {
        Self {
            inner,
            armed: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[async_trait]
impl Store for FailableSetStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        if self.armed.load(SeqCst) {
            return Err(DbError::Storage(format!(
                "FailableSetStore: injected set failure (key byte0={:?})",
                key.first()
            )));
        }
        self.inner.set(key, value).await
    }
    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }
    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }
    fn iter_stream(&self, batch_size: usize) -> TestStream {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> TestStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
    fn iter_range_stream(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> TestStream {
        self.inner
            .iter_range_stream(start_inclusive, end_inclusive, batch_size)
    }
    fn iter_range_stream_reverse(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> TestStream {
        self.inner
            .iter_range_stream_reverse(start_inclusive, end_inclusive, batch_size)
    }
}

/// Build a TableManager whose `info_store` is a `FailableSetStore`
/// wrapping a fresh `InMemoryStore`. Returns the table and the arming
/// flag's handle so the test can arm the failure window.
async fn setup_with_failable_store(
    table_name: &str,
) -> (
    TableManager,
    Arc<AtomicBool>,
    Arc<dyn Store>,
    Arc<FailableSetStore>,
) {
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let inner_info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let wrapper = Arc::new(FailableSetStore::new(Arc::clone(&inner_info)));
    let info_store: Arc<dyn Store> = Arc::clone(&wrapper) as Arc<dyn Store>;
    let table = TableManager::create(table_name.to_string(), data_store, info_store)
        .await
        .expect("TableManager::create must succeed while wrapper is disarmed");
    let armed = Arc::clone(&wrapper.armed);
    (table, armed, inner_info, wrapper)
}

// ============================================================================
// Fault-injection: persist failure during create_index must leave NO live
// regular index.
// ============================================================================

/// F-42 (#850) regression: with the post-fix ordering, a persist failure
/// during `create_index` returns `Err` AND the regular index is NOT
/// registered. Pre-Fix the index was registered first and the persist ran
/// last, so the same failure left a live index whose interner ids were
/// never durably saved.
#[tokio::test]
async fn create_index_persist_failure_leaves_no_live_index() {
    let (table, armed, _inner_info, _wrapper) = setup_with_failable_store("users").await;

    // Sanity: no indexes before.
    assert!(
        table.index_manager_ref().iter_indexes().next().is_none(),
        "no regular indexes should exist before create_index"
    );

    // Arm the failure window — the next info_store.set (which is the
    // interner chunk write inside `interner.persist()`) fails.
    armed.store(true, SeqCst);
    let result = table.create_index("email_idx", &["email"]).await;
    // Disarm not strictly needed (no further writes) — keep armed to
    // also prove the post-failure introspection path doesn't accidentally
    // retry the write.
    assert!(
        result.is_err(),
        "create_index must surface the injected persist failure, got: {result:?}"
    );
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("FailableSetStore"),
        "error must come from the injected failure, got: {err_msg}"
    );

    // The actual atomicity proof — NOT just "an error was returned":
    //   1. `index_exists` returns false (no live registration).
    //   2. `iter_indexes()` snapshot is empty (the index was never
    //      registered because the persist failure aborted the DDL
    //      before `create_index_from_records` ran).
    assert!(
        !table.index_exists("email_idx").await,
        "regular index must NOT be live after a persist failure (F-42)"
    );
    assert!(
        table.index_manager_ref().iter_indexes().next().is_none(),
        "index_manager snapshot must be empty after a persist failure (F-42)"
    );
}

// ============================================================================
// Fault-injection: persist failure during create_unique_index must leave
// NO live unique index.
// ============================================================================

/// F-42 (#850) regression for the unique-index path. Mirrors the regular
/// index test: a persist failure must abort before
/// `create_unique_index_from_records` registers the unique index.
#[tokio::test]
async fn create_unique_index_persist_failure_leaves_no_live_index() {
    let (table, armed, _inner_info, _wrapper) = setup_with_failable_store("users").await;

    assert!(
        table
            .index_manager_ref()
            .iter_unique_indexes()
            .next()
            .is_none(),
        "no unique indexes should exist before create_unique_index"
    );

    armed.store(true, SeqCst);
    let result = table.create_unique_index("email_uniq", &["email"]).await;
    assert!(
        result.is_err(),
        "create_unique_index must surface the injected persist failure, got: {result:?}"
    );
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("FailableSetStore"),
        "error must come from the injected failure, got: {err_msg}"
    );

    // Atomicity proof.
    assert!(
        !table.unique_index_exists("email_uniq").await,
        "unique index must NOT be live after a persist failure (F-42)"
    );
    assert!(
        table
            .index_manager_ref()
            .iter_unique_indexes()
            .next()
            .is_none(),
        "unique index_manager snapshot must be empty after a persist failure (F-42)"
    );
    // Regular index path must ALSO be untouched (a unique-index failure
    // must not half-register as a regular index).
    assert!(
        table.index_manager_ref().iter_indexes().next().is_none(),
        "regular index snapshot must remain empty after unique-index persist failure"
    );
}

// ============================================================================
// Fault-injection: persist failure during create_sorted_index_with_include
// must leave NO live sorted index.
// ============================================================================

/// F-42 (#850) regression for the sorted-index path. Same bug class as
/// the other two: the original `register`→`persist` ordering left the
/// sorted index live if the durable persist failed. Also covers the
/// covering-index case (`included_fields` is non-empty) to pin that
/// the inline `included_fields` interning added by F-42 runs BEFORE
/// the persist call, so the covering ids are persisted in the same
/// chunk as the name + field-path ids.
#[tokio::test]
async fn create_sorted_index_persist_failure_leaves_no_live_index() {
    let (table, armed, _inner_info, _wrapper) = setup_with_failable_store("users").await;

    assert!(
        table.sorted_indexes().iter_indexes().is_empty(),
        "no sorted indexes should exist before create_sorted_index"
    );

    armed.store(true, SeqCst);
    // Pass a non-empty `included_fields` so the covering-projection
    // interning path (added by F-42) is also exercised — its
    // `touch_ind` calls must run BEFORE the persist, otherwise a
    // persist failure could leave the covering ids un-persisted.
    let result = table
        .create_sorted_index_with_include("score_idx", &["score"], vec![vec!["email".to_string()]])
        .await;
    assert!(
        result.is_err(),
        "create_sorted_index_with_include must surface the injected persist failure, got: {result:?}"
    );
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("FailableSetStore"),
        "error must come from the injected failure, got: {err_msg}"
    );

    // Atomicity proof — sorted index must NOT be live.
    assert!(
        table.sorted_indexes().iter_indexes().is_empty(),
        "sorted index snapshot must be empty after a persist failure (F-42)"
    );
    // Cross-subsystem sanity: neither regular nor unique indexes should
    // have been disturbed.
    assert!(
        table.index_manager_ref().iter_indexes().next().is_none(),
        "regular index snapshot must remain empty after sorted-index persist failure"
    );
    assert!(
        table
            .index_manager_ref()
            .iter_unique_indexes()
            .next()
            .is_none(),
        "unique index snapshot must remain empty after sorted-index persist failure"
    );
}

// ============================================================================
// Happy-path regression: with the wrapper DISARMED, every DDL path
// succeeds and the index goes live + is introspectable. Re-confirms the
// reordering did not break the success path (the existing
// `table_manager_tests::test_create_index_simple` etc. cover the same
// end-to-end shape through the `DbInstance` facade; this test exercises
// the F-42-touched code paths through the same `FailableSetStore`
// fixture used by the fault tests above, so a regression in the
// reorder that only manifests under the wrapper would be caught here
// rather than only in a different test module).
// ============================================================================

#[tokio::test]
async fn happy_path_create_indexes_succeeds_when_store_healthy() {
    let (table, armed, _inner_info, _wrapper) = setup_with_failable_store("users").await;
    // Wrapper is disarmed; all three DDL paths must succeed and the
    // indexes must be introspectable immediately.
    assert!(!armed.load(SeqCst));

    table
        .create_index("email_idx", &["email"])
        .await
        .expect("create_index must succeed with a healthy store");
    assert!(
        table.index_exists("email_idx").await,
        "regular index must be live after a successful create_index"
    );

    table
        .create_unique_index("id_uniq", &["id"])
        .await
        .expect("create_unique_index must succeed with a healthy store");
    assert!(
        table.unique_index_exists("id_uniq").await,
        "unique index must be live after a successful create_unique_index"
    );

    table
        .create_sorted_index_with_include("score_idx", &["score"], vec![vec!["email".to_string()]])
        .await
        .expect("create_sorted_index_with_include must succeed with a healthy store");
    let sorted = table.sorted_indexes().iter_indexes();
    assert_eq!(
        sorted.len(),
        1,
        "sorted index must be live after a successful create_sorted_index_with_include"
    );
    // The covering-index projection cache must be populated (F-42 inlined
    // the interning of `included_fields` — verify the registered def
    // carries the projected ids, not an empty cache).
    let sorted_def = sorted
        .first()
        .expect("sorted def must be registered after successful create");
    assert!(
        sorted_def.is_covering(),
        "covering projection cache must be populated after a successful create_sorted_index_with_include"
    );
    assert!(
        !sorted_def.included_fields_interned.is_empty(),
        "included_fields_interned must be non-empty for a covering sorted index"
    );
}
