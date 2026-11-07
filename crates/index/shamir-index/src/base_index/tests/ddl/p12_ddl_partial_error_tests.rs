//! P1-2 (#967): DDL partial-state error enrichment tests.
//!
//! When a multi-phase DDL operation (CREATE / DROP / RENAME INDEX) fails
//! AFTER a durable marker (Building registration, drop tombstone, rename
//! tombstone) has already been persisted, the returned error must carry
//! enough context for the caller to understand WHAT partial state was
//! persisted and HOW to resolve it.
//!
//! These tests use a `FaultyStore` wrapper that can be armed to fail on a
//! specific Store method (`set_many`, or the Nth `set` call) to force the
//! later-phase failure and assert the enriched error text.

use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_info::IndexInfo;
use crate::base_index::index_info_item::IndexInfoItem;
use crate::base_index::index_keys::{build_index_key_from_record, build_posting_key};
use crate::base_index::index_manager::IndexManager;
use crate::base_index::index_record_key::IndexRecordKey;
use crate::base_index::sorted_index_definition::SortedIndexDefinition;
use crate::base_index::sorted_index_manager::SortedIndexManager;
use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{RecordKey, Store};
use shamir_tunables::store_defaults::{FULL_SCAN_BATCH, MAINT_SCAN_BATCH};
use shamir_types::core::interner::InternerKey;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

// Same underlying type as `shamir_storage::types::RecordStream` (which is
// `pub(crate)`). Defining our own alias works because Rust type aliases are
// transparent — both resolve to the identical `Pin<Box<dyn Stream<…> + Send>>`.
type TestStream = Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>;

/// `Store` wrapper that can be armed to deterministically fail on specific
/// operations. Mirrors the `FailableSetStore` pattern from
/// `shamir-engine`'s `index_create_persist_atomicity_tests.rs`.
struct FaultyStore {
    inner: Arc<dyn Store>,
    /// When `true`, the next `set_many` call returns an error.
    fail_set_many: AtomicBool,
    /// Fail the Nth `set` call (1-indexed). 0 = disabled.
    fail_set_on_nth: AtomicU64,
    set_count: AtomicU64,
    /// When `true`, the next `remove_many` call returns an error.
    fail_remove_many: AtomicBool,
    remove_batches: Mutex<Vec<usize>>,
    fail_remove_on_nth: AtomicU64,
    remove_count: AtomicU64,
}

impl FaultyStore {
    fn new(inner: Arc<dyn Store>) -> Self {
        Self {
            inner,
            fail_set_many: AtomicBool::new(false),
            fail_set_on_nth: AtomicU64::new(0),
            set_count: AtomicU64::new(0),
            fail_remove_many: AtomicBool::new(false),
            remove_batches: Mutex::new(Vec::new()),
            fail_remove_on_nth: AtomicU64::new(0),
            remove_count: AtomicU64::new(0),
        }
    }
}

#[async_trait]
impl Store for FaultyStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        let n = self.fail_set_on_nth.load(Ordering::SeqCst);
        if n > 0 {
            let count = self.set_count.fetch_add(1, Ordering::SeqCst) + 1;
            if count == n {
                return Err(DbError::Storage("injected set failure".to_string()));
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
    async fn set_many(&self, items: Vec<(RecordKey, Bytes)>) -> DbResult<Vec<bool>> {
        if self.fail_set_many.load(Ordering::SeqCst) {
            return Err(DbError::Storage("injected set_many failure".to_string()));
        }
        self.inner.set_many(items).await
    }
    async fn remove_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<bool>> {
        self.remove_batches.lock().unwrap().push(keys.len());
        let count = self.remove_count.fetch_add(1, Ordering::SeqCst) + 1;
        if self.fail_remove_many.load(Ordering::SeqCst)
            || count == self.fail_remove_on_nth.load(Ordering::SeqCst)
        {
            return Err(DbError::Storage("injected remove_many failure".to_string()));
        }
        self.inner.remove_many(keys).await
    }
}

// ============================================================================
// Helpers
// ============================================================================

#[tokio::test]
async fn paged_drop_sweeps_preserve_neighbors() {
    let faulty = Arc::new(FaultyStore::new(Arc::new(InMemoryStore::new())));
    let store: Arc<dyn Store> = faulty.clone();
    let regular = IndexManager::new(Arc::new(InMemoryStore::new()), store.clone())
        .await
        .unwrap();
    regular
        .create_index(IndexDefinition::new(41, vec![IndexInfoItem::new(vec![1])]))
        .await
        .unwrap();
    let sorted = SortedIndexManager::new(store.clone()).await.unwrap();
    sorted
        .register(SortedIndexDefinition::new(42, vec![1]))
        .await
        .unwrap();
    for (prefix, count) in [
        (
            IndexRecordKey::new(false, 41).to_prefix_bytes(),
            FULL_SCAN_BATCH + 1,
        ),
        (
            Bytes::copy_from_slice(&shamir_types::core::sorted_index_layout::sorted_prefix(42)),
            MAINT_SCAN_BATCH + 1,
        ),
    ] {
        for n in 0..count {
            let mut key = prefix.to_vec();
            key.extend_from_slice(&(n as u64).to_be_bytes());
            store
                .set(Bytes::from(key).into(), Bytes::new())
                .await
                .unwrap();
        }
    }
    let neighbor = Bytes::from_static(b"unrelated-neighbor");
    store
        .set(neighbor.clone().into(), Bytes::from_static(b"keep"))
        .await
        .unwrap();
    regular.drop_index(41, None, None).await.unwrap();
    let regular_batches = std::mem::take(&mut *faulty.remove_batches.lock().unwrap());
    assert!(regular_batches.len() >= 2);
    assert!(regular_batches.iter().all(|n| *n <= FULL_SCAN_BATCH));
    sorted.drop_index(42, None, None).await.unwrap();
    let sorted_batches = std::mem::take(&mut *faulty.remove_batches.lock().unwrap());
    assert!(sorted_batches.len() >= 2);
    assert!(sorted_batches.iter().all(|n| *n <= MAINT_SCAN_BATCH));
    assert_eq!(
        store.get(neighbor.into()).await.unwrap(),
        Bytes::from_static(b"keep")
    );
    assert_eq!(regular.entry_count(41, false).await.unwrap(), 0);
    let sorted_prefix =
        Bytes::copy_from_slice(&shamir_types::core::sorted_index_layout::sorted_prefix(42));
    assert!(store
        .scan_prefix_stream(sorted_prefix, MAINT_SCAN_BATCH)
        .next()
        .await
        .is_none());
}

#[tokio::test]
async fn drop_failure_after_first_page_recovers_without_touching_neighbor() {
    let faulty = Arc::new(FaultyStore::new(Arc::new(InMemoryStore::new())));
    let store: Arc<dyn Store> = faulty.clone();
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let manager = IndexManager::new(data.clone(), store.clone())
        .await
        .unwrap();
    manager
        .create_index(IndexDefinition::new(41, vec![IndexInfoItem::new(vec![1])]))
        .await
        .unwrap();
    let prefix = IndexRecordKey::new(false, 41).to_prefix_bytes();
    let mut keys = Vec::new();
    for n in 0..(FULL_SCAN_BATCH + 1) {
        let mut key = prefix.to_vec();
        key.extend_from_slice(&(n as u64).to_be_bytes());
        let key = Bytes::from(key);
        store.set(key.clone().into(), Bytes::new()).await.unwrap();
        keys.push(key);
    }
    let neighbor = Bytes::from_static(b"unrelated-neighbor");
    store
        .set(neighbor.clone().into(), Bytes::from_static(b"keep"))
        .await
        .unwrap();
    faulty.remove_count.store(0, Ordering::SeqCst);
    faulty.fail_remove_on_nth.store(2, Ordering::SeqCst);
    assert!(manager.drop_index(41, None, None).await.is_err());
    assert_eq!(faulty.remove_count.load(Ordering::SeqCst), 2);
    assert!(store.get(keys[0].clone().into()).await.is_err());
    assert!(store
        .get(keys[FULL_SCAN_BATCH].clone().into())
        .await
        .is_ok());
    let tombstones = IndexManager::load_dropping_set_standalone(false, &store)
        .await
        .unwrap();
    assert!(tombstones.iter().any(|(name, _)| *name == 41));
    drop(manager);
    faulty.fail_remove_on_nth.store(0, Ordering::SeqCst);
    let recovered = IndexManager::new(data, store.clone()).await.unwrap();
    assert!(store
        .scan_prefix_stream(prefix, FULL_SCAN_BATCH)
        .next()
        .await
        .is_none());
    assert_eq!(
        store.get(neighbor.into()).await.unwrap(),
        Bytes::from_static(b"keep")
    );
    assert!(IndexManager::load_dropping_set_standalone(false, &store)
        .await
        .unwrap()
        .is_empty());
    drop(recovered);
}

fn make_value(field_key: u64, field_val: &str) -> InnerValue {
    let mut map = new_map();
    map.insert(
        InternerKey::new(field_key),
        InnerValue::Str(field_val.to_string()),
    );
    InnerValue::Map(map)
}

// ============================================================================
// Tests — CREATE INDEX (regular hash)
// ============================================================================

/// Phase 2 backfill (`set_many`) fails AFTER Phase 1 (`save_index_info`)
/// durably persisted the `Building` definition. The returned error must
/// explain the partial state and how to resolve it.
#[tokio::test]
async fn p12_create_index_backfill_failure_enriched() {
    let faulty = Arc::new(FaultyStore::new(
        Arc::new(InMemoryStore::new()) as Arc<dyn Store>
    ));
    let info_store: Arc<dyn Store> = Arc::clone(&faulty) as Arc<dyn Store>;
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

    let mgr = IndexManager::new(data_store, info_store).await.unwrap();

    let name_interned = 42u64;
    let field_key = 1u64;
    let def = IndexDefinition::new(name_interned, vec![IndexInfoItem::new(vec![field_key])]);

    // Arm: fail the backfill posting write (Phase 2 `set_many`).
    faulty.fail_set_many.store(true, Ordering::SeqCst);

    let records = vec![
        (
            shamir_types::types::record_id::RecordId::new(),
            make_value(field_key, "hello"),
        ),
        (
            shamir_types::types::record_id::RecordId::new(),
            make_value(field_key, "world"),
        ),
    ];

    let result = mgr.create_index_from_records(def, records).await;
    assert!(result.is_err(), "should fail");
    let msg = result.unwrap_err().to_string();

    // 1. Partial state was persisted
    assert!(
        msg.contains("durably registered as Building"),
        "error should state Building was persisted, got: {msg}"
    );
    // 2. Current actual state
    assert!(
        msg.contains("NOT queryable"),
        "error should state index is NOT queryable, got: {msg}"
    );
    // 3. Resolution hint
    assert!(
        msg.contains("TableManager::verify"),
        "error should suggest verify/repair, got: {msg}"
    );
}

/// Phase 3 Ready-flip persist (`save_index_info`, the 2nd `set` call) fails
/// AFTER Phase 1 + Phase 2 succeeded. The error must explain the state split:
/// Ready in memory but Building on disk.
#[tokio::test]
async fn p12_create_index_phase3_persist_failure_enriched() {
    let faulty = Arc::new(FaultyStore::new(
        Arc::new(InMemoryStore::new()) as Arc<dyn Store>
    ));
    let info_store: Arc<dyn Store> = Arc::clone(&faulty) as Arc<dyn Store>;
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

    let mgr = IndexManager::new(data_store, info_store).await.unwrap();

    let name_interned = 42u64;
    let field_key = 1u64;
    let def = IndexDefinition::new(name_interned, vec![IndexInfoItem::new(vec![field_key])]);

    // Arm: fail the 2nd `set` call.
    // Call #1 = Phase 1 `save_index_info` (Building persist) — succeeds.
    // Call #2 = Phase 3 `save_index_info` (Ready persist) — fails.
    faulty.fail_set_on_nth.store(2, Ordering::SeqCst);

    let records = vec![(
        shamir_types::types::record_id::RecordId::new(),
        make_value(field_key, "hello"),
    )];

    let result = mgr.create_index_from_records(def, records).await;
    assert!(result.is_err(), "should fail");
    let msg = result.unwrap_err().to_string();

    // 1. Partial state was persisted
    assert!(
        msg.contains("flipped to Ready in memory"),
        "error should state the Ready flip happened, got: {msg}"
    );
    // 2. Current actual state
    assert!(
        msg.contains("durably Building on disk"),
        "error should state disk state is still Building, got: {msg}"
    );
    // 3. Resolution hint
    assert!(
        msg.contains("TableManager::verify"),
        "error should suggest verify/repair, got: {msg}"
    );
}

/// DROP INDEX sweep failure AFTER the durable tombstone was persisted.
/// The error must mention the tombstone and that recovery will resume.
#[tokio::test]
async fn p12_drop_index_sweep_failure_enriched() {
    let faulty = Arc::new(FaultyStore::new(
        Arc::new(InMemoryStore::new()) as Arc<dyn Store>
    ));
    let info_store: Arc<dyn Store> = Arc::clone(&faulty) as Arc<dyn Store>;
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

    // Seed a Ready index definition + postings into the store.
    let name_interned = 42u64;
    let field_key = 1u64;
    let def = IndexDefinition::new(name_interned, vec![IndexInfoItem::new(vec![field_key])]);
    let info = IndexInfo::from_definitions(vec![def]);
    let key = RecordId::system("indexes").to_bytes();
    let bytes = bincode::serialize(&info).unwrap();
    info_store.set(key.into(), bytes.into()).await.unwrap();

    // Write a posting so the sweep has something to remove.
    let val = make_value(field_key, "hello");
    let irk = build_index_key_from_record(
        false,
        name_interned,
        &val,
        &[IndexInfoItem::new(vec![field_key])],
    )
    .unwrap();
    let (irk, _canonical) = irk;
    let index_key = irk.to_bytes();
    let record_id = RecordId::new();
    let posting_key = build_posting_key(&index_key, &record_id);
    info_store
        .set(posting_key.into(), Bytes::new())
        .await
        .unwrap();

    // Load the manager from the seeded store.
    let mgr = IndexManager::new(data_store, info_store).await.unwrap();

    // Arm: fail `remove_many` (used by the posting sweep).
    faulty.fail_remove_many.store(true, Ordering::SeqCst);

    let result = mgr.drop_index(name_interned, None, None).await;
    assert!(result.is_err(), "should fail");
    let msg = result.unwrap_err().to_string();

    // 1. Partial state was persisted
    assert!(
        msg.contains("durable drop tombstone was persisted"),
        "error should state tombstone was persisted, got: {msg}"
    );
    // 2. Current actual state
    assert!(
        msg.contains("posting sweep failed"),
        "error should state the sweep failed, got: {msg}"
    );
    // 3. Resolution hint
    assert!(
        msg.contains("TableManager::verify"),
        "error should suggest verify, got: {msg}"
    );
}
