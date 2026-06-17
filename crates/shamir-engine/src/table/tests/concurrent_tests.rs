//! Concurrent access tests for Table
#![allow(deprecated)] // collect_list_stream is deprecated test-only utility

use crate::table::interner_manager::InternerManager;
use crate::table::record_counter::RecordCounter;
use crate::table::tests::stream_utils::collect_list_stream;
use crate::table::tests::test_helpers::query_value_to_inner_tracked;
use crate::table::Table;
use shamir_storage::storage_sled::SledRepo;
use shamir_storage::types::Repo;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};
use std::sync::Arc;

async fn create_test_table() -> (
    Table,
    InternerManager,
    Arc<RecordCounter>,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test_db");
    let repo = Arc::new(SledRepo::new(path).unwrap());
    let table_name = "users";

    let data_store = repo.store_get(table_name.to_string()).await.unwrap();
    let info_store: Arc<dyn shamir_storage::types::Store> = repo
        .store_get(format!("__info__{}", table_name))
        .await
        .unwrap();
    let table = Table::new(data_store);
    let interner = InternerManager::new(info_store.clone());
    let counter = Arc::new(RecordCounter::new(info_store));

    (table, interner, counter, dir)
}

/// Helper to intern a QueryValue and save new keys
async fn intern_value(value: &QueryValue, interner: &InternerManager) -> InnerValue {
    let inter = interner.get().await.unwrap();
    let (inner_value, new_keys) = query_value_to_inner_tracked(value, inter).unwrap();

    if !new_keys.is_empty() {
        interner.save_new_keys(&new_keys).await.unwrap();
    }

    inner_value
}

#[tokio::test]
async fn test_concurrent_inserts() {
    let (table, interner, counter, _dir) = create_test_table().await;

    let num_threads = 20;
    let records_per_thread = 10;
    let mut handles = vec![];

    for thread_id in 0..num_threads {
        let table_clone = table.clone();
        let interner_clone = interner.clone();
        let counter_clone = counter.clone();
        handles.push(tokio::spawn(async move {
            let mut ids = vec![];
            for i in 0..records_per_thread {
                let mut data = new_map();
                data.insert("thread".to_string(), QueryValue::Int(thread_id));
                data.insert("index".to_string(), QueryValue::Int(i));
                data.insert(
                    "name".to_string(),
                    QueryValue::Str(format!("User_{}_{}", thread_id, i)),
                );
                let value = QueryValue::Map(data);
                let inner = intern_value(&value, &interner_clone).await;
                let id = table_clone.insert(&inner).await.unwrap();
                counter_clone.increment(1).await.unwrap();
                ids.push(id);
            }
            ids
        }));
    }

    // Collect all IDs
    let mut all_ids = vec![];
    for handle in handles {
        let ids = handle.await.unwrap();
        all_ids.extend(ids);
    }

    assert_eq!(all_ids.len(), (num_threads * records_per_thread) as usize);

    // Verify all records can be retrieved
    let count = counter.get().await.unwrap() as usize;
    assert_eq!(count, (num_threads * records_per_thread) as usize);
}

#[tokio::test]
async fn test_concurrent_insert_and_read() {
    let (table, interner, counter, _dir) = create_test_table().await;

    let num_inserters = 10;
    let num_readers = 10;
    let mut handles = vec![];

    // Inserters
    for i in 0..num_inserters {
        let table_clone = table.clone();
        let interner_clone = interner.clone();
        let counter_clone = counter.clone();
        handles.push(tokio::spawn(async move {
            for j in 0..20 {
                let mut data = new_map();
                data.insert(
                    "key".to_string(),
                    QueryValue::Str(format!("value_{}_{}", i, j)),
                );
                data.insert("num".to_string(), QueryValue::Int(i * 20 + j));
                let inner = intern_value(&QueryValue::Map(data), &interner_clone).await;
                table_clone.insert(&inner).await.unwrap();
                counter_clone.increment(1).await.unwrap();
            }
        }));
    }

    // Readers (may read while inserts happen)
    for _ in 0..num_readers {
        let table_clone = table.clone();
        let counter_clone = counter.clone();
        handles.push(tokio::spawn(async move {
            for _ in 0..10 {
                // Just verify streaming works without panic
                let _ = collect_list_stream(&table_clone).await;
                let _ = counter_clone.get().await;
            }
        }));
    }

    // Wait for all
    for handle in handles {
        handle.await.unwrap();
    }

    // Verify final count
    let count = counter.get().await.unwrap() as usize;
    assert_eq!(count, (num_inserters * 20) as usize);
}

#[tokio::test]
async fn test_concurrent_same_keys_interning() {
    let (table, interner, counter, _dir) = create_test_table().await;

    let num_threads = 50;
    let mut handles = vec![];

    // All threads insert records with same keys
    for i in 0..num_threads {
        let table_clone = table.clone();
        let interner_clone = interner.clone();
        let counter_clone = counter.clone();
        handles.push(tokio::spawn(async move {
            for j in 0..10 {
                let mut data = new_map();
                // Same keys across all threads
                data.insert("name".to_string(), QueryValue::Str(format!("User_{}", i)));
                data.insert("age".to_string(), QueryValue::Int(i));
                data.insert(
                    "email".to_string(),
                    QueryValue::Str(format!("user{}@test.com", i)),
                );
                data.insert("index".to_string(), QueryValue::Int(j));
                let inner = intern_value(&QueryValue::Map(data), &interner_clone).await;
                table_clone.insert(&inner).await.unwrap();
                counter_clone.increment(1).await.unwrap();
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // Verify all records are correct
    let records = collect_list_stream(&table).await.unwrap();
    assert_eq!(records.len(), (num_threads * 10) as usize);

    // All records should have same 4 keys (name, age, email, index)
    let interner = interner.get().await.unwrap();
    let name_key = interner.touch_ind("name").unwrap().into_key();
    let age_key = interner.touch_ind("age").unwrap().into_key();
    let email_key = interner.touch_ind("email").unwrap().into_key();
    let index_key = interner.touch_ind("index").unwrap().into_key();

    for (_id, value) in records {
        match value {
            InnerValue::Map(m) => {
                assert_eq!(m.len(), 4);
                assert!(m.contains_key(&name_key));
                assert!(m.contains_key(&age_key));
                assert!(m.contains_key(&email_key));
                assert!(m.contains_key(&index_key));
            }
            _ => panic!("Expected Map"),
        }
    }
}

#[tokio::test]
async fn test_concurrent_updates() {
    let (table, interner, counter, _dir) = create_test_table().await;

    // Insert initial record
    let mut data = new_map();
    data.insert("counter".to_string(), QueryValue::Int(0));
    let inner = intern_value(&QueryValue::Map(data), &interner).await;
    let id = table.insert(&inner).await.unwrap();
    counter.increment(1).await.unwrap();

    let num_threads = 20;
    let mut handles = vec![];

    // All threads update same record
    for _ in 0..num_threads {
        let table_clone = table.clone();
        let interner_clone = interner.clone();
        handles.push(tokio::spawn(async move {
            for i in 0..5 {
                let mut data = new_map();
                data.insert("counter".to_string(), QueryValue::Int(i));
                data.insert("thread".to_string(), QueryValue::Str("test".to_string()));
                let inner = intern_value(&QueryValue::Map(data), &interner_clone).await;
                let _ = table_clone.update(id, &inner).await;
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // Final record should exist
    let final_record = table.get(id).await.unwrap();
    match final_record {
        InnerValue::Map(m) => {
            let interner = interner.get().await.unwrap();
            let counter_key = interner.touch_ind("counter").unwrap().into_key();
            let thread_key = interner.touch_ind("thread").unwrap().into_key();
            assert!(m.contains_key(&counter_key));
            assert!(m.contains_key(&thread_key));
        }
        _ => panic!("Expected Map"),
    }
}

#[tokio::test]
async fn test_concurrent_clone_and_operations() {
    let (table, interner, counter, _dir) = create_test_table().await;

    let num_threads = 30;
    let mut handles = vec![];

    for i in 0..num_threads {
        let table_clone = table.clone();
        let interner_clone = interner.clone();
        let counter_clone = counter.clone();
        handles.push(tokio::spawn(async move {
            // Each thread does different operations
            match i % 4 {
                0 => {
                    // Insert
                    let mut data = new_map();
                    data.insert("op".to_string(), QueryValue::Str("insert".to_string()));
                    data.insert("num".to_string(), QueryValue::Int(i));
                    let inner = intern_value(&QueryValue::Map(data), &interner_clone).await;
                    table_clone.insert(&inner).await.unwrap();
                    counter_clone.increment(1).await.unwrap();
                }
                1 => {
                    // List
                    let _ = collect_list_stream(&table_clone).await;
                }
                2 => {
                    // Count
                    let _ = counter_clone.get().await;
                }
                3 => {
                    // Insert then get
                    let mut data = new_map();
                    data.insert("op".to_string(), QueryValue::Str("insert_get".to_string()));
                    let inner = intern_value(&QueryValue::Map(data), &interner_clone).await;
                    let id = table_clone.insert(&inner).await.unwrap();
                    counter_clone.increment(1).await.unwrap();
                    let _ = table_clone.get(id).await;
                }
                _ => unreachable!(),
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // Should have inserted records
    let count = counter.get().await.unwrap() as usize;
    assert!(count > 0);
}

#[tokio::test]
async fn test_concurrent_delete() {
    let (table, interner, counter, _dir) = create_test_table().await;

    // Insert some records
    let mut ids = vec![];
    for i in 0..20 {
        let mut data = new_map();
        data.insert("id".to_string(), QueryValue::Int(i));
        let inner = intern_value(&QueryValue::Map(data), &interner).await;
        let id = table.insert(&inner).await.unwrap();
        ids.push(id);
        counter.increment(1).await.unwrap();
    }

    // Delete concurrently
    let _num_threads = 10;
    let mut handles = vec![];
    for chunk in ids.chunks(2) {
        let table_clone = table.clone();
        let chunk_ids = chunk.to_vec();
        let counter_clone = counter.clone();
        handles.push(tokio::spawn(async move {
            for id in chunk_ids {
                table_clone.delete(id).await.unwrap();
                counter_clone.increment(-1).await.unwrap();
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // All should be deleted
    let count = counter.get().await.unwrap() as usize;
    assert_eq!(count, 0);
}

#[tokio::test]
async fn test_counter_with_concurrent_operations() {
    let (table, interner, counter, _dir) = create_test_table().await;

    let num_threads = 50;
    let mut handles = vec![];

    for i in 0..num_threads {
        let table_clone = table.clone();
        let interner_clone = interner.clone();
        let counter_clone = counter.clone();
        handles.push(tokio::spawn(async move {
            for j in 0..10 {
                match i % 3 {
                    0 => {
                        // Insert
                        let mut data = new_map();
                        data.insert("val".to_string(), QueryValue::Int(i * 10 + j));
                        let inner = intern_value(&QueryValue::Map(data), &interner_clone).await;
                        table_clone.insert(&inner).await.unwrap();
                        counter_clone.increment(1).await.unwrap();
                    }
                    1 => {
                        // Delete (will fail for non-existent IDs, but that's ok)
                        let id = RecordId::new();
                        let _ = table_clone.delete(id).await;
                    }
                    2 => {
                        // Count
                        let _ = counter_clone.get().await;
                    }
                    _ => unreachable!(),
                }
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // Counter should still be accurate
    let count = counter.get().await.unwrap() as usize;
    let actual = collect_list_stream(&table).await.unwrap().len();
    assert_eq!(count, actual);
}
