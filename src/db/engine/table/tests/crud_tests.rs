//! CRUD tests for Table

use crate::db::engine::table::Table;
use crate::db::error::{DbError, DbResult};
use crate::db::storage::storage_sled::SledRepo;
use crate::types::common::new_map;
use crate::types::record_id::RecordId;
use crate::types::value::UserValue;
use std::sync::Arc;

async fn create_test_table() -> DbResult<(Table<SledRepo>, tempfile::TempDir)> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("test_db");
    let repo = Arc::new(SledRepo::new(path)?);
    let table = Table::new(repo, "users".to_string()).await?;

    Ok((table, dir))
}

#[tokio::test]
async fn test_table_insert_and_get() {
    let (table, _dir) = create_test_table().await.unwrap();

    let mut user_data = new_map();
    user_data.insert("name".to_string(), UserValue::Str("Alice".to_string()));
    user_data.insert("age".to_string(), UserValue::Int(30));
    user_data.insert("email".to_string(), UserValue::Str("alice@example.com".to_string()));
    let user_value = UserValue::Map(user_data);

    let id = table.insert(&user_value).await.unwrap();

    let retrieved = table.get(id).await.unwrap();
    assert_eq!(retrieved, user_value);
}

#[tokio::test]
async fn test_table_interning_persistence() {
    let (table, _dir) = create_test_table().await.unwrap();

    // Insert first record
    let mut data1 = new_map();
    data1.insert("name".to_string(), UserValue::Str("Bob".to_string()));
    let original1 = UserValue::Map(data1.clone());
    let id1 = table.insert(&original1).await.unwrap();

    // Insert second record with overlapping keys
    let mut data2 = new_map();
    data2.insert("name".to_string(), UserValue::Str("Charlie".to_string()));
    data2.insert("age".to_string(), UserValue::Int(25));
    let id2 = table.insert(&UserValue::Map(data2)).await.unwrap();

    // Verify both records
    let retrieved1 = table.get(id1).await.unwrap();
    assert_eq!(retrieved1, original1);

    let retrieved2 = table.get(id2).await.unwrap();
    // Check it has right data
    match retrieved2 {
        UserValue::Map(m) => {
            assert_eq!(m.get("name"), Some(&UserValue::Str("Charlie".to_string())));
            assert_eq!(m.get("age"), Some(&UserValue::Int(25)));
        }
        _ => panic!("Expected Map"),
    }
}

#[tokio::test]
async fn test_table_update() {
    let (table, _dir) = create_test_table().await.unwrap();

    let mut data = new_map();
    data.insert("name".to_string(), UserValue::Str("Dave".to_string()));
    let id = table.insert(&UserValue::Map(data.clone())).await.unwrap();

    // Update
    let mut updated = new_map();
    updated.insert("name".to_string(), UserValue::Str("David".to_string()));
    updated.insert("age".to_string(), UserValue::Int(40));

    let existed = table.update(id, &UserValue::Map(updated)).await.unwrap();
    assert!(existed);

    let retrieved = table.get(id).await.unwrap();
    match retrieved {
        UserValue::Map(m) => {
            assert_eq!(m.get("name"), Some(&UserValue::Str("David".to_string())));
            assert_eq!(m.get("age"), Some(&UserValue::Int(40)));
        }
        _ => panic!("Expected Map"),
    }
}

#[tokio::test]
async fn test_table_delete() {
    let (table, _dir) = create_test_table().await.unwrap();

    let mut data = new_map();
    data.insert("name".to_string(), UserValue::Str("Eve".to_string()));
    let id = table.insert(&UserValue::Map(data)).await.unwrap();

    let deleted = table.delete(id).await.unwrap();
    assert!(deleted);

    let get_result = table.get(id).await;
    assert!(matches!(get_result, Err(DbError::NotFound(_))));

    let deleted_again = table.delete(id).await.unwrap();
    assert!(!deleted_again);
}

#[tokio::test]
async fn test_table_list() {
    let (table, _dir) = create_test_table().await.unwrap();

    for i in 1..=3 {
        let mut data = new_map();
        data.insert("id".to_string(), UserValue::Int(i));
        data.insert("name".to_string(), UserValue::Str(format!("User{}", i)));
        table.insert(&UserValue::Map(data)).await.unwrap();
    }

    let records = table.list().await.unwrap();
    assert_eq!(records.len(), 3);
}

#[tokio::test]
async fn test_table_count() {
    let (table, _dir) = create_test_table().await.unwrap();

    assert_eq!(table.count().await.unwrap(), 0);

    for i in 1..=5 {
        let mut data = new_map();
        data.insert("id".to_string(), UserValue::Int(i));
        table.insert(&UserValue::Map(data)).await.unwrap();
    }

    assert_eq!(table.count().await.unwrap(), 5);
}

#[tokio::test]
async fn test_table_lazy_interner_loading() {
    let (table, _dir) = create_test_table().await.unwrap();

    // Interner should not be loaded yet
    // We can't check this directly, but we can verify behavior

    // First insert triggers lazy load
    let mut data = new_map();
    data.insert("first_key".to_string(), UserValue::Str("test".to_string()));
    table.insert(&UserValue::Map(data)).await.unwrap();

    // Clone table - should share same interner
    let table_clone = table.clone();

    // Use clone - should use same loaded interner
    let mut data2 = new_map();
    data2.insert("first_key".to_string(), UserValue::Str("test2".to_string()));
    data2.insert("second_key".to_string(), UserValue::Int(42));
    table_clone.insert(&UserValue::Map(data2)).await.unwrap();

    // Verify both records
    let records = table_clone.list().await.unwrap();
    assert_eq!(records.len(), 2);
}

#[tokio::test]
async fn test_table_with_nested_structures() {
    let (table, _dir) = create_test_table().await.unwrap();

    // Complex nested structure
    let mut inner_map = new_map();
    inner_map.insert("x".to_string(), UserValue::Int(10));
    inner_map.insert("y".to_string(), UserValue::Str("nested".to_string()));

    let list = vec![
        UserValue::Int(1),
        UserValue::Str("hello".to_string()),
        UserValue::Map(inner_map.clone()),
    ];

    let mut data = new_map();
    data.insert("list_data".to_string(), UserValue::List(list.clone()));
    data.insert("map_data".to_string(), UserValue::Map(inner_map));

    let id = table.insert(&UserValue::Map(data)).await.unwrap();

    // Retrieve and verify
    let retrieved = table.get(id).await.unwrap();

    match retrieved {
        UserValue::Map(m) => {
            match m.get("list_data") {
                Some(UserValue::List(l)) => {
                    assert_eq!(l.len(), 3);
                }
                _ => panic!("Expected list"),
            }
            match m.get("map_data") {
                Some(UserValue::Map(inner)) => {
                    assert_eq!(inner.len(), 2);
                    assert_eq!(inner.get("x"), Some(&UserValue::Int(10)));
                }
                _ => panic!("Expected map"),
            }
        }
        _ => panic!("Expected Map"),
    }
}

#[tokio::test]
async fn test_table_with_special_characters() {
    let (table, _dir) = create_test_table().await.unwrap();

    let special_keys = vec![
        "key with spaces",
        "key-with-dashes",
        "key_with_underscores",
        "key.with.dots",
        "key:with:colons",
        "ключ-русский",
        "🔑emoji-key",
    ];

    for key in &special_keys {
        let mut data = new_map();
        data.insert(key.to_string(), UserValue::Str("value".to_string()));
        table.insert(&UserValue::Map(data)).await.unwrap();
    }

    // Retrieve all and verify
    let records = table.list().await.unwrap();
    assert_eq!(records.len(), special_keys.len());

    for (_id, value) in records {
        match value {
            UserValue::Map(m) => {
                assert_eq!(m.len(), 1);
                let key = m.keys().next().unwrap();
                assert!(special_keys.contains(&key.as_str()));
            }
            _ => panic!("Expected Map"),
        }
    }
}

#[tokio::test]
async fn test_set_method_creates_new_record() {
    let (table, _dir) = create_test_table().await.unwrap();

    // Create a new RecordId
    let id = RecordId::new();

    let mut data = new_map();
    data.insert("name".to_string(), UserValue::Str("Alice".to_string()));
    data.insert("age".to_string(), UserValue::Int(30));
    let value = UserValue::Map(data);

    // set should create new record
    let created = table.set(id, &value).await.unwrap();
    assert!(created, "Should return true for new record");

    // Verify count increased
    assert_eq!(table.count().await.unwrap(), 1);

    // Verify record exists
    let retrieved = table.get(id).await.unwrap();
    assert_eq!(retrieved, value);
}

#[tokio::test]
async fn test_set_method_updates_existing_record() {
    let (table, _dir) = create_test_table().await.unwrap();

    // First insert a record
    let id = RecordId::new();
    let mut data1 = new_map();
    data1.insert("name".to_string(), UserValue::Str("Bob".to_string()));
    data1.insert("age".to_string(), UserValue::Int(25));
    let value1 = UserValue::Map(data1);

    let created = table.set(id, &value1).await.unwrap();
    assert!(created);
    assert_eq!(table.count().await.unwrap(), 1);

    // Now update with set
    let mut data2 = new_map();
    data2.insert("name".to_string(), UserValue::Str("Robert".to_string()));
    data2.insert("age".to_string(), UserValue::Int(26));
    data2.insert("city".to_string(), UserValue::Str("NYC".to_string()));
    let value2 = UserValue::Map(data2);

    let created_again = table.set(id, &value2).await.unwrap();
    assert!(!created_again, "Should return false for update");

    // Count should still be 1 (not incremented)
    assert_eq!(table.count().await.unwrap(), 1);

    // Verify updated value
    let retrieved = table.get(id).await.unwrap();
    assert_eq!(retrieved, value2);
}

#[tokio::test]
async fn test_record_counter_with_insert_and_delete() {
    let (table, _dir) = create_test_table().await.unwrap();

    // Initial count should be 0
    assert_eq!(table.count().await.unwrap(), 0);

    // Insert 5 records
    let mut ids = vec![];
    for i in 0..5 {
        let mut data = new_map();
        data.insert("id".to_string(), UserValue::Int(i));
        let id = table.insert(&UserValue::Map(data)).await.unwrap();
        ids.push(id);
    }

    assert_eq!(table.count().await.unwrap(), 5);

    // Delete 2 records
    table.delete(ids[0]).await.unwrap();
    table.delete(ids[1]).await.unwrap();

    assert_eq!(table.count().await.unwrap(), 3);

    // Delete 1 more
    table.delete(ids[2]).await.unwrap();

    assert_eq!(table.count().await.unwrap(), 2);

    // Insert 3 more
    for i in 0..3 {
        let mut data = new_map();
        data.insert("new_id".to_string(), UserValue::Int(i));
        table.insert(&UserValue::Map(data)).await.unwrap();
    }

    assert_eq!(table.count().await.unwrap(), 5);
}

#[tokio::test]
async fn test_set_method_respects_counter() {
    let (table, _dir) = create_test_table().await.unwrap();

    assert_eq!(table.count().await.unwrap(), 0);

    let id1 = RecordId::new();
    let id2 = RecordId::new();

    let mut data = new_map();
    data.insert("value".to_string(), UserValue::Int(42));

    // Create first record with set
    let created1 = table.set(id1, &UserValue::Map(data.clone())).await.unwrap();
    assert!(created1);
    assert_eq!(table.count().await.unwrap(), 1);

    // Create second record with set
    let created2 = table.set(id2, &UserValue::Map(data.clone())).await.unwrap();
    assert!(created2);
    assert_eq!(table.count().await.unwrap(), 2);

    // Update first record with set (count should not change)
    let updated = table.set(id1, &UserValue::Map(data.clone())).await.unwrap();
    assert!(!updated);
    assert_eq!(table.count().await.unwrap(), 2);

    // Update second record with set (count should not change)
    let updated2 = table.set(id2, &UserValue::Map(data.clone())).await.unwrap();
    assert!(!updated2);
    assert_eq!(table.count().await.unwrap(), 2);
}
