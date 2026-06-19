//! L9 fast-path tests: `has_any_index()` guard skips index planning
//! on unindexed tables.

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{IsolationLevel, TxContext, TxId};
use shamir_types::core::interner::InternerKey;
use shamir_types::types::value::InnerValue;

use crate::index::index_definition::IndexDefinition;
use crate::index::index_info_item::IndexInfoItem;
use crate::table::TableManager;

async fn make_table() -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    TableManager::create("t".into(), data, info).await.unwrap()
}

// ---- has_any_index flag tests ----

#[tokio::test]
async fn has_any_index_false_for_fresh_table() {
    let tbl = make_table().await;
    assert!(
        !tbl.has_any_index(),
        "fresh table with no indexes must return false"
    );
}

#[tokio::test]
async fn has_any_index_true_after_legacy_index() {
    let tbl = make_table().await;
    assert!(!tbl.has_any_index());

    // Add a legacy (non-unique) hash index.
    let def = IndexDefinition::new(42, vec![IndexInfoItem::new(vec![1])]);
    tbl.index_manager_ref().create_index(def).await.unwrap();

    assert!(
        tbl.has_any_index(),
        "after adding a legacy index, has_any_index must be true"
    );
}

#[tokio::test]
async fn has_any_index_true_after_unique_index() {
    let tbl = make_table().await;
    assert!(!tbl.has_any_index());

    let def = IndexDefinition::new(99, vec![IndexInfoItem::new(vec![2])]);
    tbl.index_manager_ref()
        .create_unique_index(def)
        .await
        .unwrap();

    assert!(
        tbl.has_any_index(),
        "after adding a unique index, has_any_index must be true"
    );
}

#[tokio::test]
async fn has_any_index_true_after_index2_backend() {
    let tbl = make_table().await;
    assert!(!tbl.has_any_index());

    // Register an FTS index2 backend.
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let desc = crate::index2::descriptor::IndexDescriptor {
        id: 1,
        name: "idx_test".into(),
        name_interned: 55,
        paths: smallvec::smallvec![vec![1]],
        kind: crate::index2::kind::IndexKind::Fts {
            tokenizer: crate::index2::kind::TokenizerKind::Whitespace,
            language: None,
        },
        created_at_nanos: 0,
        options: Vec::new(),
    };
    let backend = crate::index2::build_index2_backend(desc, &info);
    tbl.index2_registry().insert(backend).await.unwrap();

    assert!(
        tbl.has_any_index(),
        "after adding an index2 backend, has_any_index must be true"
    );
}

// ---- fast-path behavioural test: unindexed insert produces no index ops ----

#[tokio::test]
async fn insert_tx_many_unindexed_produces_no_index_ops() {
    let tbl = make_table().await;
    assert!(!tbl.has_any_index());

    let mut tx = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);

    let values = vec![InnerValue::Str("a".into()), InnerValue::Str("b".into())];
    let ids = tbl.insert_tx_many(&values, &mut tx).await.unwrap();
    assert_eq!(ids.len(), 2);

    // No index ops should have been staged.
    assert!(
        tx.index_write_set.is_empty(),
        "unindexed table must produce zero index_write_set entries"
    );
}

#[tokio::test]
async fn insert_tx_unindexed_produces_no_index_ops() {
    let tbl = make_table().await;
    assert!(!tbl.has_any_index());

    let mut tx = TxContext::new(TxId::new(2), 0, 0, IsolationLevel::Snapshot);

    let _rid = tbl
        .insert_tx(&InnerValue::Str("v".into()), Some(&mut tx))
        .await
        .unwrap();

    assert!(
        tx.index_write_set.is_empty(),
        "single insert_tx on unindexed table must produce zero index ops"
    );
}

// ---- indexed table still writes index entries ----

#[tokio::test]
async fn insert_tx_many_indexed_produces_index_ops() {
    let tbl = make_table().await;

    // Add a legacy hash index on field path [1].
    let def = IndexDefinition::new(42, vec![IndexInfoItem::new(vec![1])]);
    tbl.index_manager_ref().create_index(def).await.unwrap();
    assert!(tbl.has_any_index());

    let mut tx = TxContext::new(TxId::new(3), 0, 0, IsolationLevel::Snapshot);

    // Build an InnerValue with the indexed field present.
    let mut map = shamir_types::types::common::new_map();
    map.insert(InternerKey::new(1), InnerValue::Str("hello".into()));
    let value = InnerValue::Map(map);

    let ids = tbl.insert_tx_many(&[value], &mut tx).await.unwrap();
    assert_eq!(ids.len(), 1);

    assert!(
        !tx.index_write_set.is_empty(),
        "indexed table must produce index_write_set entries"
    );
}
