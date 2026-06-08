//! P2 — route `TableManager` current-reads through the MVCC seam.
//!
//! With an `MvccStore` attached, `TableManager::get` and `list_stream` flow
//! through `MvccStore::get_current` / `current_stream` instead of the table's
//! `data_store`. Because `MvccStore` is constructed with the table's
//! `data_store` as its `main`, the two paths are byte-identical TODAY; these
//! tests pin that parity so a later collapse-main slice can swap the seam body.

use std::sync::Arc;

use futures::StreamExt;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{MvccStore, RepoTxGate};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::table::TableManager;

/// Build a `TableManager` with an `MvccStore` attached over the SAME
/// `data_store` (mirroring `RepoInstance::create_table_context`).
async fn make_mvcc_table() -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let tbl = TableManager::create("t".into(), Arc::clone(&data), info)
        .await
        .unwrap();
    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    tbl.with_mvcc_store(mvcc)
}

#[tokio::test]
async fn get_present_key_returns_value_via_seam() {
    let tbl = make_mvcc_table().await;
    let value = InnerValue::Str("hello".to_string());
    let id = tbl.insert(&value).await.unwrap();

    let got = tbl.get(id).await.unwrap();
    assert_eq!(format!("{got:?}"), format!("{value:?}"));
}

#[tokio::test]
async fn get_absent_key_returns_not_found_via_seam() {
    let tbl = make_mvcc_table().await;
    let unknown = RecordId::new();

    let err = tbl.get(unknown).await.unwrap_err();
    assert!(
        matches!(err, shamir_storage::error::DbError::NotFound(_)),
        "expected NotFound, got {err:?}"
    );
}

#[tokio::test]
async fn list_stream_yields_inserted_records_via_seam() {
    let tbl = make_mvcc_table().await;
    let n = 5;
    let mut inserted = Vec::new();
    for i in 0..n {
        let v = InnerValue::Str(format!("v{i}"));
        let id = tbl.insert(&v).await.unwrap();
        inserted.push((id, v));
    }

    let mut stream = tbl.list_stream(1000);
    let mut collected: Vec<(RecordId, InnerValue)> = Vec::new();
    while let Some(batch) = stream.next().await {
        collected.extend(batch.unwrap());
    }

    assert_eq!(collected.len(), n, "expected exactly {n} records");
    for (id, v) in &inserted {
        let found = collected
            .iter()
            .find(|(rid, val)| rid == id && format!("{val:?}") == format!("{v:?}"));
        assert!(
            found.is_some(),
            "inserted record {id:?} not found in stream"
        );
    }
}
