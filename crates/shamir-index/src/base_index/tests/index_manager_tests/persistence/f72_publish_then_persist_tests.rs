//! F-72 (#899, P0) — regular-hash CREATE INDEX publish-then-persist fix.
//!
//! Pre-fix, `IndexManager::create_index_from_records` persisted the index
//! metadata (`save_index_info`) exactly ONCE, right after `add_index`
//! published the definition (Phase 1) — the backfill (Phase 2) then ran with
//! NO further persist. Since a freshly-constructed `IndexDefinition` was
//! always `Ready` (pre-F-72, there was no `Building` state at all), a Phase 1
//! persist failure would `Err` out of the whole method, but by then the
//! definition WAS already live in `self.indexes` — a `Ready`-looking, fully
//! queryable in-memory index whose metadata was never durably saved. That is
//! the "publish-then-persist inversion" this task fixes.
//!
//! F-72's fix makes the flow three phases:
//!   1. register at `Building`, persist (durable `Building` marker);
//!   2. backfill postings;
//!   3. flip `Building` → `Ready` in-memory, THEN persist again.
//!
//! This test proves the SPECIFIC property the brief calls out: a simulated
//! `save_index_info` failure at the Phase 3 (post-backfill) persist call
//! must NEVER leave a `Ready`-durably-unsaved index behind. Concretely: after
//! such a failure, the ON-DISK bytes must still describe the definition as
//! `Building` (or be absent from a prior successful Phase-1 persist re-read),
//! never `Ready` — so a restart (which loads straight from disk) can never
//! observe a `Ready`, queryable index whose durable record disagrees.

use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::Arc;

use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_info::IndexInfo;
use crate::base_index::index_info_item::IndexInfoItem;
use crate::base_index::index_manager::IndexManager;
use crate::state::IndexState;
use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_types::types::record_id::RecordId;
use std::pin::Pin;

type TestStream = Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>;

/// `Store` wrapper that fails the Nth `set` call whose key matches the
/// regular-index metadata key (`RecordId::system("indexes")`), counting
/// from one. Every other key (postings, the unique-index metadata key,
/// etc.) and every OTHER call to the metadata key delegate straight
/// through to `inner`.
///
/// This lets the test fail SPECIFICALLY the Phase 3 (post-backfill)
/// `save_index_info` persist — Phase 1's persist (which must succeed, or
/// nothing gets registered at all) and any other write are unaffected.
struct FailNthIndexesSetStore {
    inner: Arc<dyn Store>,
    indexes_key: RecordKey,
    seen: AtomicUsize,
    fail_at: usize,
}

impl FailNthIndexesSetStore {
    fn new(inner: Arc<dyn Store>, fail_at: usize) -> Self {
        Self {
            inner,
            indexes_key: RecordId::system("indexes").to_bytes().into(),
            seen: AtomicUsize::new(0),
            fail_at,
        }
    }
}

#[async_trait]
impl Store for FailNthIndexesSetStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        if key == self.indexes_key {
            let n = self.seen.fetch_add(1, SeqCst) + 1;
            if n == self.fail_at {
                return Err(DbError::Storage(format!(
                    "FailNthIndexesSetStore: injected failure on write #{n} to the \
                     regular-index metadata key"
                )));
            }
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

/// Read the raw on-disk `indexes` metadata blob straight from `inner` (NOT
/// through the wrapper, and NOT through a fresh `IndexManager` — this reads
/// exactly what is durably there right now) and decode it via the same
/// forward-compat path `IndexManager::new` uses.
async fn read_persisted_indexes(inner: &Arc<dyn Store>) -> IndexInfo {
    let key: RecordKey = RecordId::system("indexes").to_bytes().into();
    let bytes = inner.get(key).await.expect("indexes key must be present");
    IndexInfo::decode_bytes(&bytes).expect("persisted bytes must decode")
}

#[tokio::test]
async fn phase3_persist_failure_leaves_definition_durably_building_not_ready() {
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let inner_info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    // Fail the SECOND write to the `indexes` key — Phase 1's persist (write
    // #1, durable `Building` marker) succeeds; Phase 3's persist (write #2,
    // the `Ready` flip) fails.
    let wrapper: Arc<dyn Store> = Arc::new(FailNthIndexesSetStore::new(Arc::clone(&inner_info), 2));

    let manager = IndexManager::new(Arc::clone(&data_store), Arc::clone(&wrapper))
        .await
        .expect("IndexManager::new must succeed against an empty store");

    let name_interned = 777u64;
    let mut index_def = IndexDefinition::new(name_interned, vec![IndexInfoItem::new(vec![1u64])]);
    // Mirrors `TableManager::create_index`'s call site: register at Building.
    index_def.state = IndexState::Building;

    // No records to backfill — the property under test is about the
    // persist ordering, not the postings themselves.
    let result = manager
        .create_index_from_records(index_def, Vec::new())
        .await;

    assert!(
        result.is_err(),
        "create_index_from_records must surface the injected Phase 3 persist \
         failure, got: {result:?}"
    );
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("FailNthIndexesSetStore"),
        "error must come from the injected Phase 3 failure, got: {err_msg}"
    );

    // THE PROOF: the durable, on-disk record (read straight from the
    // underlying store, bypassing the wrapper) must NOT describe the
    // definition as `Ready` — Phase 1's persist (which DID succeed) wrote it
    // as `Building`, and the failed Phase 3 persist never overwrote that.
    // Restart / any fresh `IndexManager::new` over this store would load
    // `Building` — permanently planner-invisible until an operator
    // reconciles it — never a `Ready`-durably-unsaved index.
    let persisted = read_persisted_indexes(&inner_info).await;
    let def = persisted
        .get_index(name_interned)
        .expect("Phase 1's persist must have durably registered the definition");
    assert_eq!(
        def.state,
        IndexState::Building,
        "F-72: a Phase 3 persist failure must leave the DURABLE record at \
         Building, never Ready — a Ready-on-disk-but-not-really-saved index \
         would resurrect as queryable (and wrongly complete) on the next \
         restart"
    );
}

/// Control: with a healthy store, the SAME sequence completes normally and
/// the durable record ends at `Ready` — confirms the wrapper/test harness
/// itself isn't the reason the fault test observes `Building`.
#[tokio::test]
async fn happy_path_flips_to_durably_ready() {
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let manager = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();

    let name_interned = 778u64;
    let mut index_def = IndexDefinition::new(name_interned, vec![IndexInfoItem::new(vec![2u64])]);
    index_def.state = IndexState::Building;

    manager
        .create_index_from_records(index_def, Vec::new())
        .await
        .expect("create_index_from_records must succeed against a healthy store");

    let persisted = read_persisted_indexes(&info_store).await;
    let def = persisted
        .get_index(name_interned)
        .expect("definition must be durably registered");
    assert_eq!(
        def.state,
        IndexState::Ready,
        "a successful create must flip the durable record to Ready"
    );
}
