#![allow(deprecated)]

use crate::storage_redb::RedbRepo;
use crate::tests::types_tests::{collect_stream, run_batch_store_tests};
use crate::types::{KvOp, RecordKey, Repo, Store};
use bytes::Bytes;
use futures::StreamExt;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::fs;
use std::path::Path;
use tokio::time::{sleep, Duration};

async fn run_store_tests(store: &dyn Store) {
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
async fn test_redb_repo_basic() {
    let path = "./test_data/redb_repo_basic/db.redb";
    if let Some(parent) = Path::new(path).parent() {
        if parent.exists() {
            fs::remove_dir_all(parent).unwrap();
        }
    }

    let repo = RedbRepo::new(path).unwrap();
    let store = repo.store_get("test_table").await.unwrap();

    run_store_tests(store.as_ref()).await;

    assert!(repo.store_delete("test_table").await.unwrap());
}

#[tokio::test]
async fn test_redb_batch_ops() {
    let path = "./test_data/redb_batch_ops/db.redb";
    if let Some(parent) = Path::new(path).parent() {
        if parent.exists() {
            fs::remove_dir_all(parent).unwrap();
        }
        std::fs::create_dir_all(parent).unwrap();
    }
    let repo = RedbRepo::new(path).unwrap();
    let store = repo.store_get("batch").await.unwrap();
    run_batch_store_tests(store).await;
}

#[tokio::test]
async fn test_redb_iter_stream() {
    let path = "./test_data/redb_iter_stream/db.redb";
    if let Some(parent) = Path::new(path).parent() {
        if parent.exists() {
            fs::remove_dir_all(parent).unwrap();
        }
    }

    let repo = RedbRepo::new(path).unwrap();
    let store = repo.store_get("test_table").await.unwrap();

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
        println!("Batch {} has {} records", batch_count, batch.len());
        all_records.extend(batch);
    }

    assert_eq!(all_records.len(), 25);
    assert_eq!(batch_count, 3); // 10 + 10 + 5 = 25

    // Verify all keys are present
    for key in &expected_keys {
        assert!(all_records.iter().any(|(rec_key, _)| rec_key == key));
    }
}

/// Native `iter_range_stream` on redb — exercises the
/// `table.range((Included, Included))` path.
#[tokio::test]
async fn test_redb_transact_atomic() {
    let path = "./test_data/redb_transact/db.redb";
    if let Some(parent) = Path::new(path).parent() {
        if parent.exists() {
            fs::remove_dir_all(parent).unwrap();
        }
    }
    let repo = RedbRepo::new(path).unwrap();
    let store = repo.store_get("transact_test").await.unwrap();

    // Seed
    let k1: RecordKey = Bytes::from_static(b"k1");
    let k2: RecordKey = Bytes::from_static(b"k2");
    store
        .set(k1.clone(), Bytes::from_static(b"old1"))
        .await
        .unwrap();
    store
        .set(k2.clone(), Bytes::from_static(b"old2"))
        .await
        .unwrap();

    // Spawn observer that reads both keys repeatedly within a single
    // `get_many` call — that uses one read_txn, so we observe an
    // atomic snapshot. Two separate `get` calls would each open
    // their own read_txn and could legitimately straddle the commit
    // boundary; that's redb's documented snapshot semantics, not a
    // transact bug.
    let obs_store = repo.store_get("transact_test").await.unwrap();
    let k1c = k1.clone();
    let k2c = k2.clone();
    let observer = tokio::spawn(async move {
        for _ in 0..50 {
            let pair = obs_store
                .get_many(vec![k1c.clone(), k2c.clone()])
                .await
                .unwrap();
            if let (Some(va), Some(vb)) = (&pair[0], &pair[1]) {
                let a_new = va.as_ref() == b"new1";
                let b_new = vb.as_ref() == b"new2";
                assert_eq!(
                    a_new, b_new,
                    "partial state observed in single snapshot: a={:?} b={:?}",
                    va, vb
                );
            }
            tokio::task::yield_now().await;
        }
    });

    // Transact
    store
        .transact(vec![
            KvOp::Set(k1.clone(), Bytes::from_static(b"new1")),
            KvOp::Set(k2.clone(), Bytes::from_static(b"new2")),
        ])
        .await
        .unwrap();

    observer.await.unwrap();

    assert_eq!(store.get(k1).await.unwrap().as_ref(), b"new1");
    assert_eq!(store.get(k2).await.unwrap().as_ref(), b"new2");

    fs::remove_dir_all(Path::new(path).parent().unwrap()).ok();
}

#[tokio::test]
async fn test_redb_iter_range_stream_native() {
    let path = "./test_data/redb_iter_range/db.redb";
    if let Some(parent) = Path::new(path).parent() {
        if parent.exists() {
            fs::remove_dir_all(parent).unwrap();
        }
    }
    let repo = RedbRepo::new(path).unwrap();
    let store = repo.store_get("range_test").await.unwrap();

    // Seed keys "k00".."k19" (sortable ASCII) — set() takes explicit keys.
    for i in 0..20 {
        let key = Bytes::from(format!("k{i:02}"));
        let val = Bytes::from(format!("v{i}"));
        store.set(key, val).await.unwrap();
    }

    // Closed range [k05 ..= k10]
    let stream = store.iter_range_stream(Some(Bytes::from("k05")), Some(Bytes::from("k10")), 100);
    let mut got: Vec<String> = Vec::new();
    futures::pin_mut!(stream);
    while let Some(batch) = stream.next().await {
        for (k, _) in batch.unwrap() {
            got.push(String::from_utf8(k.to_vec()).unwrap());
        }
    }
    got.sort();
    assert_eq!(got, vec!["k05", "k06", "k07", "k08", "k09", "k10"]);

    // Unbounded upper.
    let stream = store.iter_range_stream(Some(Bytes::from("k17")), None, 100);
    let mut got: Vec<String> = Vec::new();
    futures::pin_mut!(stream);
    while let Some(batch) = stream.next().await {
        for (k, _) in batch.unwrap() {
            got.push(String::from_utf8(k.to_vec()).unwrap());
        }
    }
    got.sort();
    assert_eq!(got, vec!["k17", "k18", "k19"]);

    // Empty range — no overlap.
    let stream = store.iter_range_stream(Some(Bytes::from("z0")), Some(Bytes::from("z9")), 100);
    let mut count = 0;
    futures::pin_mut!(stream);
    while let Some(batch) = stream.next().await {
        count += batch.unwrap().len();
    }
    assert_eq!(count, 0);

    // Cursor advance across multiple batches.
    let stream = store.iter_range_stream(
        Some(Bytes::from("k00")),
        Some(Bytes::from("k19")),
        6, // 20 / 6 → 4 batches (6+6+6+2)
    );
    let mut total = 0;
    let mut batches = 0;
    futures::pin_mut!(stream);
    while let Some(batch) = stream.next().await {
        let b = batch.unwrap();
        assert!(b.len() <= 6);
        total += b.len();
        batches += 1;
    }
    assert_eq!(total, 20);
    assert!(batches >= 4, "expected ≥4 batches, got {batches}");

    fs::remove_dir_all(Path::new(path).parent().unwrap()).ok();
}
