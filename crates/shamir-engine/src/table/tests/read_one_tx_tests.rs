//! Tests for `TableManager::read_one_tx` and `with_mvcc_store`.

use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{IsolationLevel, MvccStore, RepoTxGate, TxContext, TxId};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::table::TableManager;

async fn make_table() -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    TableManager::create("t".into(), data, info).await.unwrap()
}

fn make_tx(snapshot: u64) -> TxContext {
    TxContext::new(TxId::new(1), 0, snapshot, IsolationLevel::Snapshot)
}

#[tokio::test]
async fn read_one_tx_none_equals_get() {
    let tbl = make_table().await;
    let unknown_id = RecordId::new();

    let value = InnerValue::Str("v".to_string());
    let inserted_id = tbl.insert(&value).await.unwrap();

    let via_get = tbl.get(inserted_id).await.unwrap();
    let via_tx = tbl.read_one_tx(inserted_id, None).await.unwrap();
    assert_eq!(format!("{:?}", via_get), format!("{:?}", via_tx));

    let _ = tbl.get(unknown_id).await.unwrap_err();
    let _ = tbl.read_one_tx(unknown_id, None).await.unwrap_err();
}

#[tokio::test]
async fn read_one_tx_some_without_mvcc_falls_back_to_get() {
    let tbl = make_table().await;
    let value = InnerValue::Str("a".to_string());
    let id = tbl.insert(&value).await.unwrap();

    let tx = make_tx(100);
    let via = tbl.read_one_tx(id, Some(&tx)).await.unwrap();
    let direct = tbl.get(id).await.unwrap();
    assert_eq!(format!("{:?}", via), format!("{:?}", direct));
}

#[tokio::test]
async fn read_one_tx_routes_through_mvcc_when_attached() {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let tbl = TableManager::create("t".into(), Arc::clone(&data), info)
        .await
        .unwrap();
    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    let tbl = tbl.with_mvcc_store(Arc::clone(&mvcc));

    let value = InnerValue::Str("x".to_string());
    let id = tbl.insert(&value).await.unwrap();

    let tx = make_tx(u64::MAX);
    let via_tx = tbl.read_one_tx(id, Some(&tx)).await.unwrap();
    let direct = tbl.get(id).await.unwrap();
    assert_eq!(format!("{:?}", via_tx), format!("{:?}", direct));
}

#[tokio::test]
async fn read_one_tx_with_mvcc_not_found_maps_to_error() {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let tbl = TableManager::create("t".into(), Arc::clone(&data), info)
        .await
        .unwrap();
    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    let tbl = tbl.with_mvcc_store(mvcc);

    let id = RecordId::new();
    let tx = make_tx(100);
    let err = tbl.read_one_tx(id, Some(&tx)).await.unwrap_err();
    assert!(
        matches!(err, shamir_storage::error::DbError::NotFound(_)),
        "expected NotFound, got {:?}",
        err
    );
}
