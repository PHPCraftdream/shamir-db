//! R0-D (#1013) — open-path index recovery must fail CLOSED, not open.
//!
//! Two independent readonly reviews of the #957-#1005 wave found the same
//! defect class in `TableManager::create`'s reopen path: on a genuine
//! recovery failure (a failed `drop_all` during the Building self-heal, or a
//! failed `restore_on_open`), the code logged a warning and left the backend
//! registered as if recovery had succeeded — silently serving a partial or
//! empty backend with no error signal a client can distinguish from "no
//! matches".
//!
//! These tests prove the fix: both sites now move the backend's
//! authoritative registry state to `IndexState::Failed` (planner-invisible,
//! exactly like `Building`; counted by `degraded_index_count()`) instead of
//! proceeding as if nothing went wrong.
//!
//! Fault-injection pattern: a togglable `Store` wrapper that fails exactly
//! one streaming method while armed and delegates everything else straight
//! through — mirrors the established `FailableSetStore`
//! (`index_create_persist_atomicity_tests.rs`) / `ReadCountingStore`
//! (`degraded_index_count_tests.rs`) wrappers, adapted to fail a READ-side
//! stream instead of a write.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;

use shamir_query_types::admin::types::CreateIndexOp;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

use crate::index2::state::IndexState;
use crate::table::index2_backfill_hook::BackfillPauseHook;
use crate::table::TableManager;

type TestStream = Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>;

// ---------------------------------------------------------------------------
// Fault-injection wrappers
// ---------------------------------------------------------------------------

/// `Store` wrapper whose `scan_prefix_stream` can be armed to
/// deterministically fail (returns a one-shot error stream instead of
/// delegating). Every other op — including `iter_stream`, writes, and
/// `scan_prefix_stream` while disarmed — passes straight through to
/// `inner`. Used to fail `FtsRankedBackend::drop_all` (which reads via
/// `store.scan_prefix_stream`) without touching anything else the
/// table-open path needs (interner persist, counter, etc., all use
/// `set`/`get`/`iter_stream`).
struct FailingScanStore {
    inner: Arc<dyn Store>,
    armed: Arc<AtomicBool>,
}

impl FailingScanStore {
    fn new(inner: Arc<dyn Store>) -> Self {
        Self {
            inner,
            armed: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[async_trait]
impl Store for FailingScanStore {
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
    fn iter_stream(&self, batch_size: usize) -> TestStream {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> TestStream {
        if self.armed.load(SeqCst) {
            return Box::pin(futures::stream::once(async {
                Err(DbError::Storage(
                    "FailingScanStore: injected scan_prefix_stream failure".to_string(),
                ))
            }));
        }
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

/// `Store` wrapper whose `iter_stream` can be armed to deterministically
/// fail. Used to fail `VectorBackend::rebuild` (the full-scan fallback
/// `restore_on_open` reaches when no HNSW snapshot exists), which reads via
/// `data_store.iter_stream`.
struct FailingIterStore {
    inner: Arc<dyn Store>,
    armed: Arc<AtomicBool>,
}

impl FailingIterStore {
    fn new(inner: Arc<dyn Store>) -> Self {
        Self {
            inner,
            armed: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[async_trait]
impl Store for FailingIterStore {
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
    fn iter_stream(&self, batch_size: usize) -> TestStream {
        if self.armed.load(SeqCst) {
            return Box::pin(futures::stream::once(async {
                Err(DbError::Storage(
                    "FailingIterStore: injected iter_stream failure".to_string(),
                ))
            }));
        }
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

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

async fn key_id(tbl: &TableManager, name: &str) -> u64 {
    let interner = tbl.interner().get().await.unwrap();
    match interner.touch_ind(name).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

fn str_record(key: u64, val: &str) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(InternerKey::new(key), InnerValue::Str(val.into()));
    InnerValue::Map(m)
}

fn vec_record(key: u64, vec: &[f32]) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(
        InternerKey::new(key),
        InnerValue::List(vec.iter().map(|f| InnerValue::F64(*f as f64)).collect()),
    );
    InnerValue::Map(m)
}

fn fts_index_op(name: &str, table: &str, field: &str) -> CreateIndexOp {
    CreateIndexOp {
        create_index: name.into(),
        table: table.into(),
        fields: vec![vec![field.into()]],
        unique: false,
        sorted: false,
        repo: "main".into(),
        index_type: Some("fts".into()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: None,
        functional_args: None,
        vector_dim: None,
        vector_metric: None,
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    }
}

fn vector_index_op(name: &str, table: &str, field: &str, dim: u32) -> CreateIndexOp {
    CreateIndexOp {
        create_index: name.into(),
        table: table.into(),
        fields: vec![vec![field.into()]],
        unique: false,
        sorted: false,
        repo: "main".into(),
        index_type: Some("vector".into()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: None,
        functional_args: None,
        vector_dim: Some(dim),
        vector_metric: Some("cosine".into()),
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    }
}

// ============================================================================
// Site 1 — Building self-heal: a failed `drop_all` must mark Failed, not
// proceed to backfill + Ready.
// ============================================================================

/// Simulate a crash mid-`create_index_v2` for an FTS index (Building
/// persisted on disk, same technique as
/// `index2_lifecycle_state_tests::building_index_self_heals_on_reopen`), then
/// reopen with `info_store` wrapped so the self-heal's `drop_all` call
/// (`FtsRankedBackend::drop_all` reads via `store.scan_prefix_stream`)
/// deterministically fails.
///
/// Pre-fix: the reopen loop logged a warning and proceeded to backfill
/// anyway, flipping the backend to `Ready` — reporting a "successful" reopen
/// with (for FTS specifically) potentially double-counted stats from the
/// undropped partial postings. Post-fix: the backend must be `Failed`, NOT
/// `Ready`, and the backfill/Ready-flip must be skipped entirely.
#[tokio::test]
async fn failed_drop_all_during_building_self_heal_marks_failed_not_ready() {
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let inner_info: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    // --- Phase 1: create + park mid-build on the UNWRAPPED store (Building
    // persisted on disk). Setup must not be affected by the fault injection.
    let mgr1 = TableManager::create(
        "docs".into(),
        Arc::clone(&data_store),
        Arc::clone(&inner_info),
    )
    .await
    .unwrap();
    let title_field = key_id(&mgr1, "title").await;
    mgr1.insert(&str_record(title_field, "hello world"))
        .await
        .unwrap();
    mgr1.interner().persist().await.unwrap();

    let hook = Arc::new(BackfillPauseHook::new());
    mgr1.set_create_index2_backfill_hook(Some(Arc::clone(&hook)));

    let mgr1c = mgr1.clone();
    let create = tokio::spawn(async move {
        mgr1c
            .create_index_v2(&fts_index_op("title_fts", "docs", "title"))
            .await
    });
    hook.wait_until_parked().await;
    create.abort();
    let _ = create.await;
    drop(mgr1);

    // --- Phase 2: reopen with a scan-failing info_store. `drop_all` on the
    // recovered Building FTS backend must fail.
    let failing_info = Arc::new(FailingScanStore::new(Arc::clone(&inner_info)));
    let armed = Arc::clone(&failing_info.armed);
    armed.store(true, SeqCst);
    let info_store: Arc<dyn Store> = failing_info;

    let mgr2 = TableManager::create("docs".into(), Arc::clone(&data_store), info_store)
        .await
        .unwrap();

    let backends = mgr2.index2_registry().all_backends().await;
    assert_eq!(
        backends.len(),
        1,
        "the backend must still be REGISTERED (fail closed, not vanish) — got {}",
        backends.len()
    );
    let id = backends[0].descriptor().id;
    let state = mgr2.index2_registry().state_of(id).await;
    assert_eq!(
        state,
        Some(IndexState::Failed),
        "a failed drop_all during the Building self-heal must mark the backend \
         Failed, NOT proceed to backfill + Ready (pre-fix regression: got {:?})",
        state
    );
    let reason = mgr2.index2_registry().failure_reason_of(id).await;
    assert!(
        reason.is_some(),
        "a Failed backend must carry a recorded failure reason"
    );
    assert!(
        reason.unwrap().contains("drop_all"),
        "the recorded reason must mention drop_all as the failing operation"
    );
}

// ============================================================================
// Site 2 — restore_on_open failure must mark Failed, not leave the backend
// at its freshly-constructed (empty/incomplete) state.
// ============================================================================

/// A vector index is created and made durable (Ready, with data), then the
/// table is reopened with `data_store` wrapped so the fallback full-scan
/// `rebuild` (which `restore_on_open` calls when no HNSW snapshot exists —
/// the default/first-open path for every fresh vector index) fails via
/// `iter_stream`.
///
/// Pre-fix: the reopen loop logged a warning and left the backend
/// registered — its freshly-constructed (EMPTY) HNSW adapter never got
/// populated, silently returning empty search results forever with no error
/// signal. Post-fix: the backend must be `Failed`.
#[tokio::test]
async fn failed_restore_on_open_marks_vector_backend_failed() {
    let inner_data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    // --- Phase 1: create the vector index and let it backfill normally.
    let mgr1 = TableManager::create(
        "vecs".into(),
        Arc::clone(&inner_data),
        Arc::clone(&info_store),
    )
    .await
    .unwrap();
    let emb_field = key_id(&mgr1, "embedding").await;
    mgr1.insert(&vec_record(emb_field, &[1.0, 0.0, 0.0]))
        .await
        .unwrap();
    mgr1.interner().persist().await.unwrap();
    mgr1.create_index_v2(&vector_index_op("vec_idx", "vecs", "embedding", 3))
        .await
        .unwrap();
    drop(mgr1);

    // --- Phase 2: reopen with a scan-failing data_store. No HNSW snapshot
    // was ever dumped (no snapshot threshold crossed), so `restore_on_open`
    // takes the `NotFound` branch and falls back to `rebuild(data_store)`,
    // which fails on the injected `iter_stream` error.
    let failing_data = Arc::new(FailingIterStore::new(Arc::clone(&inner_data)));
    let armed = Arc::clone(&failing_data.armed);
    armed.store(true, SeqCst);
    let data_store: Arc<dyn Store> = failing_data;

    let mgr2 = TableManager::create("vecs".into(), data_store, Arc::clone(&info_store))
        .await
        .unwrap();

    let backends = mgr2.index2_registry().all_backends().await;
    assert_eq!(backends.len(), 1);
    let id = backends[0].descriptor().id;
    let state = mgr2.index2_registry().state_of(id).await;
    assert_eq!(
        state,
        Some(IndexState::Failed),
        "a failed restore_on_open must mark the backend Failed instead of \
         leaving it registered as if recovery succeeded (pre-fix regression: \
         got {:?})",
        state
    );
    let reason = mgr2.index2_registry().failure_reason_of(id).await;
    assert!(
        reason
            .as_deref()
            .is_some_and(|r| r.contains("restore_on_open")),
        "the recorded reason must mention restore_on_open as the failing \
         operation, got {reason:?}"
    );

    // Planner-invisibility: the Failed backend must not be reachable via the
    // name-keyed lookup's Ready-gated sibling — `lease_by_field_and_kind`
    // (used by query planning) must not return it.
    let plan = mgr2
        .index2_registry()
        .lease_by_field_and_kind(&[emb_field], "vector")
        .await
        .unwrap();
    assert!(
        plan.is_none(),
        "a Failed vector backend must be planner-invisible, exactly like Building"
    );

    // degraded_index_count() must count it (it is not in-flight — this
    // process never started a CREATE for it, it only reopened).
    let degraded = mgr2.degraded_index_count().await;
    assert_eq!(
        degraded, 1,
        "a Failed backend must be counted as degraded by degraded_index_count()"
    );
}
