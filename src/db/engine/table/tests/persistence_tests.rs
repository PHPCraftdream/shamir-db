//! Persistence tests for Table

use crate::db::engine::table::Table;
use crate::db::storage::storage_sled::SledRepo;
use crate::types::common::new_map;
use crate::types::record_id::RecordId;
use crate::types::value::UserValue;
use std::sync::Arc;

#[tokio::test]
async fn test_interner_persistence_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test_persistence_db");
    let table_name = "users";

    // === First session: write data ===
    let repo1 = Arc::new(SledRepo::new(path.clone()).unwrap());
    let table1 = Table::new(repo1.clone(), table_name.to_string()).await.unwrap();

    // Insert multiple records with overlapping keys to test interning
    let mut data1 = new_map();
    data1.insert("name".to_string(), UserValue::Str("Alice".to_string()));
    data1.insert("email".to_string(), UserValue::Str("alice@example.com".to_string()));
    data1.insert("age".to_string(), UserValue::Int(30));
    let value1 = UserValue::Map(data1);

    let id1 = table1.insert(&value1).await.unwrap();

    // Insert second record with same keys (should reuse interner entries)
    let mut data2 = new_map();
    data2.insert("name".to_string(), UserValue::Str("Bob".to_string()));
    data2.insert("email".to_string(), UserValue::Str("bob@example.com".to_string()));
    data2.insert("age".to_string(), UserValue::Int(25));
    let value2 = UserValue::Map(data2);

    let id2 = table1.insert(&value2).await.unwrap();

    // Verify records in first session
    let retrieved1 = table1.get(id1).await.unwrap();
    assert_eq!(retrieved1, value1);

    let retrieved2 = table1.get(id2).await.unwrap();
    assert_eq!(retrieved2, value2);

    let count1 = table1.count().await.unwrap();
    assert_eq!(count1, 2);

    // table1 and repo1 are dropped here, closing the database
    drop(table1);
    drop(repo1);

    // === Second session: reopen and verify ===
    let repo2 = Arc::new(SledRepo::new(path).unwrap());
    let table2 = Table::new(repo2, table_name.to_string()).await.unwrap();

    // Verify records are still there after restart
    let retrieved1_after = table2.get(id1).await.unwrap();
    assert_eq!(retrieved1_after, value1, "First record should match after restart");

    let retrieved2_after = table2.get(id2).await.unwrap();
    assert_eq!(retrieved2_after, value2, "Second record should match after restart");

    // Verify count
    let count2 = table2.count().await.unwrap();
    assert_eq!(count2, 2, "Should have 2 records after restart");

    // Insert new record with same keys (should reuse restored interner entries)
    let mut data3 = new_map();
    data3.insert("name".to_string(), UserValue::Str("Charlie".to_string()));
    data3.insert("email".to_string(), UserValue::Str("charlie@example.com".to_string()));
    data3.insert("age".to_string(), UserValue::Int(35));
    let value3 = UserValue::Map(data3);

    let id3 = table2.insert(&value3).await.unwrap();

    // Verify all three records
    let retrieved3 = table2.get(id3).await.unwrap();
    assert_eq!(retrieved3, value3);

    let count3 = table2.count().await.unwrap();
    assert_eq!(count3, 3, "Should have 3 records after inserting in second session");

    // List all records and verify
    let all_records = table2.list().await.unwrap();
    assert_eq!(all_records.len(), 3);

    // Verify each record has correct structure
    for (_id, value) in all_records {
        match value {
            UserValue::Map(m) => {
                assert!(m.contains_key("name"), "Should have 'name' key");
                assert!(m.contains_key("email"), "Should have 'email' key");
                assert!(m.contains_key("age"), "Should have 'age' key");
                assert_eq!(m.len(), 3, "Should have exactly 3 keys");
            }
            _ => panic!("Expected Map"),
        }
    }
}

#[tokio::test]
async fn test_counter_persistence_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test_counter_persistence_db");
    let table_name = "counter_test";

    // === First session: insert records ===
    let repo1 = Arc::new(SledRepo::new(path.clone()).unwrap());
    let table1 = Table::new(repo1.clone(), table_name.to_string()).await.unwrap();

    // Insert multiple records
    for i in 0..5 {
        let mut data = new_map();
        data.insert("id".to_string(), UserValue::Int(i));
        data.insert("name".to_string(), UserValue::Str(format!("User{}", i)));
        table1.insert(&UserValue::Map(data)).await.unwrap();
    }

    let count1 = table1.count().await.unwrap();
    assert_eq!(count1, 5, "Should have 5 records in first session");

    // table1 and repo1 are dropped here
    drop(table1);
    drop(repo1);

    // === Second session: reopen and verify ===
    let repo2 = Arc::new(SledRepo::new(path.clone()).unwrap());
    let table2 = Table::new(repo2.clone(), table_name.to_string()).await.unwrap();

    let count2 = table2.count().await.unwrap();
    assert_eq!(count2, 5, "Counter should persist after restart");

    // Verify actual records match counter
    let records = table2.list().await.unwrap();
    assert_eq!(records.len(), 5, "Actual record count should match counter");

    // Insert more records
    for i in 5..10 {
        let mut data = new_map();
        data.insert("id".to_string(), UserValue::Int(i));
        data.insert("name".to_string(), UserValue::Str(format!("User{}", i)));
        table2.insert(&UserValue::Map(data)).await.unwrap();
    }

    let count3 = table2.count().await.unwrap();
    assert_eq!(count3, 10, "Counter should update correctly");

    // table2 and repo2 are dropped here
    drop(table2);
    drop(repo2);

    // === Third session: verify final state ===
    let repo3 = Arc::new(SledRepo::new(path.clone()).unwrap());
    let table3 = Table::new(repo3, table_name.to_string()).await.unwrap();

    let count4 = table3.count().await.unwrap();
    assert_eq!(count4, 10, "Counter should persist correctly after multiple restarts");

    // Verify counter matches actual record count
    let records = table3.list().await.unwrap();
    assert_eq!(records.len(), 10, "Counter should always match actual records");
}

#[tokio::test]
async fn test_counter_matches_actual_record_count() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test_counter_accuracy_db");
    let table_name = "accuracy_test";

    let repo = Arc::new(SledRepo::new(path).unwrap());
    let table = Table::new(repo.clone(), table_name.to_string()).await.unwrap();

    // Test after various operations
    let mut ids = vec![];

    // Initial state
    assert_eq!(table.count().await.unwrap(), 0);
    assert_eq!(table.list().await.unwrap().len(), 0);

    // Insert 10 records
    for i in 0..10 {
        let mut data = new_map();
        data.insert("id".to_string(), UserValue::Int(i));
        let id = table.insert(&UserValue::Map(data)).await.unwrap();
        ids.push(id);
    }

    assert_eq!(table.count().await.unwrap(), 10);
    assert_eq!(table.list().await.unwrap().len(), 10);

    // Delete 3 records
    table.delete(ids[0]).await.unwrap();
    table.delete(ids[1]).await.unwrap();
    table.delete(ids[2]).await.unwrap();

    assert_eq!(table.count().await.unwrap(), 7);
    assert_eq!(table.list().await.unwrap().len(), 7);

    // Update 2 records
    let mut data = new_map();
    data.insert("updated".to_string(), UserValue::Str("yes".to_string()));
    table.update(ids[3], &UserValue::Map(data.clone())).await.unwrap();
    table.update(ids[4], &UserValue::Map(data)).await.unwrap();

    // Count should not change on update
    assert_eq!(table.count().await.unwrap(), 7);
    assert_eq!(table.list().await.unwrap().len(), 7);

    // Insert 5 more
    for i in 10..15 {
        let mut data = new_map();
        data.insert("id".to_string(), UserValue::Int(i));
        table.insert(&UserValue::Map(data)).await.unwrap();
    }

    assert_eq!(table.count().await.unwrap(), 12);
    assert_eq!(table.list().await.unwrap().len(), 12);
}
