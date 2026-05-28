//! Tests for TableManager::insert_tx (Stage 4.D.6.a).

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{IsolationLevel, TxContext, TxId};
use shamir_types::types::value::InnerValue;

use crate::table::TableManager;

async fn make_table() -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    TableManager::create("t".into(), data, info).await.unwrap()
}

#[tokio::test]
async fn insert_tx_none_delegates_to_insert() {
    let tbl = make_table().await;
    let rid = tbl
        .insert_tx(&InnerValue::Str("v".into()), None)
        .await
        .unwrap();
    let _ = tbl.get(rid).await.unwrap();
}

#[tokio::test]
async fn insert_tx_some_stages_in_write_set() {
    let tbl = make_table().await;
    let mut tx = TxContext::new(TxId::new(1), 0, 0, IsolationLevel::Snapshot);

    let rid = tbl
        .insert_tx(&InnerValue::Str("staged".into()), Some(&mut tx))
        .await
        .unwrap();

    assert!(
        tbl.get(rid).await.is_err(),
        "staged write must not be in main store"
    );

    let token = tbl.table_token();
    assert!(tx.write_set.contains_key(&token));
    assert_eq!(tx.table_tokens.get(&token), Some(&"t".to_string()));

    assert_eq!(*tx.counter_deltas.get(&token).unwrap(), 1);
}

#[tokio::test]
async fn insert_tx_multiple_same_table() {
    let tbl = make_table().await;
    let mut tx = TxContext::new(TxId::new(2), 0, 0, IsolationLevel::Snapshot);

    let r1 = tbl
        .insert_tx(&InnerValue::Int(1), Some(&mut tx))
        .await
        .unwrap();
    let r2 = tbl
        .insert_tx(&InnerValue::Int(2), Some(&mut tx))
        .await
        .unwrap();
    assert_ne!(r1, r2);

    let token = tbl.table_token();
    assert_eq!(*tx.counter_deltas.get(&token).unwrap(), 2);
    assert_eq!(tx.write_set.len(), 1, "same table = one StagingStore");
}

#[tokio::test]
async fn table_token_is_deterministic() {
    let tbl = make_table().await;
    let t1 = tbl.table_token();
    let t2 = tbl.table_token();
    assert_eq!(t1, t2, "table_token must be deterministic");
    assert_ne!(t1, 0);
}
