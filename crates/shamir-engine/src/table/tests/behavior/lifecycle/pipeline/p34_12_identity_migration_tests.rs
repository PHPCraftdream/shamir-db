//! Identity-migration acceptance tests (P34-12 S2): injected Store failures,
//! actual WAL recovery integration, and engine-level unique-index regressions.

use rustc_hash::FxHasher;
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::{InMemoryRepo, InMemoryStore};
use shamir_storage::types::{KvOp, RecordKey, Store};
use shamir_tx::IsolationLevel;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use shamir_wal::{WalDurability, WalEntryV2, WalOpV2};

use crate::index::index_definition::IndexDefinition;
use crate::index::index_info::IndexInfo;
use crate::index::index_info_item::IndexInfoItem;
use crate::index::index_record_key::IndexRecordKey;
use crate::index2::persistence::{load_index_identity_version, INDEX_IDENTITY_VERSION};
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;
use crate::table::TableManager;
use shamir_query_builder::{select, Query};

const K: u64 = 0xf1357aea2e62a9c5;
type TestStream = Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>;

fn make_repo() -> RepoInstance {
    RepoInstance::new(
        "test".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        Vec::new(),
    )
}

async fn key_id(table: &TableManager, name: &str) -> u64 {
    let interner = table.interner().get().await.unwrap();
    match interner.touch_ind(name).unwrap() {
        TouchInd::Exists(key) | TouchInd::New(key) => key.id(),
    }
}

fn record_with_ints(k1: u64, v1: i64, k2: u64, v2: i64) -> InnerValue {
    let mut map = new_map_wc(2);
    map.insert(InternerKey::new(k1), InnerValue::Int(v1));
    map.insert(InternerKey::new(k2), InnerValue::Int(v2));
    InnerValue::Map(map)
}

async fn add_unique_int_index(table: &TableManager) -> (u64, u64, u64) {
    let a = key_id(table, "a").await;
    let b = key_id(table, "b").await;
    let idx = key_id(table, "uq_ab").await;
    table
        .index_manager_ref()
        .create_unique_index(IndexDefinition::new(
            idx,
            vec![IndexInfoItem::new(vec![a]), IndexInfoItem::new(vec![b])],
        ))
        .await
        .unwrap();
    (a, b, idx)
}

#[tokio::test]
async fn rv_index_9e472725_engine_insert_many_no_false_duplicate() {
    let repo = make_repo();
    repo.add_table_for_test(TableConfig::new("t"));
    let table = repo.get_table("t").await.unwrap();
    let (a, b, idx) = add_unique_int_index(&table).await;
    let witness_b = (0u64.wrapping_sub(K.wrapping_mul(K))) as i64;
    let rows = [
        record_with_ints(a, 0, b, 0),
        record_with_ints(a, 1, b, witness_b),
    ];
    let ids = table
        .insert_many(&rows)
        .await
        .expect("both distinct tuples must be admitted");
    assert_eq!(ids.len(), 2);
    assert_eq!(
        table
            .index_manager_ref()
            .lookup_by_unique_index(idx, &[InnerValue::Int(0), InnerValue::Int(0)])
            .await
            .unwrap(),
        Some(ids[0])
    );
    assert_eq!(
        table
            .index_manager_ref()
            .lookup_by_unique_index(idx, &[InnerValue::Int(1), InnerValue::Int(witness_b)])
            .await
            .unwrap(),
        Some(ids[1])
    );
}

#[tokio::test]
async fn rv_index_9e472725_engine_tx_insert_no_false_duplicate() {
    let repo = make_repo();
    repo.add_table_for_test(TableConfig::new("t"));
    let table = repo.get_table("t").await.unwrap();
    let (a, b, idx) = add_unique_int_index(&table).await;
    let witness_b = (0u64.wrapping_sub(K.wrapping_mul(K))) as i64;
    let row_a = record_with_ints(a, 0, b, 0);
    let row_b = record_with_ints(a, 1, b, witness_b);
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let id_a = table.insert_tx(&row_a, Some(&mut tx)).await.unwrap();
    let id_b = table.insert_tx(&row_b, Some(&mut tx)).await.unwrap();
    repo.commit_tx(tx).await.expect("commit must succeed");
    assert_eq!(
        table
            .index_manager_ref()
            .lookup_by_unique_index(idx, &[InnerValue::Int(0), InnerValue::Int(0)])
            .await
            .unwrap(),
        Some(id_a)
    );
    assert_eq!(
        table
            .index_manager_ref()
            .lookup_by_unique_index(idx, &[InnerValue::Int(1), InnerValue::Int(witness_b)])
            .await
            .unwrap(),
        Some(id_b)
    );
}

#[test]
fn rv_index_9e472725_pinned_fxhash_stream_collision() {
    let old_stream = |values: [i64; 2]| {
        let mut hasher = FxHasher::default();
        9100u64.hash(&mut hasher);
        for value in values {
            2u8.hash(&mut hasher);
            value.hash(&mut hasher);
        }
        hasher.finish()
    };
    assert_eq!(
        old_stream([0, 0]),
        old_stream([1, (0u64.wrapping_sub(K.wrapping_mul(K))) as i64])
    );
}

#[derive(Clone, Copy)]
enum FailurePoint {
    AfterStep1,
    DuringStep2,
    BeforeStep3,
}

struct InjectedMigrationFailureStore {
    inner: Arc<dyn Store>,
    point: FailurePoint,
    fired: AtomicUsize,
}
impl InjectedMigrationFailureStore {
    fn new(inner: Arc<dyn Store>, point: FailurePoint) -> Self {
        Self {
            inner,
            point,
            fired: AtomicUsize::new(0),
        }
    }
    fn fire_once(&self) -> bool {
        self.fired.fetch_add(1, Ordering::SeqCst) == 0
    }
}

#[async_trait]
impl Store for InjectedMigrationFailureStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        if matches!(self.point, FailurePoint::BeforeStep3)
            && key.as_ref() == RecordId::system("_m.idx.idv").to_bytes().as_ref()
            && self.fire_once()
        {
            return Err(DbError::Storage("injected idv write failure".into()));
        }
        self.inner.set(key, value).await
    }
    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }
    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }
    async fn remove_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<bool>> {
        let v2_sweep = keys
            .iter()
            .any(|key| matches!(key.first(), Some(0x02 | 0x03)));
        if v2_sweep && matches!(self.point, FailurePoint::DuringStep2) {
            let result = self.inner.remove_many(keys).await?;
            if self.fire_once() {
                return Err(DbError::Storage("injected failure during step 2".into()));
            }
            return Ok(result);
        }
        self.inner.remove_many(keys).await
    }
    async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
        let clears_markers = matches!(self.point, FailurePoint::AfterStep1)
            && ops.iter().any(|op| match op {
                KvOp::Remove(key) => {
                    key.as_ref() == RecordId::system("_m.idx.lfv").to_bytes().as_ref()
                        || key.as_ref() == RecordId::system("_m.idx.idv").to_bytes().as_ref()
                }
                KvOp::Set(_, _) => false,
            });
        if clears_markers && self.fire_once() {
            self.inner.transact(ops).await?;
            return Err(DbError::Storage("injected failure after step 1".into()));
        }
        self.inner.transact(ops).await
    }
    fn iter_stream(&self, batch_size: usize) -> TestStream {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> TestStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
    fn iter_range_stream(
        &self,
        start: Option<Bytes>,
        end: Option<Bytes>,
        size: usize,
    ) -> TestStream {
        self.inner.iter_range_stream(start, end, size)
    }
    fn iter_range_stream_reverse(
        &self,
        start: Option<Bytes>,
        end: Option<Bytes>,
        size: usize,
    ) -> TestStream {
        self.inner.iter_range_stream_reverse(start, end, size)
    }
}

// Literal v3 hash tail of the composite `Int(7), Int(-11)` key, copied from the
// pre-v2 `rv_index_605eec5d_golden_regular_unique_keys` vector.
const LEGACY_V3_HASH: [u8; 16] = [
    0xe8, 0xef, 0x9d, 0xc2, 0x57, 0xa3, 0xdd, 0x8f, 0x8c, 0xe6, 0xf8, 0xb4, 0xe8, 0x67, 0x4d, 0xd4,
];

pub(super) fn legacy_v3_key(unique: bool, name: u64) -> Vec<u8> {
    let mut key = IndexRecordKey::new(unique, name)
        .to_legacy_prefix_bytes()
        .to_vec();
    key.extend_from_slice(&LEGACY_V3_HASH);
    key
}

pub(super) async fn count_prefix(store: &Arc<dyn Store>, prefix: Bytes) -> usize {
    let mut stream = store.scan_prefix_stream(prefix, 128);
    let mut count = 0;
    while let Some(batch) = stream.next().await {
        count += batch.unwrap().len();
    }
    count
}

async fn persisted_definition(store: &Arc<dyn Store>, key: &str, name: u64) -> bool {
    match store.get(RecordId::system(key).to_bytes().into()).await {
        Ok(bytes) => IndexInfo::decode_bytes(&bytes).unwrap().contains(name),
        Err(DbError::NotFound(_)) => false,
        Err(error) => panic!("unexpected definition read failure: {error}"),
    }
}

#[derive(Clone, Copy)]
struct IndexIds {
    regular: u64,
    unique: u64,
}

/// What a crash can leave behind: the markers, the persisted definitions and
/// the postings of both index families in both namespaces.
#[derive(Debug, PartialEq, Eq)]
struct DurableState {
    idv: Option<u32>,
    lfv_present: bool,
    regular_definition: bool,
    unique_definition: bool,
    regular_v2_postings: usize,
    unique_v2_postings: usize,
    legacy_postings: usize,
}

async fn durable_state(store: &Arc<dyn Store>, ids: IndexIds) -> DurableState {
    DurableState {
        idv: load_index_identity_version(store).await.unwrap(),
        lfv_present: shamir_index::persistence::legacy_index_version_is_present(store)
            .await
            .unwrap(),
        regular_definition: persisted_definition(store, "indexes", ids.regular).await,
        unique_definition: persisted_definition(store, "indexes_unique", ids.unique).await,
        regular_v2_postings: count_prefix(
            store,
            IndexRecordKey::new(false, ids.regular).to_prefix_bytes(),
        )
        .await,
        unique_v2_postings: count_prefix(
            store,
            IndexRecordKey::new(true, ids.unique).to_prefix_bytes(),
        )
        .await,
        legacy_postings: count_prefix(
            store,
            IndexRecordKey::new(false, ids.regular).to_legacy_prefix_bytes(),
        )
        .await
            + count_prefix(
                store,
                IndexRecordKey::new(true, ids.unique).to_legacy_prefix_bytes(),
            )
            .await,
    }
}

/// A table written by a new binary, then touched by an old one: `lfv = 3` and
/// one literal v3 posting per family beside the v2 postings.
async fn setup_failure_case(
    point: FailurePoint,
) -> (
    Arc<dyn Store>,
    Arc<dyn Store>,
    Arc<InjectedMigrationFailureStore>,
    IndexIds,
) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let table = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&inner))
        .await
        .unwrap();
    let (a, b, unique) = add_unique_int_index(&table).await;
    let regular = key_id(&table, "ix_a").await;
    table
        .index_manager_ref()
        .create_index(IndexDefinition::new(
            regular,
            vec![IndexInfoItem::new(vec![a])],
        ))
        .await
        .unwrap();
    let rid = table.insert(&record_with_ints(a, 13, b, 14)).await.unwrap();
    table.flush_metadata().await.unwrap();
    inner
        .set(
            RecordId::system("_m.idx.lfv").to_bytes().into(),
            Bytes::from(3u32.to_le_bytes().to_vec()),
        )
        .await
        .unwrap();
    let mut legacy_regular = legacy_v3_key(false, regular);
    legacy_regular.extend_from_slice(rid.as_bytes());
    inner
        .set(Bytes::from(legacy_regular).into(), Bytes::new())
        .await
        .unwrap();
    inner
        .set(
            Bytes::from(legacy_v3_key(true, unique)).into(),
            Bytes::copy_from_slice(rid.as_bytes()),
        )
        .await
        .unwrap();
    let wrapper = Arc::new(InjectedMigrationFailureStore::new(inner, point));
    let info: Arc<dyn Store> = wrapper.clone();
    (data, info, wrapper, IndexIds { regular, unique })
}

async fn injected_failure_reopen_converges(point: FailurePoint, after_failure: DurableState) {
    let (data, info, wrapper, ids) = setup_failure_case(point).await;
    assert_eq!(
        durable_state(&info, ids).await,
        DurableState {
            idv: Some(INDEX_IDENTITY_VERSION),
            lfv_present: true,
            regular_definition: true,
            unique_definition: true,
            regular_v2_postings: 1,
            unique_v2_postings: 1,
            legacy_postings: 2,
        },
        "precondition: an old binary touched a v2 table"
    );
    assert!(
        TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
            .await
            .is_err(),
        "the injected store failure must abort this open"
    );
    assert!(
        wrapper.fired.load(Ordering::SeqCst) > 0,
        "configured failure point must fire"
    );
    // The commit point (`idv = 2`) was never reached and both definitions
    // survive; the postings show how far step 2 got.
    assert_eq!(durable_state(&info, ids).await, after_failure);

    let recovered = TableManager::create("t".into(), data, Arc::clone(&info))
        .await
        .unwrap();
    assert_eq!(
        durable_state(&info, ids).await,
        DurableState {
            idv: Some(INDEX_IDENTITY_VERSION),
            lfv_present: false,
            regular_definition: true,
            unique_definition: true,
            regular_v2_postings: 1,
            unique_v2_postings: 1,
            legacy_postings: 0,
        },
        "the reopen must converge to a clean identity-v2 table"
    );
    assert!(recovered
        .index_manager_ref()
        .lookup_by_unique_index(ids.unique, &[InnerValue::Int(13), InnerValue::Int(14)])
        .await
        .unwrap()
        .is_some());
    let regular_hits = recovered
        .index_manager_ref()
        .lookup_by_index(ids.regular, &[InnerValue::Int(13)])
        .await
        .unwrap()
        .expect("regular index must be usable after the migration");
    assert_eq!(regular_hits.len(), 1);
}

#[tokio::test]
async fn p34_12_crash_after_step_1_keeps_definitions_and_postings() {
    injected_failure_reopen_converges(
        FailurePoint::AfterStep1,
        DurableState {
            idv: None,
            lfv_present: false,
            regular_definition: true,
            unique_definition: true,
            regular_v2_postings: 1,
            unique_v2_postings: 1,
            legacy_postings: 2,
        },
    )
    .await;
}

#[tokio::test]
async fn p34_12_crash_during_step_2_keeps_definitions_after_partial_sweep() {
    // The regular index is swept first: its v2 postings are gone, nothing
    // was rebuilt yet, and the unique index and legacy postings are untouched.
    injected_failure_reopen_converges(
        FailurePoint::DuringStep2,
        DurableState {
            idv: None,
            lfv_present: false,
            regular_definition: true,
            unique_definition: true,
            regular_v2_postings: 0,
            unique_v2_postings: 1,
            legacy_postings: 2,
        },
    )
    .await;
}

#[tokio::test]
async fn p34_12_crash_before_step_3_has_rebuilt_postings_but_no_commit_marker() {
    injected_failure_reopen_converges(
        FailurePoint::BeforeStep3,
        DurableState {
            idv: None,
            lfv_present: false,
            regular_definition: true,
            unique_definition: true,
            regular_v2_postings: 1,
            unique_v2_postings: 1,
            legacy_postings: 0,
        },
    )
    .await;
}

async fn create_with_idv_bytes(bytes: &[u8]) -> (Arc<dyn Store>, Result<TableManager, DbError>) {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();
    info.set(
        RecordId::system("_m.idx.idv").to_bytes().into(),
        Bytes::copy_from_slice(bytes),
    )
    .await
    .unwrap();
    let opened = TableManager::create("t".into(), data, Arc::clone(&info)).await;
    (info, opened)
}

#[tokio::test]
async fn p34_12_future_identity_version_fails_closed() {
    let future = (INDEX_IDENTITY_VERSION + 1).to_le_bytes();
    let (info, opened) = create_with_idv_bytes(&future).await;
    let Err(error) = opened else {
        panic!("a table stamped by a newer binary must not open");
    };
    assert!(
        matches!(&error, DbError::Internal(message) if message.contains("newer than this binary")),
        "unexpected error: {error}"
    );
    // Fail closed means no marker was rewritten or removed.
    assert_eq!(
        &info
            .get(RecordId::system("_m.idx.idv").to_bytes().into())
            .await
            .unwrap()[..],
        future.as_slice()
    );
}

#[tokio::test]
async fn p34_12_malformed_identity_marker_fails_with_codec_error() {
    for malformed in [&[2u8, 0, 0][..], &[2u8, 0, 0, 0, 0][..]] {
        let (info, opened) = create_with_idv_bytes(malformed).await;
        let Err(error) = opened else {
            panic!("a malformed idv marker must not open: {malformed:?}");
        };
        assert!(
            matches!(&error, DbError::Codec(message) if message.contains("expected exactly 4")),
            "unexpected error for {malformed:?}: {error}"
        );
        assert_eq!(
            &info
                .get(RecordId::system("_m.idx.idv").to_bytes().into())
                .await
                .unwrap()[..],
            malformed
        );
    }
}

#[tokio::test]
async fn p34_12_engine_count_fast_path_matches_scan() {
    let repo = make_repo();
    repo.add_table_for_test(TableConfig::new("t"));
    let table = repo.get_table("t").await.unwrap();
    let a = key_id(&table, "a").await;
    let b = key_id(&table, "b").await;
    let idx = key_id(&table, "ix_a").await;
    table
        .index_manager_ref()
        .create_index(IndexDefinition::new(idx, vec![IndexInfoItem::new(vec![a])]))
        .await
        .unwrap();
    let long = "v".repeat(300);
    for (av, bv) in [
        ("short-a", "short-b"),
        ("other-a", "other-b"),
        (long.as_str(), "long-b"),
    ] {
        let mut map = new_map_wc(2);
        map.insert(InternerKey::new(a), InnerValue::Str(av.to_owned()));
        map.insert(InternerKey::new(b), InnerValue::Str(bv.to_owned()));
        table.insert(&InnerValue::Map(map)).await.unwrap();
    }
    let query = |field: &str, value: &str| {
        Query::from("t")
            .select([select::count_all("count")])
            .where_eq(field, value)
            .build()
    };
    for (indexed_field, indexed_value, scan_field, scan_value, expected) in [
        ("a", "short-a", "b", "short-b", 1),
        ("a", "other-a", "b", "other-b", 1),
        ("a", long.as_str(), "b", "long-b", 1),
    ] {
        let interner = table.interner().get().await.unwrap();
        let refs = new_map_wc(0);
        let ctx = crate::query::filter::FilterContext::new(interner, &refs);
        let indexed_result = table
            .read(&query(indexed_field, indexed_value), &ctx)
            .await
            .unwrap();
        let scan_result = table
            .read(&query(scan_field, scan_value), &ctx)
            .await
            .unwrap();
        assert_eq!(
            indexed_result.records[0].get_value_i64("count"),
            Some(expected)
        );
        assert_eq!(
            scan_result.records[0].get_value_i64("count"),
            Some(expected)
        );
        assert!(
            indexed_result
                .stats
                .as_ref()
                .is_some_and(|stats| stats.index_used.is_some()),
            "the indexed predicate must take the index count fast path"
        );
        assert!(
            scan_result
                .stats
                .as_ref()
                .is_none_or(|stats| stats.index_used.is_none()),
            "the unindexed predicate must be answered by a scan"
        );
    }
    assert!(table
        .index_manager_ref()
        .iter_indexes_ready()
        .any(|d| d.name_interned == idx));
}

#[tokio::test]
async fn p34_12_recover_inflight_v2_replays_pre_upgrade_index_put_and_reruns_migration() {
    let repo = make_repo();
    repo.add_table_for_test(TableConfig::new("t"));
    let table = repo.get_table("t").await.unwrap();
    let (a, b, idx) = add_unique_int_index(&table).await;
    let regular = key_id(&table, "ix_a").await;
    table
        .index_manager_ref()
        .create_index(IndexDefinition::new(
            regular,
            vec![IndexInfoItem::new(vec![a])],
        ))
        .await
        .unwrap();
    let rid = table.insert(&record_with_ints(a, 7, b, -11)).await.unwrap();
    table.flush_metadata().await.unwrap();
    // The reopen below rebuilds from the base data store: drain the committed
    // overlay first, otherwise the evicted table takes the record with it.
    repo.flush_buffers().await.unwrap();
    let info = table.info_store_for_test();
    info.set(
        RecordId::system("_m.idx.lfv").to_bytes().into(),
        Bytes::from(3u32.to_le_bytes().to_vec()),
    )
    .await
    .unwrap();
    info.remove(RecordId::system("_m.idx.idv").to_bytes().into())
        .await
        .unwrap();
    let mut legacy_regular = legacy_v3_key(false, regular);
    legacy_regular.extend_from_slice(rid.as_bytes());
    info.set(Bytes::from(legacy_regular).into(), Bytes::new())
        .await
        .unwrap();
    // Evict the open table but keep its stores: the reopen below is a real
    // open over the old-binary state, so it needs the configuration back.
    repo.remove_table_for_test("t");
    repo.add_table_for_test(TableConfig::new("t"));
    let migrated = repo.get_table("t").await.unwrap();
    assert!(migrated.identity_migrated_this_open());
    let v2_key = crate::index::index_keys::build_index_key(
        true,
        idx,
        &[InnerValue::Int(7), InnerValue::Int(-11)],
    )
    .expect("v2 identity")
    .to_bytes();
    assert!(
        migrated
            .info_store_for_test()
            .remove(v2_key.clone().into())
            .await
            .unwrap(),
        "remove the exact v2 unique key before replay"
    );
    assert_eq!(
        migrated
            .index_manager_ref()
            .lookup_by_unique_index(idx, &[InnerValue::Int(7), InnerValue::Int(-11)])
            .await
            .unwrap(),
        None
    );
    let mut old_key = Vec::with_capacity(25);
    old_key.push(0x01);
    old_key.extend_from_slice(&idx.to_le_bytes());
    old_key.extend_from_slice(&[
        0xe8, 0xef, 0x9d, 0xc2, 0x57, 0xa3, 0xdd, 0x8f, 0x8c, 0xe6, 0xf8, 0xb4, 0xe8, 0x67, 0x4d,
        0xd4,
    ]);
    let wal = repo.repo_wal().await.unwrap();
    let staged = WalEntryV2::new(
        wal.fresh_txn_id(),
        crate::repo::repo_token(repo.name()),
        vec![WalOpV2::IndexDel {
            table_id_interned: crate::table::table_manager::table_token_for("t"),
            idx_id: 0,
            key: Bytes::from(old_key.clone()),
        }],
    )
    .with_commit_version(1);
    wal.begin_grouped(&staged, WalDurability::Buffered)
        .await
        .unwrap();
    let replay_entry = WalEntryV2::new(
        wal.fresh_txn_id(),
        crate::repo::repo_token(repo.name()),
        vec![WalOpV2::IndexPut {
            table_id_interned: crate::table::table_manager::table_token_for("t"),
            idx_id: 0,
            key: Bytes::from(old_key),
            value: Bytes::copy_from_slice(rid.as_bytes()),
        }],
    )
    .with_commit_version(2);
    wal.begin_grouped(&replay_entry, WalDurability::Buffered)
        .await
        .unwrap();
    assert_eq!(repo.recover_v2_inflight().await.unwrap(), 2);
    let reopened = repo.get_table("t").await.unwrap();
    assert_eq!(
        reopened
            .index_manager_ref()
            .lookup_by_unique_index(idx, &[InnerValue::Int(7), InnerValue::Int(-11)],)
            .await
            .unwrap(),
        Some(rid)
    );
    assert_eq!(
        count_prefix(
            &reopened.info_store_for_test(),
            IndexRecordKey::new(true, idx).to_legacy_prefix_bytes()
        )
        .await,
        0
    );
    assert_eq!(
        count_prefix(
            &reopened.info_store_for_test(),
            IndexRecordKey::new(false, regular).to_legacy_prefix_bytes()
        )
        .await,
        0
    );
    let regular_hits = reopened
        .index_manager_ref()
        .lookup_by_index(regular, &[InnerValue::Int(7)])
        .await
        .unwrap()
        .expect("regular index must be usable after the re-run");
    assert_eq!(&regular_hits[..], &[rid]);
}
