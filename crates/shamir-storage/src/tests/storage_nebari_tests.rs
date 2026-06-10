#![allow(deprecated)]

use crate::error::DbError;
use crate::storage_nebari::NebariRepo;
use crate::tests::types_tests::{collect_stream, run_batch_store_tests};
use crate::types::{KvOp, RecordKey, Repo, Store};
use bytes::Bytes;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

async fn run_store_tests(store: Arc<dyn Store>) {
    // Test insert and get
    let value1 = InnerValue::Str("hello".to_string());
    let key1 = store.insert(value1.to_bytes().unwrap()).await.unwrap();
    let retrieved_bytes = store.get(key1.clone()).await.unwrap();
    assert_eq!(InnerValue::from_bytes(retrieved_bytes).unwrap(), value1);

    // Test set (update)
    sleep(Duration::from_micros(50)).await;
    let value2 = InnerValue::Str("world".to_string());
    let created = store
        .set(key1.clone(), value2.to_bytes().unwrap())
        .await
        .unwrap();
    assert!(!created); // Should be false, as it's an update
    let retrieved_bytes2 = store.get(key1.clone()).await.unwrap();
    assert_eq!(InnerValue::from_bytes(retrieved_bytes2).unwrap(), value2);

    // Test set (create)
    let id2 = RecordId::new();
    let key2 = Bytes::copy_from_slice(id2.as_bytes());
    let value3 = InnerValue::Int(123);
    let created2 = store
        .set(key2.clone(), value3.to_bytes().unwrap())
        .await
        .unwrap();
    assert!(created2); // Should be true, as it's a new record
    let retrieved_bytes3 = store.get(key2.clone()).await.unwrap();
    assert_eq!(InnerValue::from_bytes(retrieved_bytes3).unwrap(), value3);

    // Test iter
    let value4 = InnerValue::Bool(true);
    let _key3 = store.insert(value4.to_bytes().unwrap()).await.unwrap();
    let all_records = collect_stream(store.iter_stream(1000)).await.unwrap();
    assert_eq!(all_records.len(), 3);
    assert!(all_records.iter().any(|(k, _)| *k == key1));
    assert!(all_records
        .iter()
        .any(|(_, bytes)| InnerValue::from_bytes(bytes.clone()).unwrap() == value4));

    // Test remove
    assert!(store.remove(key1.clone()).await.unwrap());
    assert!(store.get(key1.clone()).await.is_err());
    assert!(!store.remove(key1).await.unwrap()); // Already removed

    let all_records_after_remove = collect_stream(store.iter_stream(1000)).await.unwrap();
    assert_eq!(all_records_after_remove.len(), 2);
}

#[tokio::test]
async fn test_nebari_repo_basic() {
    let path = "./test_data/nebari_repo_basic.nebari";
    if Path::new(path).exists() {
        fs::remove_dir_all(path).unwrap();
    }

    let repo = NebariRepo::new(path).unwrap();
    let store = repo.store_get("test_table").await.unwrap();

    run_store_tests(store).await;

    assert!(repo.store_delete("test_table").await.unwrap());
}

#[tokio::test]
async fn test_nebari_batch_ops() {
    let path = "./test_data/nebari_batch_ops.nebari";
    if Path::new(path).exists() {
        fs::remove_dir_all(path).unwrap();
    }
    let repo = NebariRepo::new(path).unwrap();
    let store = repo.store_get("batch").await.unwrap();
    run_batch_store_tests(store).await;
}

/// Nebari transact test — verifies all ops applied atomically via
/// one `Roots::transaction` commit.
///
/// Note: nebari's `tree.get()` does not provide snapshot isolation
/// across multiple calls, so we only verify final state here
/// (write atomicity, not cross-read snapshot isolation).
#[tokio::test]
async fn test_nebari_transact_atomic() {
    let path = "./test_data/nebari_transact.nebari";
    if Path::new(path).exists() {
        fs::remove_dir_all(path).unwrap();
    }
    let repo = NebariRepo::new(path).unwrap();
    let store = repo.store_get("transact_test").await.unwrap();

    // Seed
    let k1: RecordKey = Bytes::from_static(b"k1");
    let k2: RecordKey = Bytes::from_static(b"k2");
    let k3: RecordKey = Bytes::from_static(b"k3");
    store
        .set(k1.clone(), Bytes::from_static(b"old1"))
        .await
        .unwrap();
    store
        .set(k2.clone(), Bytes::from_static(b"old2"))
        .await
        .unwrap();
    store
        .set(k3.clone(), Bytes::from_static(b"to_remove"))
        .await
        .unwrap();

    // Mixed transact: update k1, update k2, remove k3
    store
        .transact(vec![
            KvOp::Set(k1.clone(), Bytes::from_static(b"new1")),
            KvOp::Set(k2.clone(), Bytes::from_static(b"new2")),
            KvOp::Remove(k3.clone()),
        ])
        .await
        .unwrap();

    assert_eq!(store.get(k1).await.unwrap().as_ref(), b"new1");
    assert_eq!(store.get(k2).await.unwrap().as_ref(), b"new2");
    assert!(store.get(k3).await.is_err(), "k3 should be removed");

    fs::remove_dir_all(path).ok();
}

#[tokio::test]
async fn test_nebari_repo_list_stores() {
    let path = "./test_data/nebari_repo_list.nebari";
    if Path::new(path).exists() {
        fs::remove_dir_all(path).unwrap();
    }

    let repo = NebariRepo::new(path).unwrap();

    // Create first store
    let _store1 = repo.store_get("table1").await.unwrap();

    let tables = repo.stores_list().await.unwrap();
    assert_eq!(tables.len(), 1);
    assert!(tables.contains(&"table1".to_string()));

    // Create second store
    let _store2 = repo.store_get("table2").await.unwrap();

    let tables = repo.stores_list().await.unwrap();
    assert_eq!(tables.len(), 2);
    assert!(tables.contains(&"table1".to_string()));
    assert!(tables.contains(&"table2".to_string()));

    // Delete one store
    assert!(repo.store_delete("table1").await.unwrap());
    let tables = repo.stores_list().await.unwrap();
    assert_eq!(tables.len(), 1);
    assert!(!tables.contains(&"table1".to_string()));
    assert!(tables.contains(&"table2".to_string()));
}

#[tokio::test]
async fn test_nebari_repo_store_isolation() {
    let path = "./test_data/nebari_repo_isolation.nebari";
    if Path::new(path).exists() {
        fs::remove_dir_all(path).unwrap();
    }

    let repo = NebariRepo::new(path).unwrap();

    let store1 = repo.store_get("isolated_table1").await.unwrap();
    let store2 = repo.store_get("isolated_table2").await.unwrap();

    // Insert into table1
    let value1 = InnerValue::Str("table1_value".to_string());
    let key1 = store1.insert(value1.to_bytes().unwrap()).await.unwrap();

    // Insert into table2
    let value2 = InnerValue::Str("table2_value".to_string());
    let key2 = store2.insert(value2.to_bytes().unwrap()).await.unwrap();

    // Verify isolation - each table should have only 1 record
    assert_eq!(
        collect_stream(store1.iter_stream(1000))
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        collect_stream(store2.iter_stream(1000))
            .await
            .unwrap()
            .len(),
        1
    );

    // Verify correct values
    let retrieved_bytes1 = store1.get(key1.clone()).await.unwrap();
    assert_eq!(InnerValue::from_bytes(retrieved_bytes1).unwrap(), value1);

    let retrieved_bytes2 = store2.get(key2.clone()).await.unwrap();
    assert_eq!(InnerValue::from_bytes(retrieved_bytes2).unwrap(), value2);

    // Verify cross-table isolation (get should fail with NotFound)
    assert!(matches!(store2.get(key1).await, Err(DbError::NotFound(_))));
    assert!(matches!(store1.get(key2).await, Err(DbError::NotFound(_))));

    // Clean up
    repo.store_delete("isolated_table1").await.unwrap();
    repo.store_delete("isolated_table2").await.unwrap();
}

/// Regression: a deleted key that lands on a batch boundary (cursor)
/// must NOT cause the stream to silently drop all subsequent records.
#[tokio::test]
async fn test_nebari_deleted_cursor_no_truncation() {
    let path = "./test_data/nebari_deleted_cursor.nebari";
    if Path::new(path).exists() {
        fs::remove_dir_all(path).unwrap();
    }

    let repo = NebariRepo::new(path).unwrap();
    let store = repo.store_get("test_table").await.unwrap();

    // Insert four keys with deterministic ordering
    for i in 1..=4 {
        let key = Bytes::from(format!("k{i}"));
        let val = Bytes::from(format!("v{i}"));
        store.set(key, val).await.unwrap();
    }

    // Delete k2 — this key would be the batch-1 cursor with batch_size=2
    store.remove(Bytes::from_static(b"k2")).await.unwrap();

    // Drain with batch_size=2.  Without the ordering-based skip fix,
    // the exact-match skip never finds k2, so skipping stays true
    // and the entire batch is empty → stream ends early.
    let all = collect_stream(store.iter_stream(2)).await.unwrap();

    let mut keys: Vec<&[u8]> = all.iter().map(|(k, _)| k.as_ref()).collect();
    keys.sort();

    assert_eq!(
        keys,
        vec![&b"k1"[..], &b"k3"[..], &b"k4"[..]],
        "deleted cursor must not truncate the tail"
    );

    fs::remove_dir_all(path).ok();
}
