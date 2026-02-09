//! CRUD tests for Table

#![allow(deprecated)]

use crate::core::transform;
use crate::db::engine::table::Table;
use crate::db::engine::table::interner_manager::InternerManager;
use crate::db::engine::table::record_counter::RecordCounter;
use crate::db::error::{DbError, DbResult};
use crate::db::storage::storage_sled::SledRepo;
use crate::db::storage::types::Repo;
use crate::types::common::new_map;
use crate::types::record_id::RecordId;
use crate::types::value::{InnerValue, UserValue};
use std::sync::Arc;

async fn create_test_table() -> DbResult<(Table, InternerManager, Arc<RecordCounter>, tempfile::TempDir)> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("test_db");
    let repo = Arc::new(SledRepo::new(path)?);

    let data_store = repo.store_get("__data__users".to_string()).await?;
    let info_store = repo.store_get("__info__users".to_string()).await?;

    let data_store: Arc<dyn crate::db::storage::types::Store> = Arc::from(data_store);
    let info_store: Arc<dyn crate::db::storage::types::Store> = Arc::from(info_store);

    let table = Table::new(Arc::clone(&data_store));
    let counter = Arc::new(RecordCounter::new(Arc::clone(&info_store)));
    let interner = InternerManager::new(info_store);

    Ok((table, interner, counter, dir))
}

/// Helper to intern a UserValue and save new keys
async fn intern_value(value: &UserValue, interner: &InternerManager) -> DbResult<InnerValue> {
    let inter = interner.get().await?;
    let transform = transform::user_to_inner(value, inter);

    // Save new keys if any
    if let Some(ref new_keys) = transform.new_keys {
        interner.save_new_keys(new_keys).await?;
    }

    Ok(transform.inner_value)
}

#[tokio::test]
async fn test_table_insert_and_get() {
    let (table, interner, _counter, _dir) = create_test_table().await.unwrap();

    let mut user_data = new_map();
    user_data.insert("name".to_string(), UserValue::Str("Alice".to_string()));
    user_data.insert("age".to_string(), UserValue::Int(30));
    user_data.insert("email".to_string(), UserValue::Str("alice@example.com".to_string()));
    let user_value = UserValue::Map(user_data);

    // Intern the value
    let inner_value = intern_value(&user_value, &interner).await.unwrap();
    let id = table.insert(&inner_value).await.unwrap();

    let retrieved = table.get(id).await.unwrap();
    assert_eq!(retrieved, inner_value);
}

#[tokio::test]
async fn test_table_interning_persistence() {
    let (table, interner, _counter, _dir) = create_test_table().await.unwrap();

    // Insert first record
    let mut data1 = new_map();
    data1.insert("name".to_string(), UserValue::Str("Bob".to_string()));
    let original1 = UserValue::Map(data1.clone());
    let inner1 = intern_value(&original1, &interner).await.unwrap();
    let id1 = table.insert(&inner1).await.unwrap();

    // Insert second record with overlapping keys
    let mut data2 = new_map();
    data2.insert("name".to_string(), UserValue::Str("Charlie".to_string()));
    data2.insert("age".to_string(), UserValue::Int(25));
    let inner2 = intern_value(&UserValue::Map(data2), &interner).await.unwrap();
    let id2 = table.insert(&inner2).await.unwrap();

    // Verify both records
    let retrieved1 = table.get(id1).await.unwrap();
    assert_eq!(retrieved1, inner1);

    let retrieved2 = table.get(id2).await.unwrap();
    // Check it has right data (check interned keys)
    match retrieved2 {
        InnerValue::Map(m) => {
            let interner = interner.get().await.unwrap();
            let name_key = interner.touch_ind("name").unwrap().key().clone();
            let age_key = interner.touch_ind("age").unwrap().key().clone();
            assert_eq!(m.get(&name_key), Some(&InnerValue::Str("Charlie".to_string())));
            assert_eq!(m.get(&age_key), Some(&InnerValue::Int(25)));
        }
        _ => panic!("Expected Map"),
    }
}

#[tokio::test]
async fn test_table_update() {
    let (table, interner, _counter, _dir) = create_test_table().await.unwrap();

    let mut data = new_map();
    data.insert("name".to_string(), UserValue::Str("Dave".to_string()));
    let inner = intern_value(&UserValue::Map(data.clone()), &interner).await.unwrap();
    let id = table.insert(&inner).await.unwrap();

    // Update
    let mut updated = new_map();
    updated.insert("name".to_string(), UserValue::Str("David".to_string()));
    updated.insert("age".to_string(), UserValue::Int(40));
    let inner_updated = intern_value(&UserValue::Map(updated), &interner).await.unwrap();

    let existed = table.update(id, &inner_updated).await.unwrap();
    assert!(existed);

    let retrieved = table.get(id).await.unwrap();
    match retrieved {
        InnerValue::Map(m) => {
            let interner = interner.get().await.unwrap();
            let name_key = interner.touch_ind("name").unwrap().key().clone();
            let age_key = interner.touch_ind("age").unwrap().key().clone();
            assert_eq!(m.get(&name_key), Some(&InnerValue::Str("David".to_string())));
            assert_eq!(m.get(&age_key), Some(&InnerValue::Int(40)));
        }
        _ => panic!("Expected Map"),
    }
}

#[tokio::test]
async fn test_table_delete() {
    let (table, interner, _counter, _dir) = create_test_table().await.unwrap();

    let mut data = new_map();
    data.insert("name".to_string(), UserValue::Str("Eve".to_string()));
    let inner = intern_value(&UserValue::Map(data), &interner).await.unwrap();
    let id = table.insert(&inner).await.unwrap();

    let deleted = table.delete(id).await.unwrap();
    assert!(deleted);

    let get_result = table.get(id).await;
    assert!(matches!(get_result, Err(DbError::NotFound(_))));

    let deleted_again = table.delete(id).await.unwrap();
    assert!(!deleted_again);
}

#[tokio::test]
async fn test_table_list() {
    let (table, interner, _counter, _dir) = create_test_table().await.unwrap();

    for i in 1..=3 {
        let mut data = new_map();
        data.insert("id".to_string(), UserValue::Int(i));
        data.insert("name".to_string(), UserValue::Str(format!("User{}", i)));
        let inner = intern_value(&UserValue::Map(data), &interner).await.unwrap();
        table.insert(&inner).await.unwrap();
    }

    let records = table.list().await.unwrap();
    assert_eq!(records.len(), 3);
}

#[tokio::test]
async fn test_table_count() {
    let (table, interner, counter, _dir) = create_test_table().await.unwrap();

    assert_eq!(counter.get().await.unwrap() as usize, 0);

    for i in 1..=5 {
        let mut data = new_map();
        data.insert("id".to_string(), UserValue::Int(i));
        let inner = intern_value(&UserValue::Map(data), &interner).await.unwrap();
        table.insert(&inner).await.unwrap();
        counter.increment(1).await.unwrap();
    }

    assert_eq!(counter.get().await.unwrap() as usize, 5);
}

#[tokio::test]
async fn test_table_with_nested_structures() {
    let (table, interner, _counter, _dir) = create_test_table().await.unwrap();

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

    let inner = intern_value(&UserValue::Map(data), &interner).await.unwrap();
    let id = table.insert(&inner).await.unwrap();

    // Retrieve and verify
    let retrieved = table.get(id).await.unwrap();

    match retrieved {
        InnerValue::Map(m) => {
            match m.values().find(|v| matches!(v, InnerValue::List(_))) {
                Some(InnerValue::List(l)) => {
                    assert_eq!(l.len(), 3);
                }
                _ => panic!("Expected list"),
            }
            match m.values().find(|v| matches!(v, InnerValue::Map(_))) {
                Some(InnerValue::Map(inner)) => {
                    assert_eq!(inner.len(), 2);
                    assert!(inner.values().any(|v| matches!(v, InnerValue::Int(10))));
                }
                _ => panic!("Expected map"),
            }
        }
        _ => panic!("Expected Map"),
    }
}

#[tokio::test]
async fn test_table_with_special_characters() {
    let (table, interner, _counter, _dir) = create_test_table().await.unwrap();

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
        let inner = intern_value(&UserValue::Map(data), &interner).await.unwrap();
        table.insert(&inner).await.unwrap();
    }

    // Retrieve all and verify
    let records = table.list().await.unwrap();
    assert_eq!(records.len(), special_keys.len());

    for (_id, value) in records {
        match value {
            InnerValue::Map(m) => {
                assert_eq!(m.len(), 1);
                // Verify we have interned keys
                let interner = interner.get().await.unwrap();
                for key in m.keys() {
                    let original = interner.get_str(key).unwrap();
                    assert!(special_keys.contains(&original.as_str()));
                }
            }
            _ => panic!("Expected Map"),
        }
    }
}

#[tokio::test]
async fn test_set_method_creates_new_record() {
    let (table, interner, counter, _dir) = create_test_table().await.unwrap();

    // Create a new RecordId
    let id = RecordId::new();

    let mut data = new_map();
    data.insert("name".to_string(), UserValue::Str("Alice".to_string()));
    data.insert("age".to_string(), UserValue::Int(30));
    let inner = intern_value(&UserValue::Map(data), &interner).await.unwrap();

    // set should create new record
    let created = table.set(id, &inner).await.unwrap();
    assert!(created, "Should return true for new record");
    counter.increment(1).await.unwrap();

    // Verify count increased
    assert_eq!(counter.get().await.unwrap() as usize, 1);

    // Verify record exists
    let retrieved = table.get(id).await.unwrap();
    assert_eq!(retrieved, inner);
}

#[tokio::test]
async fn test_set_method_updates_existing_record() {
    let (table, interner, counter, _dir) = create_test_table().await.unwrap();

    // First insert a record
    let id = RecordId::new();
    let mut data1 = new_map();
    data1.insert("name".to_string(), UserValue::Str("Bob".to_string()));
    data1.insert("age".to_string(), UserValue::Int(25));
    let inner1 = intern_value(&UserValue::Map(data1), &interner).await.unwrap();

    let created = table.set(id, &inner1).await.unwrap();
    assert!(created);
    counter.increment(1).await.unwrap();
    assert_eq!(counter.get().await.unwrap() as usize, 1);

    // Now update with set
    let mut data2 = new_map();
    data2.insert("name".to_string(), UserValue::Str("Robert".to_string()));
    data2.insert("age".to_string(), UserValue::Int(26));
    data2.insert("city".to_string(), UserValue::Str("NYC".to_string()));
    let inner2 = intern_value(&UserValue::Map(data2), &interner).await.unwrap();

    let created_again = table.set(id, &inner2).await.unwrap();
    assert!(!created_again, "Should return false for update");

    // Count should still be 1 (not incremented)
    assert_eq!(counter.get().await.unwrap() as usize, 1);

    // Verify updated value
    let retrieved = table.get(id).await.unwrap();
    assert_eq!(retrieved, inner2);
}

#[tokio::test]
async fn test_record_counter_with_insert_and_delete() {
    let (table, interner, counter, _dir) = create_test_table().await.unwrap();

    // Initial count should be 0
    assert_eq!(counter.get().await.unwrap() as usize, 0);

    // Insert 5 records
    let mut ids = vec![];
    for i in 0..5 {
        let mut data = new_map();
        data.insert("id".to_string(), UserValue::Int(i));
        let inner = intern_value(&UserValue::Map(data), &interner).await.unwrap();
        let id = table.insert(&inner).await.unwrap();
        counter.increment(1).await.unwrap();
        ids.push(id);
    }

    assert_eq!(counter.get().await.unwrap() as usize, 5);

    // Delete 2 records
    table.delete(ids[0]).await.unwrap();
    counter.increment(-1).await.unwrap();
    table.delete(ids[1]).await.unwrap();
    counter.increment(-1).await.unwrap();

    assert_eq!(counter.get().await.unwrap() as usize, 3);

    // Delete 1 more
    table.delete(ids[2]).await.unwrap();
    counter.increment(-1).await.unwrap();

    assert_eq!(counter.get().await.unwrap() as usize, 2);

    // Insert 3 more
    for i in 0..3 {
        let mut data = new_map();
        data.insert("new_id".to_string(), UserValue::Int(i));
        let inner = intern_value(&UserValue::Map(data), &interner).await.unwrap();
        table.insert(&inner).await.unwrap();
        counter.increment(1).await.unwrap();
    }

    assert_eq!(counter.get().await.unwrap() as usize, 5);
}

#[tokio::test]
async fn test_set_method_respects_counter() {
    let (table, interner, counter, _dir) = create_test_table().await.unwrap();

    assert_eq!(counter.get().await.unwrap() as usize, 0);

    let id1 = RecordId::new();
    let id2 = RecordId::new();

    let mut data = new_map();
    data.insert("value".to_string(), UserValue::Int(42));
    let inner = intern_value(&UserValue::Map(data.clone()), &interner).await.unwrap();

    // Create first record with set
    let created1 = table.set(id1, &inner).await.unwrap();
    assert!(created1);
    counter.increment(1).await.unwrap();
    assert_eq!(counter.get().await.unwrap() as usize, 1);

    // Create second record with set
    let created2 = table.set(id2, &inner).await.unwrap();
    assert!(created2);
    counter.increment(1).await.unwrap();
    assert_eq!(counter.get().await.unwrap() as usize, 2);

    // Update first record with set (count should not change)
    let updated = table.set(id1, &inner).await.unwrap();
    assert!(!updated);
    assert_eq!(counter.get().await.unwrap() as usize, 2);

    // Update second record with set (count should not change)
    let updated2 = table.set(id2, &inner).await.unwrap();
    assert!(!updated2);
    assert_eq!(counter.get().await.unwrap() as usize, 2);
}
