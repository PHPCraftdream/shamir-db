//! Tests for TableManager::insert_tx (Stage 4.D.6.a) and
//! execute_insert_tx (Stage 4.D.6.c.1).

use std::sync::Arc;

use serde_json::json;

use shamir_query_builder::{filter, write};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{IsolationLevel, TxContext, TxId};
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::query::filter::eval_context::FilterContext;
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

// ---- Stage 4.D.6.b: update_tx / delete_tx / set_tx ----

#[tokio::test]
async fn update_tx_none_delegates_to_set() {
    let tbl = make_table().await;
    let rid = tbl.insert(&InnerValue::Str("v1".into())).await.unwrap();
    let existed = tbl
        .update_tx(rid, &InnerValue::Str("v2".into()), None)
        .await
        .unwrap();
    assert!(
        !existed,
        "update_tx(None) delegates to set; set returns created=false for existing"
    );
    let v = tbl.get(rid).await.unwrap();
    if let InnerValue::Str(s) = v {
        assert_eq!(s, "v2");
    } else {
        panic!("expected Str");
    }
}

#[tokio::test]
async fn update_tx_some_stages_diff_index_ops() {
    let tbl = make_table().await;
    let rid = tbl.insert(&InnerValue::Str("v1".into())).await.unwrap();

    let mut tx = TxContext::new(TxId::new(10), 0, u64::MAX, IsolationLevel::Snapshot);
    let existed = tbl
        .update_tx(rid, &InnerValue::Str("v2".into()), Some(&mut tx))
        .await
        .unwrap();
    assert!(existed);

    // Main store still has v1 (staged update not applied yet).
    let direct = tbl.get(rid).await.unwrap();
    if let InnerValue::Str(s) = direct {
        assert_eq!(s, "v1", "main store must not be modified before commit");
    }

    // counter_delta = 0 for update.
    let token = tbl.table_token();
    assert_eq!(*tx.counter_deltas.get(&token).unwrap_or(&-1), 0);
}

#[tokio::test]
async fn update_tx_some_on_missing_id_acts_as_insert() {
    let tbl = make_table().await;
    let mut tx = TxContext::new(TxId::new(11), 0, u64::MAX, IsolationLevel::Snapshot);
    let id = RecordId::new();
    let existed = tbl
        .update_tx(id, &InnerValue::Str("new".into()), Some(&mut tx))
        .await
        .unwrap();
    assert!(!existed, "missing id → existed=false");

    // counter_delta = 1 since this acts as insert.
    let token = tbl.table_token();
    assert_eq!(*tx.counter_deltas.get(&token).unwrap(), 1);
}

#[tokio::test]
async fn delete_tx_none_delegates_to_delete() {
    let tbl = make_table().await;
    let rid = tbl.insert(&InnerValue::Str("v".into())).await.unwrap();
    let removed = tbl.delete_tx(rid, None).await.unwrap();
    assert!(removed);
    assert!(tbl.get(rid).await.is_err());
}

#[tokio::test]
async fn delete_tx_some_stages_remove() {
    let tbl = make_table().await;
    let rid = tbl.insert(&InnerValue::Str("v".into())).await.unwrap();

    let mut tx = TxContext::new(TxId::new(20), 0, u64::MAX, IsolationLevel::Snapshot);
    let removed = tbl.delete_tx(rid, Some(&mut tx)).await.unwrap();
    assert!(removed);

    // Main store still has the record (staged remove not applied).
    let _ = tbl.get(rid).await.unwrap();

    let token = tbl.table_token();
    assert_eq!(*tx.counter_deltas.get(&token).unwrap(), -1);
}

#[tokio::test]
async fn delete_tx_some_on_missing_id_returns_false() {
    let tbl = make_table().await;
    let mut tx = TxContext::new(TxId::new(21), 0, u64::MAX, IsolationLevel::Snapshot);
    let id = RecordId::new();
    let removed = tbl.delete_tx(id, Some(&mut tx)).await.unwrap();
    assert!(!removed);

    // No counter delta — nothing was staged.
    let token = tbl.table_token();
    assert!(!tx.counter_deltas.contains_key(&token));
}

#[tokio::test]
async fn set_tx_acts_as_update_tx() {
    let tbl = make_table().await;
    let rid = tbl.insert(&InnerValue::Str("orig".into())).await.unwrap();

    let mut tx = TxContext::new(TxId::new(30), 0, u64::MAX, IsolationLevel::Snapshot);
    let existed = tbl
        .set_tx(rid, &InnerValue::Str("new".into()), Some(&mut tx))
        .await
        .unwrap();
    assert!(existed);
}

// ---- Stage 4.D.6.c.1: execute_insert_tx ----

#[tokio::test]
async fn execute_insert_tx_stages_all_records() {
    let tbl = make_table().await;
    let mut tx = TxContext::new(TxId::new(40), 0, u64::MAX, IsolationLevel::Snapshot);

    let op = write::insert("t")
        .rows([json!("v1"), json!("v2"), json!("v3")])
        .build();

    let result = tbl.execute_insert_tx(&op, &mut tx).await.unwrap();
    assert_eq!(result.affected, 3);
    assert_eq!(result.records.len(), 3);
    for r in &result.records {
        assert!(r.get("_id").is_some(), "_id must be attached");
    }

    let token = tbl.table_token();
    assert!(tx.write_set.contains_key(&token));
    assert_eq!(*tx.counter_deltas.get(&token).unwrap(), 3);
}

#[tokio::test]
async fn execute_insert_tx_empty_values() {
    let tbl = make_table().await;
    let mut tx = TxContext::new(TxId::new(41), 0, u64::MAX, IsolationLevel::Snapshot);

    let op = write::insert("t").build();
    let result = tbl.execute_insert_tx(&op, &mut tx).await.unwrap();
    assert_eq!(result.affected, 0);
    assert!(result.records.is_empty());
}

// ---- Stage 4.D.6.c.2: execute_update_tx / execute_delete_tx / execute_set_tx ----

#[tokio::test]
async fn execute_update_tx_stages_via_update_tx() {
    let tbl = make_table().await;
    let interner = tbl.interner().get().await.unwrap();

    let existing = json!({ "name": "bob" });
    let inner = shamir_types::codecs::interned::json_value_to_inner(&existing, interner).unwrap();
    let _rid = tbl.insert(&inner).await.unwrap();

    let mut tx = TxContext::new(TxId::new(50), 0, u64::MAX, IsolationLevel::Snapshot);

    let op = write::update("t").set(json!({ "name": "alice" })).build();

    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let result = tbl.execute_update_tx(&op, &ctx, &mut tx).await.unwrap();
    assert_eq!(result.affected, 1);

    let token = tbl.table_token();
    assert_eq!(
        *tx.counter_deltas.get(&token).unwrap_or(&-99),
        0,
        "update must not change row count"
    );
}

#[tokio::test]
async fn execute_update_tx_no_match_zero_affected() {
    let tbl = make_table().await;

    let mut tx = TxContext::new(TxId::new(51), 0, u64::MAX, IsolationLevel::Snapshot);

    let op = write::update("t").set(json!({ "name": "alice" })).build();

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let result = tbl.execute_update_tx(&op, &ctx, &mut tx).await.unwrap();
    assert_eq!(result.affected, 0);
}

#[tokio::test]
async fn execute_delete_tx_stages_via_delete_tx() {
    let tbl = make_table().await;
    let _rid = tbl.insert(&InnerValue::Str("victim".into())).await.unwrap();

    let mut tx = TxContext::new(TxId::new(52), 0, u64::MAX, IsolationLevel::Snapshot);

    let op = write::delete("t").where_(filter::and(vec![])).build();

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let result = tbl.execute_delete_tx(&op, &ctx, &mut tx).await.unwrap();
    assert_eq!(result.affected, 1);
    assert!(result.records.is_empty());

    let token = tbl.table_token();
    assert_eq!(
        *tx.counter_deltas.get(&token).unwrap(),
        -1,
        "delete must decrement counter by 1"
    );
}

#[tokio::test]
async fn execute_delete_tx_no_match_zero_affected() {
    let tbl = make_table().await;

    let mut tx = TxContext::new(TxId::new(53), 0, u64::MAX, IsolationLevel::Snapshot);

    let op = write::delete("t").where_(filter::and(vec![])).build();

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let result = tbl.execute_delete_tx(&op, &ctx, &mut tx).await.unwrap();
    assert_eq!(result.affected, 0);
}

#[tokio::test]
async fn execute_set_tx_insert_path() {
    let tbl = make_table().await;
    let mut tx = TxContext::new(TxId::new(60), 0, u64::MAX, IsolationLevel::Snapshot);

    let op = write::upsert("t")
        .key(json!({ "email": "a@b.c" }))
        .value(json!({ "email": "a@b.c", "name": "alice" }))
        .build();

    let result = tbl.execute_set_tx(&op, &mut tx).await.unwrap();
    assert_eq!(result.affected, 1);
    assert_eq!(result.records.len(), 1);
    assert_eq!(result.records[0]["_created"], json!(true));

    let token = tbl.table_token();
    assert_eq!(*tx.counter_deltas.get(&token).unwrap(), 1);
}

#[tokio::test]
async fn execute_set_tx_update_path() {
    let tbl = make_table().await;

    let existing = json!({ "email": "a@b.c", "name": "alice" });
    let inner = shamir_types::codecs::interned::json_value_to_inner(
        &existing,
        tbl.interner().get().await.unwrap(),
    )
    .unwrap();
    let _rid = tbl.insert(&inner).await.unwrap();

    let mut tx = TxContext::new(TxId::new(61), 0, u64::MAX, IsolationLevel::Snapshot);

    let op = write::upsert("t")
        .key(json!({ "email": "a@b.c" }))
        .value(json!({ "name": "bob" }))
        .build();

    let result = tbl.execute_set_tx(&op, &mut tx).await.unwrap();
    assert_eq!(result.affected, 1);
    assert_eq!(result.records[0]["_created"], json!(false));

    let token = tbl.table_token();
    assert_eq!(
        *tx.counter_deltas.get(&token).unwrap_or(&0),
        0,
        "update via set_tx must not change row count"
    );
}
