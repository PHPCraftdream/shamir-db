//! S3 index2 identity migration acceptance: real v1 envelope/postings, v2
//! namespaces, rebuilt lookups and exactly-once FTS stats after open migration.

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use shamir_funclib::registry::{FnEntry, ScalarResult};
use shamir_funclib::scalar_resolver::{ScalarResolver, UserScalarLayer};
use shamir_index::backend::{FtsMode, IndexQuery, IndexResult};
use shamir_index::descriptor::IndexDescriptor;
use shamir_index::expr::IndexExpr;
use shamir_index::kind::{FunctionalConfig, IndexKind, TokenizerKind};
use shamir_index::model::identity::canonical::encode_inner_value;
use shamir_index::persistence::PersistedIndexes;
use shamir_index::state::IndexState;
use shamir_index::tokenizer::token_identity;
use shamir_index::{MetaEnvelope, MetaError};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{KvOp, RecordKey, Store};
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use shamir_types::types::value::QueryValue;
use smallvec::SmallVec;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::p34_12_identity_migration_tests::{count_prefix, legacy_v3_key};
use crate::index::index_definition::IndexDefinition;
use crate::index::index_info_item::IndexInfoItem;
use crate::index::index_record_key::IndexRecordKey;
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;
use crate::table::TableManager;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_wal::{WalDurability, WalEntryV2, WalOpV2};

const FTS_ID: u32 = 20;
const FUNCTIONAL_ID: u32 = 21;
const RID: [u8; 16] = [0x11; 16];
fn identity_descriptors(
    body: u64,
    number: u64,
    fts_name: u64,
    functional_name: u64,
) -> PersistedIndexes {
    PersistedIndexes {
        next_id: FUNCTIONAL_ID + 1,
        descriptors: vec![
            IndexDescriptor::new(
                FTS_ID,
                "hello_fts",
                fts_name,
                smallvec::smallvec![vec![body]],
                IndexKind::Fts {
                    tokenizer: TokenizerKind::Whitespace,
                    language: None,
                },
            ),
            IndexDescriptor::new(
                FUNCTIONAL_ID,
                "number_idx",
                functional_name,
                smallvec::smallvec![vec![number]],
                IndexKind::Functional(Box::new(FunctionalConfig {
                    expr: IndexExpr::Field(vec![number]),
                })),
            ),
        ],
    }
}

type TestStream = Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>>;

#[derive(Clone, Copy)]
enum Index2CrashPoint {
    AfterFlip,
    DuringBackfill,
}

struct Index2FailureStore {
    inner: Arc<dyn Store>,
    point: Index2CrashPoint,
    fired: AtomicBool,
}

impl Index2FailureStore {
    fn new(inner: Arc<dyn Store>, point: Index2CrashPoint) -> Self {
        Self {
            inner,
            point,
            fired: AtomicBool::new(false),
        }
    }

    fn fire_once(&self) -> bool {
        !self.fired.swap(true, Ordering::SeqCst)
    }
}

#[async_trait]
impl Store for Index2FailureStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        let meta_key = RecordId::system("_m.idx").to_bytes();
        let flipping = key.as_ref() == meta_key.as_ref()
            && matches!(self.point, Index2CrashPoint::AfterFlip)
            && MetaEnvelope::<PersistedIndexes>::open_accepting(&value, &[1, 2])
                .map(|(version, indexes)| {
                    version == 2
                        && indexes
                            .descriptors
                            .iter()
                            .all(|d| matches!(d.state, IndexState::Building))
                })
                .unwrap_or(false);
        if flipping {
            let result = self.inner.set(key, value).await?;
            if self.fire_once() {
                return Err(DbError::Storage(
                    "injected index2 failure after flip".into(),
                ));
            }
            return Ok(result);
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
        self.inner.remove_many(keys).await
    }

    async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
        let backfill = matches!(self.point, Index2CrashPoint::DuringBackfill)
            && ops.iter().any(|op| matches!(op, KvOp::Set(key, _) if key.as_ref().starts_with(&[20, 0, 0, 0, 0x11])));
        if backfill {
            self.inner.transact(ops).await?;
            if self.fire_once() {
                return Err(DbError::Storage(
                    "injected index2 failure during backfill".into(),
                ));
            }
            return Ok(());
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

fn stores() -> (Arc<dyn Store>, Arc<dyn Store>) {
    (
        Arc::new(InMemoryStore::new()),
        Arc::new(InMemoryStore::new()),
    )
}

async fn intern(table: &TableManager, name: &str) -> u64 {
    match table
        .interner()
        .get()
        .await
        .unwrap()
        .touch_ind(name)
        .unwrap()
    {
        TouchInd::Exists(key) | TouchInd::New(key) => key.id(),
    }
}

fn record(body: u64, number: u64) -> InnerValue {
    let mut map = new_map_wc(2);
    map.insert(InternerKey::new(body), InnerValue::Str("hello".into()));
    map.insert(InternerKey::new(number), InnerValue::Int(1));
    InnerValue::Map(map)
}

fn legacy_key(index: u32, tag: u8, hash: &[u8], rid: &[u8; 16]) -> Bytes {
    let mut key = index.to_le_bytes().to_vec();
    key.push(tag);
    key.extend_from_slice(hash);
    key.extend_from_slice(rid);
    Bytes::from(key)
}

// Literal legacy bytes from tracked fts_ranked_backend_tests.rs and
// functional_backend_tests.rs goldens (not hashes recomputed by this test).
async fn seed_legacy_index2(info: &Arc<dyn Store>, rid: &RecordId) {
    info.set(
        legacy_key(
            FTS_ID,
            1,
            &[0xe4, 0xe6, 0xbc, 0xc8, 0x07, 0x6c, 0x13, 0x15],
            rid.as_bytes(),
        )
        .into(),
        Bytes::from_static(&[1, 0, 0, 0, 1, 0, 0, 0]),
    )
    .await
    .unwrap();
    info.set(
        legacy_key(
            FUNCTIONAL_ID,
            2,
            &[
                0xc5, 0x13, 0x66, 0xdc, 0x33, 0xd8, 0x74, 0x12, 0x4e, 0xcd, 0xa0, 0xbd, 0xe4, 0x98,
                0xf4, 0x9c,
            ],
            rid.as_bytes(),
        )
        .into(),
        Bytes::new(),
    )
    .await
    .unwrap();
}

async fn index2_migration_fixture() -> (Arc<dyn Store>, Arc<dyn Store>, u64, u64, u64, u64) {
    let (data, info) = stores();
    let table = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();
    let body = intern(&table, "body").await;
    let number = intern(&table, "number").await;
    let fts_name = intern(&table, "hello_fts").await;
    let functional_name = intern(&table, "number_idx").await;
    let base_regular_name = intern(&table, "base_number").await;
    let base_unique_name = intern(&table, "base_number_unique").await;
    let rid = RecordId(RID);
    data.set(
        rid.to_bytes().into(),
        record(body, number).to_bytes().unwrap(),
    )
    .await
    .unwrap();
    table
        .index_manager_ref()
        .create_index(IndexDefinition::new(
            base_regular_name,
            vec![IndexInfoItem::new(vec![number])],
        ))
        .await
        .unwrap();
    table
        .index_manager_ref()
        .create_unique_index(IndexDefinition::new(
            base_unique_name,
            vec![IndexInfoItem::new(vec![number])],
        ))
        .await
        .unwrap();
    table.flush_metadata().await.unwrap();

    let v1 = MetaEnvelope::new(identity_descriptors(
        body,
        number,
        fts_name,
        functional_name,
    ))
    .encode()
    .unwrap();
    info.set(
        RecordId::system("_m.idx").to_bytes().into(),
        Bytes::from(v1),
    )
    .await
    .unwrap();
    info.set(
        RecordId::system("_m.idx.lfv").to_bytes().into(),
        Bytes::from(3u32.to_le_bytes().to_vec()),
    )
    .await
    .unwrap();
    for (unique, name) in [(false, base_regular_name), (true, base_unique_name)] {
        let mut key = legacy_v3_key(unique, name);
        if unique {
            info.set(Bytes::from(key).into(), Bytes::copy_from_slice(&RID))
                .await
                .unwrap();
        } else {
            key.extend_from_slice(&RID);
            info.set(Bytes::from(key).into(), Bytes::new())
                .await
                .unwrap();
        }
    }

    seed_legacy_index2(&info, &RecordId(RID)).await;

    (
        data,
        info,
        fts_name,
        functional_name,
        base_regular_name,
        base_unique_name,
    )
}

#[tokio::test]
async fn p34_12_index2_v1_envelope_postings_migrate_to_v2_and_stats_once() {
    let (data, info, fts_name, functional_name, _, _) = index2_migration_fixture().await;
    let rid = RecordId(RID);

    let migrated = TableManager::create("t".into(), data, Arc::clone(&info))
        .await
        .unwrap();
    assert!(migrated.identity_migrated_this_open());
    let blob = info
        .get(RecordId::system("_m.idx").to_bytes().into())
        .await
        .unwrap();
    assert!(matches!(
        MetaEnvelope::<PersistedIndexes>::open(&blob),
        Err(MetaError::UnsupportedVersion(2))
    ));
    let decoded = MetaEnvelope::<PersistedIndexes>::open_accepting(&blob, &[1, 2]).unwrap();
    assert_eq!(decoded.0, 2);
    assert_eq!(
        decoded
            .1
            .descriptors
            .iter()
            .filter(|descriptor| matches!(
                descriptor.kind,
                IndexKind::Fts { .. } | IndexKind::Functional(_)
            ))
            .count(),
        2
    );
    assert!(decoded
        .1
        .descriptors
        .iter()
        .all(|descriptor| matches!(descriptor.state, shamir_index::state::IndexState::Ready)));

    for (id, tag) in [(FTS_ID, 1u8), (FUNCTIONAL_ID, 2u8)] {
        let mut old = id.to_le_bytes().to_vec();
        old.push(tag);
        assert_eq!(
            count_prefix(&info, Bytes::from(old)).await,
            0,
            "legacy namespace must be swept"
        );
    }
    let token = token_identity("hello");
    let fts_prefix = [FTS_ID.to_le_bytes().as_slice(), &[0x11]].concat();
    assert_eq!(count_prefix(&info, Bytes::from(fts_prefix)).await, 1);
    let canonical = encode_inner_value(&InnerValue::Int(1)).unwrap();
    let fts_backend = migrated
        .index2_registry()
        .get_by_name(fts_name)
        .await
        .unwrap();
    assert!(matches!(
        fts_backend
            .lookup(IndexQuery::Fts {
                tokens: vec![token],
                mode: FtsMode::OrAny,
                limit: None,
            })
            .await
            .unwrap(),
        IndexResult::Ranked(rows) if rows.iter().any(|(found, _)| *found == rid)
    ));
    assert_eq!(
        fts_backend
            .as_any()
            .downcast_ref::<shamir_index::fts_ranked_backend::FtsRankedBackend>()
            .unwrap()
            .doc_count(),
        1,
        "rebuild must count the document once"
    );
    let functional_backend = migrated
        .index2_registry()
        .get_by_name(functional_name)
        .await
        .unwrap();
    let functional_result = functional_backend
        .lookup(IndexQuery::Point {
            keys: smallvec::smallvec![canonical],
        })
        .await
        .unwrap();
    assert!(
        matches!(functional_result, IndexResult::Set(rows) if rows.len() == 1 && rows.contains(&rid))
    );
    assert_eq!(functional_backend.descriptor().id, FUNCTIONAL_ID);
    let functional_prefix = [FUNCTIONAL_ID.to_le_bytes().as_slice(), &[0x12]].concat();
    assert!(
        count_prefix(&info, Bytes::from(functional_prefix)).await > 0,
        "v2 functional namespace must contain postings"
    );
}

#[tokio::test]
async fn p34_12_index2_repo_migration_preserves_fts_and_functional_state() {
    let repo = RepoInstance::new(
        "test".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        Vec::new(),
    );
    repo.add_table_for_test(TableConfig::new("t"));
    let table = repo.get_table("t").await.unwrap();
    let body = intern(&table, "body").await;
    let number = intern(&table, "number").await;
    let fts_name = intern(&table, "hello_fts").await;
    let functional_name = intern(&table, "number_idx").await;
    let mut expected = vec![
        table.insert(&record(body, number)).await.unwrap(),
        table.insert(&record(body, number)).await.unwrap(),
    ];
    expected.sort();
    table.flush_metadata().await.unwrap();
    repo.flush_buffers().await.unwrap();

    let info = table.info_store_for_test();
    let v1 = MetaEnvelope::new(identity_descriptors(
        body,
        number,
        fts_name,
        functional_name,
    ))
    .encode()
    .unwrap();
    info.set(
        RecordId::system("_m.idx").to_bytes().into(),
        Bytes::from(v1),
    )
    .await
    .unwrap();
    info.set(
        RecordId::system("_m.idx.lfv").to_bytes().into(),
        Bytes::from(3u32.to_le_bytes().to_vec()),
    )
    .await
    .unwrap();
    for rid in &expected {
        seed_legacy_index2(&info, rid).await;
    }

    repo.remove_table_for_test("t");
    repo.add_table_for_test(TableConfig::new("t"));
    let migrated = repo.get_table("t").await.unwrap();
    let expected: std::collections::BTreeSet<_> = expected.into_iter().collect();
    let fts = migrated
        .index2_registry()
        .get_by_name(fts_name)
        .await
        .unwrap();
    let fts_rows: std::collections::BTreeSet<RecordId> = match fts
        .lookup(IndexQuery::Fts {
            tokens: vec![token_identity("hello")],
            mode: FtsMode::OrAny,
            limit: None,
        })
        .await
        .unwrap()
    {
        IndexResult::Ranked(rows) => rows.into_iter().map(|(rid, _)| rid).collect(),
        other => panic!("expected ranked FTS results, got {other:?}"),
    };
    assert_eq!(fts_rows, expected);
    let ranked = fts
        .as_any()
        .downcast_ref::<shamir_index::fts_ranked_backend::FtsRankedBackend>()
        .unwrap();
    assert_eq!(ranked.doc_count(), 2);
    assert_eq!(ranked.sum_doc_len(), 2);

    let functional = migrated
        .index2_registry()
        .get_by_name(functional_name)
        .await
        .unwrap();
    let functional_rows = match functional
        .lookup(IndexQuery::Point {
            keys: smallvec::smallvec![encode_inner_value(&InnerValue::Int(1)).unwrap()],
        })
        .await
        .unwrap()
    {
        IndexResult::Set(rows) => rows,
        other => panic!("expected functional result set, got {other:?}"),
    };
    assert_eq!(functional_rows, expected);

    let blob = info
        .get(RecordId::system("_m.idx").to_bytes().into())
        .await
        .unwrap();
    let decoded = MetaEnvelope::<PersistedIndexes>::open_accepting(&blob, &[1, 2]).unwrap();
    assert_eq!(decoded.0, 2);
    assert!(decoded
        .1
        .descriptors
        .iter()
        .all(|descriptor| matches!(descriptor.state, shamir_index::state::IndexState::Ready)));

    let mut v2_fts = info.scan_prefix_stream(Bytes::from_static(&[20, 0, 0, 0, 0x11]), 128);
    while let Some(item) = v2_fts.next().await {
        for (key, _) in item.unwrap() {
            info.remove(key).await.unwrap();
        }
    }
    assert!(matches!(fts.lookup(IndexQuery::Fts {
        tokens: vec![token_identity("hello")],
        mode: FtsMode::OrAny,
        limit: None,
    }).await.unwrap(), IndexResult::Ranked(rows) if rows.is_empty()));
    let rid = *expected.iter().next().unwrap();
    let wal = repo.repo_wal().await.unwrap();
    let replay = WalEntryV2::new(
        wal.fresh_txn_id(),
        crate::repo::repo_token(repo.name()),
        vec![WalOpV2::IndexPut {
            table_id_interned: crate::table::table_manager::table_token_for("t"),
            idx_id: 0,
            key: legacy_key(
                FTS_ID,
                1,
                &[0xe4, 0xe6, 0xbc, 0xc8, 0x07, 0x6c, 0x13, 0x15],
                rid.as_bytes(),
            ),
            value: Bytes::from_static(&[1, 0, 0, 0, 1, 0, 0, 0]),
        }],
    )
    .with_commit_version(2);
    wal.begin_grouped(&replay, WalDurability::Buffered)
        .await
        .unwrap();
    assert!(repo.recover_v2_inflight().await.unwrap() >= 1);
    let restored = match fts
        .lookup(IndexQuery::Fts {
            tokens: vec![token_identity("hello")],
            mode: FtsMode::OrAny,
            limit: None,
        })
        .await
        .unwrap()
    {
        IndexResult::Ranked(rows) => rows
            .into_iter()
            .map(|(rid, _)| rid)
            .collect::<std::collections::BTreeSet<RecordId>>(),
        other => panic!("expected ranked FTS results, got {other:?}"),
    };
    assert_eq!(restored, expected);
    assert_eq!(ranked.doc_count(), 2);
    assert_eq!(ranked.sum_doc_len(), 2);
    assert_eq!(
        count_prefix(&info, Bytes::from_static(&[20, 0, 0, 0, 1])).await,
        0
    );

    assert!(migrated.drop_index2("hello_fts", None).await.unwrap());
    let (version, indexes) = shamir_index::persistence::load_index2_metadata(&info)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(version, 2);
    assert!(indexes
        .descriptors
        .iter()
        .any(|descriptor| descriptor.id == FUNCTIONAL_ID));
    assert!(migrated.drop_index2("number_idx", None).await.unwrap());
    let blob = info
        .get(RecordId::system("_m.idx").to_bytes().into())
        .await
        .unwrap();
    let persisted = MetaEnvelope::<PersistedIndexes>::open(&blob).unwrap();
    assert!(persisted.descriptors.is_empty());
    assert!(indexes
        .descriptors
        .iter()
        .any(|descriptor| descriptor.id == FUNCTIONAL_ID));
}

async fn crash_case(point: Index2CrashPoint) {
    let (data, raw_info, fts_name, functional_name, base_regular_name, base_unique_name) =
        index2_migration_fixture().await;
    let idv_key = RecordId::system("_m.idx.idv").to_bytes();
    let lfv_key = RecordId::system("_m.idx.lfv").to_bytes();
    let indexes_key = RecordId::system("indexes").to_bytes();
    let unique_key = RecordId::system("indexes_unique").to_bytes();
    let before = (
        raw_info.get(idv_key.clone().into()).await.ok(),
        raw_info.get(lfv_key.clone().into()).await.ok(),
        raw_info.get(indexes_key.clone().into()).await.ok(),
        raw_info.get(unique_key.clone().into()).await.ok(),
    );
    let base_definition = (
        raw_info.get(indexes_key.clone().into()).await.unwrap(),
        raw_info.get(unique_key.clone().into()).await.unwrap(),
    );
    let base_prefixes = [
        IndexRecordKey::new(false, base_regular_name).to_prefix_bytes(),
        Bytes::from(legacy_v3_key(false, base_regular_name)),
        IndexRecordKey::new(true, base_unique_name).to_prefix_bytes(),
        Bytes::from(legacy_v3_key(true, base_unique_name)),
    ];
    let base_counts = futures::future::join_all(
        base_prefixes
            .iter()
            .map(|prefix| count_prefix(&raw_info, prefix.clone())),
    )
    .await;
    let base_blob = raw_info
        .get(RecordId::system("_m.idx").to_bytes().into())
        .await
        .unwrap();
    let (_, mut expected) =
        MetaEnvelope::<PersistedIndexes>::open_accepting(&base_blob, &[1, 2]).unwrap();
    for descriptor in &mut expected.descriptors {
        descriptor.state = IndexState::Building;
    }
    let expected_bytes = bincode::serialize(&expected).unwrap();
    let failure = Arc::new(Index2FailureStore::new(Arc::clone(&raw_info), point));
    let wrapped: Arc<dyn Store> = failure.clone();
    let attempt = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&wrapped)).await;
    assert!(matches!(attempt, Err(DbError::Storage(_))));
    assert!(
        failure.fired.load(Ordering::SeqCst),
        "injection point was not reached"
    );

    let blob = raw_info
        .get(RecordId::system("_m.idx").to_bytes().into())
        .await
        .unwrap();
    let (version, indexes) =
        MetaEnvelope::<PersistedIndexes>::open_accepting(&blob, &[1, 2]).unwrap();
    assert_eq!(version, 2);
    assert_eq!(indexes.next_id, 22);
    assert_eq!(
        indexes.descriptors.iter().map(|d| d.id).collect::<Vec<_>>(),
        vec![20, 21]
    );
    assert!(indexes
        .descriptors
        .iter()
        .all(|d| d.state == IndexState::Building));
    assert_eq!(bincode::serialize(&indexes).unwrap(), expected_bytes);
    assert_eq!(
        before,
        (
            raw_info.get(idv_key.into()).await.ok(),
            raw_info.get(lfv_key.into()).await.ok(),
            raw_info.get(indexes_key.clone().into()).await.ok(),
            raw_info.get(unique_key.clone().into()).await.ok(),
        )
    );
    assert_eq!(
        base_definition,
        (
            raw_info.get(indexes_key.into()).await.unwrap(),
            raw_info.get(unique_key.into()).await.unwrap(),
        )
    );
    assert_eq!(
        base_counts,
        futures::future::join_all(
            base_prefixes
                .iter()
                .map(|prefix| count_prefix(&raw_info, prefix.clone()))
        )
        .await
    );
    let (v1_fts, v1_func, v2_fts, v2_func) = if matches!(point, Index2CrashPoint::AfterFlip) {
        (1, 1, 0, 0)
    } else {
        (0, 1, 1, 0)
    };
    assert_eq!(
        count_prefix(&raw_info, Bytes::from_static(&[20, 0, 0, 0, 1])).await,
        v1_fts
    );
    assert_eq!(
        count_prefix(&raw_info, Bytes::from_static(&[21, 0, 0, 0, 2])).await,
        v1_func
    );
    assert_eq!(
        count_prefix(&raw_info, Bytes::from_static(&[20, 0, 0, 0, 0x11])).await,
        v2_fts
    );
    assert_eq!(
        count_prefix(&raw_info, Bytes::from_static(&[21, 0, 0, 0, 0x12])).await,
        v2_func
    );

    let reopened = TableManager::create("t".into(), Arc::clone(&data), raw_info.clone())
        .await
        .unwrap();
    assert_eq!(
        reopened.index2_registry().state_of(20).await,
        Some(IndexState::Ready)
    );
    assert_eq!(
        reopened.index2_registry().state_of(21).await,
        Some(IndexState::Ready)
    );
    let fts = reopened
        .index2_registry()
        .get_by_name(fts_name)
        .await
        .unwrap();
    assert!(
        matches!(fts.lookup(IndexQuery::Fts { tokens: vec![token_identity("hello")], mode: FtsMode::OrAny, limit: None }).await.unwrap(), IndexResult::Ranked(rows) if rows.len() == 1 && rows[0].0 == RecordId(RID))
    );
    let ranked = fts
        .as_any()
        .downcast_ref::<shamir_index::fts_ranked_backend::FtsRankedBackend>()
        .unwrap();
    assert_eq!(ranked.doc_count(), 1);
    assert_eq!(ranked.sum_doc_len(), 1);
    assert_eq!(
        count_prefix(&raw_info, Bytes::from_static(&[20, 0, 0, 0, 1])).await,
        0
    );
    assert_eq!(
        count_prefix(&raw_info, Bytes::from_static(&[21, 0, 0, 0, 2])).await,
        0
    );
    assert_eq!(
        count_prefix(&raw_info, Bytes::from_static(&[20, 0, 0, 0, 0x11])).await,
        1
    );
    assert_eq!(
        count_prefix(&raw_info, Bytes::from_static(&[21, 0, 0, 0, 0x12])).await,
        1
    );
    let functional = reopened
        .index2_registry()
        .get_by_name(functional_name)
        .await
        .unwrap();
    let result = functional
        .lookup(IndexQuery::Point {
            keys: smallvec::smallvec![encode_inner_value(&InnerValue::Int(1)).unwrap()],
        })
        .await
        .unwrap();
    assert!(
        matches!(result,IndexResult::Set(rows) if rows == [RecordId(RID)].into_iter().collect())
    );
    let blob = raw_info
        .get(RecordId::system("_m.idx").to_bytes().into())
        .await
        .unwrap();
    let (version, persisted) =
        MetaEnvelope::<PersistedIndexes>::open_accepting(&blob, &[1, 2]).unwrap();
    assert_eq!(version, 2);
    assert!(persisted
        .descriptors
        .iter()
        .all(|d| d.state == IndexState::Ready));
}

#[tokio::test]
async fn p34_12_index2_crash_after_flip_durable_state_and_reopen() {
    crash_case(Index2CrashPoint::AfterFlip).await;
}

#[tokio::test]
async fn p34_12_index2_crash_during_backfill_durable_state_and_reopen() {
    crash_case(Index2CrashPoint::DuringBackfill).await;
}

#[tokio::test]
async fn p34_12_index2_user_scalar_open_fails_closed_then_repair_recovers() {
    let (data, info) = stores();
    let table = TableManager::create("scalar".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();
    let field = intern(&table, "number").await;
    let name = intern(&table, "double_idx").await;
    let body = intern(&table, "body").await;
    let rid = RecordId(RID);
    data.set(
        rid.to_bytes().into(),
        record(body, field).to_bytes().unwrap(),
    )
    .await
    .unwrap();
    table.flush_metadata().await.unwrap();
    let mut paths = SmallVec::new();
    paths.push(vec![field]);
    let descriptor = IndexDescriptor::new(
        22,
        "double_idx",
        name,
        paths,
        IndexKind::Functional(Box::new(FunctionalConfig {
            expr: IndexExpr::Scalar {
                name: "my_double".into(),
                inner: Box::new(IndexExpr::Field(vec![field])),
            },
        })),
    );
    let v1 = MetaEnvelope::new(PersistedIndexes {
        next_id: 23,
        descriptors: vec![descriptor],
    })
    .encode()
    .unwrap();
    info.set(
        RecordId::system("_m.idx").to_bytes().into(),
        Bytes::from(v1),
    )
    .await
    .unwrap();
    info.set(
        RecordId::system("_m.idx.lfv").to_bytes().into(),
        Bytes::from(3u32.to_le_bytes().to_vec()),
    )
    .await
    .unwrap();
    info.set(
        legacy_key(
            22,
            2,
            &[
                0xc5, 0x13, 0x66, 0xdc, 0x33, 0xd8, 0x74, 0x12, 0x4e, 0xcd, 0xa0, 0xbd, 0xe4, 0x98,
                0xf4, 0x9c,
            ],
            rid.as_bytes(),
        )
        .into(),
        Bytes::new(),
    )
    .await
    .unwrap();

    let reopened = TableManager::create("scalar".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();
    assert_eq!(
        reopened.index2_registry().state_of(22).await,
        Some(shamir_index::state::IndexState::Failed)
    );
    let reason = reopened
        .index2_registry()
        .failure_reason_of(22)
        .await
        .unwrap();
    assert!(
        reason.contains("unknown_function")
            && reason.contains("set_scalar_resolver()")
            && reason.contains("repair()"),
        "unexpected diagnostic: {reason}"
    );
    let blob = info
        .get(RecordId::system("_m.idx").to_bytes().into())
        .await
        .unwrap();
    let persisted = MetaEnvelope::<PersistedIndexes>::open_accepting(&blob, &[1, 2])
        .unwrap()
        .1;
    assert_eq!(
        persisted
            .descriptors
            .iter()
            .find(|d| d.id == 22)
            .unwrap()
            .state,
        shamir_index::state::IndexState::Failed
    );
    let failure_blob = info
        .get(RecordId::system("_m.idx.err").to_bytes().into())
        .await
        .unwrap();
    let failures: Vec<(u32, String)> = MetaEnvelope::open(&failure_blob).unwrap();
    assert!(failures
        .iter()
        .any(|(id, saved)| *id == 22 && saved == &reason));
    drop(reopened);

    let reopened = TableManager::create("scalar".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();
    assert_eq!(
        reopened
            .index2_registry()
            .failure_reason_of(22)
            .await
            .as_deref(),
        Some(reason.as_str())
    );

    let layer = UserScalarLayer::new();
    layer.register(
        "my_double",
        FnEntry::pure(
            |args: &[QueryValue]| -> ScalarResult {
                match &args[0] {
                    QueryValue::Int(n) => Ok(QueryValue::Int(n * 2)),
                    _ => Err(shamir_funclib::registry::ScalarError::new("type_mismatch")),
                }
            },
            1,
            Some(1),
        )
        .trusted_pure(),
    );
    reopened
        .set_scalar_resolver(ScalarResolver::new(Arc::new(layer)))
        .await;
    reopened.repair().await.unwrap();
    assert_eq!(
        reopened.index2_registry().state_of(22).await,
        Some(shamir_index::state::IndexState::Ready)
    );
    let backend = reopened.index2_registry().get_by_name(name).await.unwrap();
    let key = encode_inner_value(&InnerValue::Int(2)).unwrap();
    assert!(
        matches!(backend.lookup(IndexQuery::Point { keys: smallvec::smallvec![key] }).await.unwrap(), IndexResult::Set(rows) if rows.contains(&rid))
    );
    let blob = info
        .get(RecordId::system("_m.idx").to_bytes().into())
        .await
        .unwrap();
    let persisted = MetaEnvelope::<PersistedIndexes>::open_accepting(&blob, &[1, 2])
        .unwrap()
        .1;
    assert_eq!(
        persisted
            .descriptors
            .iter()
            .find(|d| d.id == 22)
            .unwrap()
            .state,
        shamir_index::state::IndexState::Ready
    );
    assert!(matches!(
        info.get(RecordId::system("_m.idx.err").to_bytes().into())
            .await,
        Err(DbError::NotFound(_))
    ));
}
