//! Concurrent access tests for Table

use crate::db::engine::table::Table;
use crate::db::storage::storage_sled::SledRepo;
use crate::types::common::new_map;
use crate::types::record_id::RecordId;
use crate::types::value::UserValue;
use std::sync::Arc;

async fn create_test_table() -> (Table<SledRepo>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test_db");
    let repo = Arc::new(SledRepo::new(path).unwrap());
    let table = Table::new(repo, "users".to_string()).await.unwrap();
    (table, dir)
}

#[tokio::test]
async fn test_concurrent_inserts() {
    let (table, _dir) = create_test_table().await;

    let num_threads = 20;
    let records_per_thread = 10;
    let mut handles = vec![];

    for thread_id in 0..num_threads {
        let table_clone = table.clone();
        handles.push(tokio::spawn(async move {
            let mut ids = vec![];
            for i in 0..records_per_thread {
                let mut data = new_map();
                data.insert("thread".to_string(), UserValue::Int(thread_id));
                data.insert("index".to_string(), UserValue::Int(i));
                data.insert("name".to_string(), UserValue::Str(format!("User_{}_{}", thread_id, i)));
                let value = UserValue::Map(data);
                let id = table_clone.insert(&value).await.unwrap();
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
    let count = table.count().await.unwrap();
    assert_eq!(count, (num_threads * records_per_thread) as usize);
}

#[tokio::test]
async fn test_concurrent_insert_and_read() {
    let (table, _dir) = create_test_table().await;

    let num_inserters = 10;
    let num_readers = 10;
    let mut handles = vec![];

    // Inserters
    for i in 0..num_inserters {
        let table_clone = table.clone();
        handles.push(tokio::spawn(async move {
            for j in 0..20 {
                let mut data = new_map();
                data.insert("key".to_string(), UserValue::Str(format!("value_{}_{}", i, j)));
                data.insert("num".to_string(), UserValue::Int(i * 20 + j));
                table_clone.insert(&UserValue::Map(data)).await.unwrap();
            }
        }));
    }

    // Readers (may read while inserts happen)
    for _ in 0..num_readers {
        let table_clone = table.clone();
        handles.push(tokio::spawn(async move {
            for _ in 0..10 {
                let _ = table_clone.list().await;
                let _ = table_clone.count().await;
            }
        }));
    }

    // Wait for all
    for handle in handles {
        handle.await.unwrap();
    }

    // Verify final count
    let count = table.count().await.unwrap();
    assert_eq!(count, (num_inserters * 20) as usize);
}

#[tokio::test]
async fn test_concurrent_same_keys_interning() {
    let (table, _dir) = create_test_table().await;

    let num_threads = 50;
    let mut handles = vec![];

    // All threads insert records with same keys
    for i in 0..num_threads {
        let table_clone = table.clone();
        handles.push(tokio::spawn(async move {
            for j in 0..10 {
                let mut data = new_map();
                // Same keys across all threads
                data.insert("name".to_string(), UserValue::Str(format!("User_{}", i)));
                data.insert("age".to_string(), UserValue::Int(i));
                data.insert("email".to_string(), UserValue::Str(format!("user{}@test.com", i)));
                data.insert("index".to_string(), UserValue::Int(j));
                table_clone.insert(&UserValue::Map(data)).await.unwrap();
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // Verify all records are correct
    let records = table.list().await.unwrap();
    assert_eq!(records.len(), (num_threads * 10) as usize);

    // All records should have same 4 keys (name, age, email, index)
    for (_id, value) in records {
        match value {
            UserValue::Map(m) => {
                assert_eq!(m.len(), 4);
                assert!(m.contains_key("name"));
                assert!(m.contains_key("age"));
                assert!(m.contains_key("email"));
                assert!(m.contains_key("index"));
            }
            _ => panic!("Expected Map"),
        }
    }
}

#[tokio::test]
async fn test_concurrent_updates() {
    let (table, _dir) = create_test_table().await;

    // Insert initial record
    let mut data = new_map();
    data.insert("counter".to_string(), UserValue::Int(0));
    let id = table.insert(&UserValue::Map(data)).await.unwrap();

    let num_threads = 20;
    let mut handles = vec![];

    // All threads update same record
    for _ in 0..num_threads {
        let table_clone = table.clone();
        handles.push(tokio::spawn(async move {
            for i in 0..5 {
                let mut data = new_map();
                data.insert("counter".to_string(), UserValue::Int(i));
                data.insert("thread".to_string(), UserValue::Str("test".to_string()));
                let _ = table_clone.update(id, &UserValue::Map(data)).await;
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // Final record should exist
    let final_record = table.get(id).await.unwrap();
    match final_record {
        UserValue::Map(m) => {
            assert!(m.contains_key("counter"));
            assert!(m.contains_key("thread"));
        }
        _ => panic!("Expected Map"),
    }
}

#[tokio::test]
async fn test_concurrent_clone_and_operations() {
    let (table, _dir) = create_test_table().await;

    let num_threads = 30;
    let mut handles = vec![];

    for i in 0..num_threads {
        let table_clone = table.clone();
        handles.push(tokio::spawn(async move {
            // Each thread does different operations
            match i % 4 {
                0 => {
                    // Insert
                    let mut data = new_map();
                    data.insert("op".to_string(), UserValue::Str("insert".to_string()));
                    data.insert("num".to_string(), UserValue::Int(i));
                    table_clone.insert(&UserValue::Map(data)).await.unwrap();
                }
                1 => {
                    // List
                    let _ = table_clone.list().await;
                }
                2 => {
                    // Count
                    let _ = table_clone.count().await;
                }
                3 => {
                    // Insert then get
                    let mut data = new_map();
                    data.insert("op".to_string(), UserValue::Str("insert_get".to_string()));
                    let id = table_clone.insert(&UserValue::Map(data)).await.unwrap();
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
    let count = table.count().await.unwrap();
    assert!(count > 0);
}

#[tokio::test]
async fn test_concurrent_delete() {
    let (table, _dir) = create_test_table().await;

    // Insert some records
    let mut ids = vec![];
    for i in 0..20 {
        let mut data = new_map();
        data.insert("id".to_string(), UserValue::Int(i));
        let id = table.insert(&UserValue::Map(data)).await.unwrap();
        ids.push(id);
    }

    // Delete concurrently
    let _num_threads = 10;
    let mut handles = vec![];
    for chunk in ids.chunks(2) {
        let table_clone = table.clone();
        let chunk_ids = chunk.to_vec();
        handles.push(tokio::spawn(async move {
            for id in chunk_ids {
                table_clone.delete(id).await.unwrap();
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // All should be deleted
    let count = table.count().await.unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn test_counter_with_concurrent_operations() {
    let (table, _dir) = create_test_table().await;

    let num_threads = 50;
    let mut handles = vec![];

    for i in 0..num_threads {
        let table_clone = table.clone();
        handles.push(tokio::spawn(async move {
            for j in 0..10 {
                match i % 3 {
                    0 => {
                        // Insert
                        let mut data = new_map();
                        data.insert("val".to_string(), UserValue::Int(i * 10 + j));
                        table_clone.insert(&UserValue::Map(data)).await.unwrap();
                    }
                    1 => {
                        // Delete
                        let id = RecordId::new();
                        let _ = table_clone.delete(id).await;
                    }
                    2 => {
                        // Count
                        let _ = table_clone.count().await;
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
    let count = table.count().await.unwrap();
    let actual = table.list().await.unwrap().len();
    assert_eq!(count, actual);
}
