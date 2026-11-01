//! S3 index2 identity migration acceptance: real v1 envelope/postings, v2
//! namespaces, rebuilt lookups and exactly-once FTS stats after open migration.

use bytes::Bytes;
use shamir_index::backend::{FtsMode, IndexQuery, IndexResult};
use shamir_index::descriptor::IndexDescriptor;
use shamir_index::expr::IndexExpr;
use shamir_index::kind::{FunctionalConfig, IndexKind, TokenizerKind};
use shamir_index::model::identity::canonical::encode_inner_value;
use shamir_index::persistence::PersistedIndexes;
use shamir_index::tokenizer::token_identity;
use shamir_index::{MetaEnvelope, MetaError};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use smallvec::SmallVec;
use std::sync::Arc;

use super::p34_12_identity_migration_tests::count_prefix;
use crate::table::TableManager;

const FTS_ID: u32 = 20;
const FUNCTIONAL_ID: u32 = 21;
const RID: [u8; 16] = [0x11; 16];

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

#[tokio::test]
async fn p34_12_index2_v1_envelope_postings_migrate_to_v2_and_stats_once() {
    let (data, info) = stores();
    let table = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();
    let body = intern(&table, "body").await;
    let number = intern(&table, "number").await;
    let fts_name = intern(&table, "hello_fts").await;
    let functional_name = intern(&table, "number_idx").await;
    let rid = RecordId(RID);
    data.set(
        rid.to_bytes().into(),
        record(body, number).to_bytes().unwrap(),
    )
    .await
    .unwrap();
    table.flush_metadata().await.unwrap();

    let mut fts_paths = SmallVec::new();
    fts_paths.push(vec![body]);
    let fts = IndexDescriptor::new(
        FTS_ID,
        "hello_fts",
        fts_name,
        fts_paths,
        IndexKind::Fts {
            tokenizer: TokenizerKind::Whitespace,
            language: None,
        },
    );
    let mut functional_paths = SmallVec::new();
    functional_paths.push(vec![number]);
    let functional = IndexDescriptor::new(
        FUNCTIONAL_ID,
        "number_idx",
        functional_name,
        functional_paths,
        IndexKind::Functional(Box::new(FunctionalConfig {
            expr: IndexExpr::Field(vec![number]),
        })),
    );
    let v1 = MetaEnvelope::new(PersistedIndexes {
        next_id: FUNCTIONAL_ID + 1,
        descriptors: vec![fts, functional],
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

    // Literal legacy bytes from tracked fts_ranked_backend_tests.rs and
    // functional_backend_tests.rs goldens (not hashes recomputed by this test).
    info.set(
        legacy_key(
            FTS_ID,
            1,
            &[0xe4, 0xe6, 0xbc, 0xc8, 0x07, 0x6c, 0x13, 0x15],
            &RID,
        )
        .into(),
        Bytes::new(),
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
            &RID,
        )
        .into(),
        Bytes::new(),
    )
    .await
    .unwrap();

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
    assert!(matches!(functional_result, IndexResult::Set(rows) if rows.contains(&rid)));
    assert_eq!(functional_backend.descriptor().id, FUNCTIONAL_ID);
    let functional_prefix = [FUNCTIONAL_ID.to_le_bytes().as_slice(), &[0x12]].concat();
    assert!(
        count_prefix(&info, Bytes::from(functional_prefix)).await > 0,
        "v2 functional namespace must contain postings"
    );
}
