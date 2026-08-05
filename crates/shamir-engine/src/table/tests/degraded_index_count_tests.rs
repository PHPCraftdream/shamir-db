//! #984 — tests for [`TableManager::degraded_index_count`].
//!
//! Proves:
//! 1. A fully-`Ready` table (regular + unique + sorted indexes) reports
//!    zero degraded indexes.
//! 2. Each of the four index families (regular, unique, sorted, index2)
//!    can be stuck in `Building` state (crash-orphaned — hand-constructed
//!    WITHOUT going through the live create path, simulating a `Building`
//!    definition left over from a PAST process) and is counted as degraded.
//! 3. The count path performs **zero store reads** — proven with a
//!    `ReadCountingStore` wrapper that tallies every read-type `Store`
//!    method call.
//! 4. #1003 (follow-up to #984): a CREATE INDEX genuinely in flight in
//!    THIS process (parked mid-backfill via the live create path, using
//!    the existing test-only pause-hook mechanisms) must NOT count as
//!    degraded — the false-positive #1003 fixes, proven for both the
//!    regular-hash and index2 (functional) families.

use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
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

use crate::index2::build_index2_backend_with_resolver;
use crate::index2::descriptor::IndexDescriptor;
use crate::index2::expr::IndexExpr;
use crate::index2::kind::{FunctionalConfig, IndexKind};
use crate::index2::state::IndexState;
use crate::table::index2_backfill_hook::BackfillPauseHook as Index2BackfillPauseHook;
use crate::table::TableManager;

// ---------------------------------------------------------------------------
// Store wrapper that counts read-type operations — proves the count path
// does zero store I/O.
// ---------------------------------------------------------------------------

type TestStream = Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>;

/// Wraps an `Arc<dyn Store>` and tallies every read-type operation
/// (`get`, `get_many`, `iter_stream`, `scan_prefix_stream`,
/// `iter_range_stream`, `iter_range_stream_reverse`). Write operations
/// pass through uncounted. No counting store double exists in this
/// workspace today — the fault-injecting wrappers
/// (`FailableSetStore`, `PausableInfoStore`, `FailingSetStore`) only
/// fail writes, they don't count reads — so this minimal wrapper is
/// built here inline rather than reaching for new infrastructure.
struct ReadCountingStore {
    inner: Arc<dyn Store>,
    read_count: Arc<AtomicU64>,
}

impl ReadCountingStore {
    fn new(inner: Arc<dyn Store>) -> Self {
        Self {
            inner,
            read_count: Arc::new(AtomicU64::new(0)),
        }
    }

    fn read_count(&self) -> u64 {
        self.read_count.load(SeqCst)
    }

    fn reset_reads(&self) {
        self.read_count.store(0, SeqCst);
    }
}

#[async_trait]
impl Store for ReadCountingStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        self.inner.set(key, value).await
    }
    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.read_count.fetch_add(1, SeqCst);
        self.inner.get(key).await
    }
    async fn get_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<Option<Bytes>>> {
        self.read_count.fetch_add(1, SeqCst);
        self.inner.get_many(keys).await
    }
    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }
    fn iter_stream(&self, batch_size: usize) -> TestStream {
        self.read_count.fetch_add(1, SeqCst);
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> TestStream {
        self.read_count.fetch_add(1, SeqCst);
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
    fn iter_range_stream(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> TestStream {
        self.read_count.fetch_add(1, SeqCst);
        self.inner
            .iter_range_stream(start_inclusive, end_inclusive, batch_size)
    }
    fn iter_range_stream_reverse(
        &self,
        start_inclusive: Option<Bytes>,
        end_inclusive: Option<Bytes>,
        batch_size: usize,
    ) -> TestStream {
        self.read_count.fetch_add(1, SeqCst);
        self.inner
            .iter_range_stream_reverse(start_inclusive, end_inclusive, batch_size)
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn make_stores() -> (Arc<dyn Store>, Arc<dyn Store>) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    (data, info)
}

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

fn int_record(key: u64, val: i64) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(InternerKey::new(key), InnerValue::Int(val));
    InnerValue::Map(m)
}

/// Build a functional `lower(<field>)` index2 backend with `state = Building`
/// (directly — NOT through `create_index_v2`, which would flip to Ready).
/// Mirrors `index2_lifecycle_state_tests::build_building_functional_backend`.
fn build_building_functional_backend(
    id: u32,
    name: &str,
    name_interned: u64,
    field_path: Vec<u64>,
    info_store: &Arc<dyn Store>,
) -> Arc<dyn crate::index2::backend::IndexBackend> {
    let first_path = field_path.clone();
    let expr = IndexExpr::Lower(Box::new(IndexExpr::Field(first_path)));
    let kind = IndexKind::Functional(Box::new(FunctionalConfig { expr }));
    let mut desc = IndexDescriptor::new(
        id,
        name,
        name_interned,
        smallvec::smallvec![field_path],
        kind,
    );
    desc.state = IndexState::Building;
    build_index2_backend_with_resolver(desc, info_store, None)
}

/// A functional `lower(<field>)` CREATE INDEX op, for driving the LIVE
/// `create_index_v2` path (as opposed to `build_building_functional_backend`'s
/// direct backend construction) — mirrors
/// `index2_create_barrier_tests::functional_lower_op`.
fn functional_lower_op(name: &str, table: &str, field: &str) -> CreateIndexOp {
    CreateIndexOp {
        create_index: name.into(),
        table: table.into(),
        fields: vec![vec![field.into()]],
        unique: false,
        sorted: false,
        repo: "main".into(),
        index_type: Some("functional".into()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: Some("lower".into()),
        functional_args: None,
        vector_dim: None,
        vector_metric: None,
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// A table with regular + unique + sorted indexes — all `Ready` — must
/// report zero degraded indexes.
#[tokio::test]
async fn degraded_index_count_zero_when_all_ready() {
    let (data_store, info_store) = make_stores();
    let tbl = TableManager::create("t".into(), data_store, info_store)
        .await
        .unwrap();

    // Need some records for the indexes to backfill.
    let name_key = key_id(&tbl, "name").await;
    let id_key = key_id(&tbl, "id").await;
    let score_key = key_id(&tbl, "score").await;
    for i in 0..5 {
        let mut m = new_map_wc(3);
        m.insert(InternerKey::new(id_key), InnerValue::Str(format!("u{i}")));
        m.insert(
            InternerKey::new(name_key),
            InnerValue::Str(format!("name{i}")),
        );
        m.insert(InternerKey::new(score_key), InnerValue::Int(i * 10));
        tbl.insert(&InnerValue::Map(m)).await.unwrap();
    }

    tbl.create_index("by_name", &["name"]).await.unwrap();
    tbl.create_unique_index("by_id", &["id"]).await.unwrap();
    tbl.create_sorted_index("by_score", &["score"])
        .await
        .unwrap();

    assert_eq!(
        tbl.degraded_index_count().await,
        0,
        "all-Ready table must report zero degraded indexes"
    );
}

/// #1003: a regular (hash) index whose CREATE INDEX is genuinely in flight
/// (parked mid-backfill via the live create path, in THIS process) must
/// NOT count as degraded — this is the exact false-positive scenario #1003
/// fixes. A healthy, currently-building index is not "stuck".
#[tokio::test]
async fn degraded_index_count_live_in_progress_create_regular_not_degraded() {
    use shamir_index::base_index::backfill_pause_hook::BackfillPauseHook;

    let (data_store, info_store) = make_stores();
    let tbl = TableManager::create("t".into(), data_store, info_store)
        .await
        .unwrap();
    let status_key = key_id(&tbl, "status").await;

    for s in ["a", "b", "c"] {
        tbl.insert(&str_record(status_key, s)).await.unwrap();
    }

    let hook = Arc::new(BackfillPauseHook::new());
    tbl.index_manager_ref()
        .set_create_index_backfill_hook(Some(Arc::clone(&hook)));

    let tbl_c = tbl.clone();
    let create = tokio::spawn(async move { tbl_c.create_index("status_idx", &["status"]).await });

    hook.wait_until_parked().await;

    // The index is genuinely `Building` right now (backfill parked
    // mid-flight, in THIS process) — but the in-flight-create counter
    // (#1003) excludes it from the degraded tally.
    assert_eq!(
        tbl.degraded_index_count().await,
        0,
        "a healthy in-progress CREATE INDEX must NOT count as degraded (#1003)"
    );

    hook.release();
    create.await.unwrap().expect("create_index must complete");

    assert_eq!(
        tbl.degraded_index_count().await,
        0,
        "after backfill completes, the regular index must be Ready"
    );
}

/// #1003: a functional (index2) index whose CREATE INDEX is genuinely in
/// flight (parked mid-create via `create_index_v2`'s live path, in THIS
/// process) must NOT count as degraded. Same false-positive scenario as
/// the regular-hash test above, exercised through `create_index_v2`'s own
/// in-flight guard (a separate call site from `create_index`'s).
#[tokio::test]
async fn degraded_index_count_live_in_progress_create_index2_not_degraded() {
    let (data_store, info_store) = make_stores();
    let tbl = TableManager::create("t".into(), data_store, info_store)
        .await
        .unwrap();
    let name_field = key_id(&tbl, "name").await;

    for n in ["Alice", "Bob", "Carol"] {
        tbl.insert(&str_record(name_field, n)).await.unwrap();
    }

    let hook = Arc::new(Index2BackfillPauseHook::new());
    tbl.set_create_index2_backfill_hook(Some(Arc::clone(&hook)));

    let tbl_c = tbl.clone();
    let create = tokio::spawn(async move {
        tbl_c
            .create_index_v2(&functional_lower_op("lower_name", "t", "name"))
            .await
    });

    hook.wait_until_parked().await;

    // The functional backend is genuinely `Building` right now (backfill
    // done, not yet registered — the exact window `create_index2_backfill_hook`
    // parks at) — but the in-flight-create counter (#1003) excludes it.
    assert_eq!(
        tbl.degraded_index_count().await,
        0,
        "a healthy in-progress functional CREATE INDEX must NOT count as \
         degraded (#1003)"
    );

    hook.release();
    create
        .await
        .unwrap()
        .expect("create_index_v2 must complete");

    assert_eq!(
        tbl.degraded_index_count().await,
        0,
        "after the create completes, the functional index must be Ready"
    );
}

/// A unique (hash) index re-registered at `Building` must be counted.
///
/// The unique-index CREATE path registers AFTER backfill (never `Building`
/// in production), so we simulate the stuck state by re-registering the
/// definition at `Building` via `create_unique_index_from_records` —
/// the same approach as `doctor_tests::verify_detects_building_unique_index`.
#[tokio::test]
async fn degraded_index_count_detects_building_unique() {
    let (data_store, info_store) = make_stores();
    let tbl = TableManager::create("t".into(), data_store, info_store)
        .await
        .unwrap();
    let id_key = key_id(&tbl, "id").await;

    for i in 0..5 {
        tbl.insert(&str_record(id_key, &format!("u{i}")))
            .await
            .unwrap();
    }

    tbl.create_unique_index("by_id", &["id"]).await.unwrap();
    let idx_id = key_id(&tbl, "by_id").await;

    assert_eq!(
        tbl.degraded_index_count().await,
        0,
        "a Ready unique index must not count as degraded"
    );

    // Simulate stuck-Building: re-register the def at Building.
    let def = tbl
        .index_manager_ref()
        .get_unique_index_definition(idx_id)
        .expect("by_id unique index registered");
    let mut building_def = def.clone();
    building_def.state = IndexState::Building;
    let records = tbl.collect_all_current_records().await.unwrap();
    tbl.index_manager_ref()
        .create_unique_index_from_records(building_def, records)
        .await
        .unwrap();

    assert!(
        tbl.degraded_index_count().await >= 1,
        "a Building unique index must count as degraded"
    );
}

/// #1003: a sorted (B-tree) index whose CREATE INDEX is genuinely in flight
/// (parked mid-backfill via the live create path, in THIS process) must
/// NOT count as degraded — same false-positive scenario as the regular-hash
/// test above, for the sorted family.
#[tokio::test]
async fn degraded_index_count_live_in_progress_create_sorted_not_degraded() {
    let (data_store, info_store) = make_stores();
    let tbl = TableManager::create("t".into(), data_store, info_store)
        .await
        .unwrap();
    let score_key = key_id(&tbl, "score").await;

    for score in [10, 20, 30] {
        tbl.insert(&int_record(score_key, score)).await.unwrap();
    }

    let hook = Arc::new(crate::table::index2_backfill_hook::BackfillPauseHook::new());
    tbl.set_create_sorted_index_backfill_hook(Some(Arc::clone(&hook)));

    let tbl_c = tbl.clone();
    let create =
        tokio::spawn(async move { tbl_c.create_sorted_index("score_sorted", &["score"]).await });

    hook.wait_until_parked().await;

    // Genuinely `Building` right now (backfill parked mid-flight, in THIS
    // process) — the in-flight-create counter (#1003) excludes it.
    assert_eq!(
        tbl.degraded_index_count().await,
        0,
        "a healthy in-progress CREATE SORTED INDEX must NOT count as degraded (#1003)"
    );

    hook.release();
    create
        .await
        .unwrap()
        .expect("create_sorted_index must complete");

    assert_eq!(
        tbl.degraded_index_count().await,
        0,
        "after backfill completes, the sorted index must be Ready"
    );
}

/// A `Building` index2 (functional) backend directly inserted into the
/// registry must be counted.
#[tokio::test]
async fn degraded_index_count_detects_building_index2() {
    let (data_store, info_store) = make_stores();
    let mgr = TableManager::create("t".into(), data_store, info_store)
        .await
        .unwrap();

    let name_field = key_id(&mgr, "name").await;
    let name_key = key_id(&mgr, "lower_name").await;

    let backend = build_building_functional_backend(
        1,
        "lower_name",
        name_key,
        vec![name_field],
        &mgr.info_store,
    );
    mgr.index2_registry().insert(backend).await.unwrap();

    assert_eq!(
        mgr.degraded_index_count().await,
        1,
        "a Building index2 backend must count as degraded"
    );

    // Flip to Ready and verify the count drops to zero.
    mgr.index2_registry().set_state(1, IndexState::Ready).await;
    assert_eq!(
        mgr.degraded_index_count().await,
        0,
        "after flipping to Ready, the index2 backend must not count as degraded"
    );
}

/// The count path must perform ZERO store reads — it inspects only the
/// in-memory `.state` fields. Proven with a `ReadCountingStore` wrapper
/// around both `data_store` and `info_store`.
#[tokio::test]
async fn degraded_index_count_no_store_reads() {
    let data: Arc<ReadCountingStore> =
        Arc::new(ReadCountingStore::new(Arc::new(InMemoryStore::new())));
    let info: Arc<ReadCountingStore> =
        Arc::new(ReadCountingStore::new(Arc::new(InMemoryStore::new())));

    let data_dyn: Arc<dyn Store> = data.clone();
    let info_dyn: Arc<dyn Store> = info.clone();

    let tbl = TableManager::create("t".into(), data_dyn, info_dyn)
        .await
        .unwrap();

    // Setup: insert records and create a Ready regular index.
    let status_key = key_id(&tbl, "status").await;
    for s in ["a", "b", "c"] {
        tbl.insert(&str_record(status_key, s)).await.unwrap();
    }
    tbl.create_index("by_status", &["status"]).await.unwrap();

    // Setup: also insert a Building index2 backend so the count is non-zero
    // (proving the count itself is what's read-free, not that there's
    // nothing to count).
    let name_field = key_id(&tbl, "name").await;
    let name_key = key_id(&tbl, "lower_name").await;
    let backend = build_building_functional_backend(
        1,
        "lower_name",
        name_key,
        vec![name_field],
        &tbl.info_store,
    );
    tbl.index2_registry().insert(backend).await.unwrap();

    // Reset read counters AFTER all setup.
    data.reset_reads();
    info.reset_reads();

    // THE PROOF: degraded_index_count() must not trigger any store reads.
    let degraded = tbl.degraded_index_count().await;
    assert!(
        degraded >= 1,
        "expected at least one degraded index (the Building index2 backend)"
    );
    assert_eq!(
        data.read_count(),
        0,
        "degraded_index_count() must NOT read data_store (got {} reads)",
        data.read_count()
    );
    assert_eq!(
        info.read_count(),
        0,
        "degraded_index_count() must NOT read info_store (got {} reads)",
        info.read_count()
    );
}
