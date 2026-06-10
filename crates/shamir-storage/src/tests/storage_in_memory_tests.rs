#![allow(deprecated)]

use crate::storage_in_memory::{InMemoryRepo, InMemoryStore};
use crate::tests::types_tests::{collect_stream, run_batch_store_tests};
use crate::types::{KvOp, RecordKey, Repo, Store};
use bytes::Bytes;
use futures::StreamExt;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
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
async fn test_inmemory_repo_basic() {
    let repo = InMemoryRepo::new();
    let store = repo.store_get("test_table").await.unwrap();

    run_store_tests(store).await;

    assert!(repo.store_delete("test_table").await.unwrap());
    assert!(!repo.store_delete("nonexistent").await.unwrap());
}

#[tokio::test]
async fn test_inmemory_batch_ops() {
    let repo = InMemoryRepo::new();
    let store = repo.store_get("batch").await.unwrap();
    run_batch_store_tests(store).await;
}

#[tokio::test]
async fn test_inmemory_repo_list_and_delete_stores() {
    let repo = InMemoryRepo::new();

    let _ = repo.store_get("table1").await.unwrap();
    let _ = repo.store_get("table2").await.unwrap();
    let _ = repo.store_get("table3").await.unwrap();

    let mut tables = repo.stores_list().await.unwrap();
    tables.sort(); // Order is not guaranteed
    assert_eq!(tables, vec!["table1", "table2", "table3"]);

    assert!(repo.store_delete("table2").await.unwrap());

    let mut tables_after_delete = repo.stores_list().await.unwrap();
    tables_after_delete.sort();
    assert_eq!(tables_after_delete, vec!["table1", "table3"]);
}

#[tokio::test]
async fn test_inmemory_iter_stream() {
    let store = InMemoryStore::new();

    // Insert 25 records
    let mut expected_keys = Vec::new();
    for i in 0..25 {
        let value = InnerValue::Int(i);
        let key = store.insert(value.to_bytes().unwrap()).await.unwrap();
        expected_keys.push(key);
    }

    // Test streaming with batch_size=10
    let mut stream = store.iter_stream(10);
    let mut all_records = Vec::new();
    let mut batch_count = 0;

    while let Some(batch_result) = stream.next().await {
        let batch = batch_result.unwrap();
        batch_count += 1;
        all_records.extend(batch);
    }

    assert_eq!(all_records.len(), 25);
    assert_eq!(batch_count, 3); // 10 + 10 + 5 = 25

    // Verify all keys are present
    for key in &expected_keys {
        assert!(all_records.iter().any(|(rec_key, _)| rec_key == key));
    }
}

#[tokio::test]
async fn test_inmemory_iter_range_stream_inclusive_bounds() {
    // Seed deterministic keys: "k00".."k19" — sortable bytes.
    let store = InMemoryStore::new();
    for i in 0..20 {
        let key = Bytes::from(format!("k{:02}", i));
        let value = Bytes::from(format!("v{}", i));
        store.set(key, value).await.unwrap();
    }

    // Range [k05 ..= k10] — six entries inclusive.
    let stream = store.iter_range_stream(Some(Bytes::from("k05")), Some(Bytes::from("k10")), 100);
    let mut got: Vec<String> = Vec::new();
    futures::pin_mut!(stream);
    while let Some(batch) = stream.next().await {
        for (k, _) in batch.unwrap() {
            got.push(String::from_utf8(k.to_vec()).unwrap());
        }
    }
    got.sort();
    assert_eq!(
        got,
        vec!["k05", "k06", "k07", "k08", "k09", "k10"],
        "range filter must include both bounds"
    );
}

#[tokio::test]
async fn test_inmemory_iter_range_stream_unbounded_lower() {
    let store = InMemoryStore::new();
    for i in 0..10 {
        let key = Bytes::from(format!("k{:02}", i));
        store.set(key, Bytes::from(format!("v{i}"))).await.unwrap();
    }
    let stream = store.iter_range_stream(None, Some(Bytes::from("k02")), 100);
    let mut got: Vec<String> = Vec::new();
    futures::pin_mut!(stream);
    while let Some(batch) = stream.next().await {
        for (k, _) in batch.unwrap() {
            got.push(String::from_utf8(k.to_vec()).unwrap());
        }
    }
    got.sort();
    assert_eq!(got, vec!["k00", "k01", "k02"]);
}

#[tokio::test]
async fn test_inmemory_iter_range_stream_unbounded_upper() {
    let store = InMemoryStore::new();
    for i in 0..5 {
        let key = Bytes::from(format!("k{:02}", i));
        store.set(key, Bytes::from(format!("v{i}"))).await.unwrap();
    }
    let stream = store.iter_range_stream(Some(Bytes::from("k02")), None, 100);
    let mut got: Vec<String> = Vec::new();
    futures::pin_mut!(stream);
    while let Some(batch) = stream.next().await {
        for (k, _) in batch.unwrap() {
            got.push(String::from_utf8(k.to_vec()).unwrap());
        }
    }
    got.sort();
    assert_eq!(got, vec!["k02", "k03", "k04"]);
}

#[tokio::test]
async fn test_inmemory_iter_range_stream_unbounded_both() {
    let store = InMemoryStore::new();
    for i in 0..3 {
        let key = Bytes::from(format!("k{i}"));
        store.set(key, Bytes::from(format!("v{i}"))).await.unwrap();
    }
    let stream = store.iter_range_stream(None, None, 100);
    let mut got: Vec<String> = Vec::new();
    futures::pin_mut!(stream);
    while let Some(batch) = stream.next().await {
        for (k, _) in batch.unwrap() {
            got.push(String::from_utf8(k.to_vec()).unwrap());
        }
    }
    got.sort();
    assert_eq!(got, vec!["k0", "k1", "k2"], "fully unbounded = full scan");
}

#[tokio::test]
async fn test_inmemory_iter_range_stream_empty_range() {
    let store = InMemoryStore::new();
    for i in 0..5 {
        store
            .set(Bytes::from(format!("k{i}")), Bytes::from("v"))
            .await
            .unwrap();
    }
    // Bounds outside the data — no matches.
    let stream = store.iter_range_stream(Some(Bytes::from("z0")), Some(Bytes::from("z9")), 100);
    let mut count = 0;
    futures::pin_mut!(stream);
    while let Some(batch) = stream.next().await {
        count += batch.unwrap().len();
    }
    assert_eq!(count, 0, "empty range yields nothing");
}

#[tokio::test]
async fn test_inmemory_iter_range_stream_batching() {
    let store = InMemoryStore::new();
    for i in 0..50 {
        store
            .set(
                Bytes::from(format!("k{i:03}")),
                Bytes::from(format!("v{i}")),
            )
            .await
            .unwrap();
    }
    // batch_size=10 over a 25-key window.
    let stream = store.iter_range_stream(Some(Bytes::from("k010")), Some(Bytes::from("k034")), 10);
    let mut total = 0;
    futures::pin_mut!(stream);
    while let Some(batch) = stream.next().await {
        let b = batch.unwrap();
        assert!(
            b.len() <= 10,
            "no batch exceeds requested size: {}",
            b.len()
        );
        total += b.len();
    }
    assert_eq!(total, 25, "25 keys in [k010..=k034]");
}

#[tokio::test]
async fn test_inmemory_concurrent_access() {
    use tokio::task::JoinSet;

    let store = Arc::new(InMemoryStore::new());
    let mut join_set = JoinSet::new();

    // Spawn 100 concurrent writes
    for i in 0..100 {
        let store_clone = store.clone();
        join_set.spawn(async move {
            let key = format!("key_{}", i);
            let value = Bytes::from(key.clone());
            store_clone.set(key.into(), value).await.unwrap();
        });
    }

    // Spawn 100 concurrent reads while writes are happening
    for i in 0..100 {
        let store_clone = store.clone();
        join_set.spawn(async move {
            let key = format!("key_{}", i);
            let _ = store_clone.get(key.into()).await;
        });
    }

    // All tasks should complete without deadlocking
    while let Some(result) = join_set.join_next().await {
        result.unwrap();
    }

    // Verify all writes succeeded
    let all_records = collect_stream(store.iter_stream(1000)).await.unwrap();
    assert_eq!(all_records.len(), 100);
}

#[tokio::test]
async fn transact_empty_ops_is_noop() {
    let store = InMemoryStore::new();
    store.transact(vec![]).await.unwrap();
    // nothing should appear
    let stream = store.iter_stream(10);
    futures::pin_mut!(stream);
    let mut total = 0usize;
    while let Some(batch) = stream.next().await {
        total += batch.unwrap().len();
    }
    assert_eq!(total, 0);
}

#[tokio::test]
async fn transact_applies_set_ops_in_order() {
    let store = InMemoryStore::new();
    let k1 = RecordKey::from(b"k1".to_vec());
    let k2 = RecordKey::from(b"k2".to_vec());
    store
        .transact(vec![
            KvOp::Set(k1.clone(), Bytes::from_static(b"v1")),
            KvOp::Set(k2.clone(), Bytes::from_static(b"v2")),
        ])
        .await
        .unwrap();
    assert_eq!(store.get(k1).await.unwrap(), Bytes::from_static(b"v1"));
    assert_eq!(store.get(k2).await.unwrap(), Bytes::from_static(b"v2"));
}

#[tokio::test]
async fn raw_backend_default_is_none() {
    let s: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    assert!(s.raw_backend().await.is_none());
}

#[tokio::test]
async fn transact_mixed_set_remove() {
    let store = InMemoryStore::new();
    let k1 = RecordKey::from(b"k1".to_vec());
    let k2 = RecordKey::from(b"k2".to_vec());
    let k3 = RecordKey::from(b"k3".to_vec());

    // Seed state
    store
        .set(k1.clone(), Bytes::from_static(b"old1"))
        .await
        .unwrap();
    store
        .set(k2.clone(), Bytes::from_static(b"old2"))
        .await
        .unwrap();

    // Mixed batch: update k1, remove k2, insert k3
    store
        .transact(vec![
            KvOp::Set(k1.clone(), Bytes::from_static(b"new1")),
            KvOp::Remove(k2.clone()),
            KvOp::Set(k3.clone(), Bytes::from_static(b"new3")),
        ])
        .await
        .unwrap();

    assert_eq!(store.get(k1).await.unwrap(), Bytes::from_static(b"new1"));
    assert!(store.get(k2).await.is_err()); // removed
    assert_eq!(store.get(k3).await.unwrap(), Bytes::from_static(b"new3"));
}
