use super::helpers::{create_manager, create_nested_value, create_test_value};
use crate::legacy::index_definition::IndexDefinition;
use crate::legacy::index_info::IndexInfo;
use crate::legacy::index_info_item::IndexInfoItem;
use crate::legacy::index_manager::IndexManager;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::sync::Arc;

// ============================================================================
// Initialization tests
// ============================================================================

#[tokio::test]
async fn test_has_indexes_initially_false() {
    let (_, _, manager) = create_manager();

    assert!(!manager.has_indexes());
    assert!(!manager.has_unique_indexes());
}

#[tokio::test]
async fn test_has_indexes_true_after_load() {
    let data_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let info_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    let indexes = IndexInfo::from_definitions(vec![index_def]);
    let indexes_key = RecordId::system("indexes").to_bytes();
    let bytes = bincode::serialize(&indexes).unwrap();
    info_store
        .set(indexes_key.into(), bytes.into())
        .await
        .unwrap();

    let manager = IndexManager::new(data_store, info_store).await.unwrap();

    assert!(manager.has_indexes());
    assert!(!manager.has_unique_indexes());
}

#[tokio::test]
async fn test_has_unique_indexes_true_after_load() {
    let data_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let info_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

    let index_def = IndexDefinition::new(1002, vec![IndexInfoItem::new(vec![1])]);
    let indexes = IndexInfo::from_definitions(vec![index_def]);
    let indexes_unique_key = RecordId::system("indexes_unique").to_bytes();
    let bytes = bincode::serialize(&indexes).unwrap();
    info_store
        .set(indexes_unique_key.into(), bytes.into())
        .await
        .unwrap();

    let manager = IndexManager::new(data_store, info_store).await.unwrap();

    assert!(!manager.has_indexes());
    assert!(manager.has_unique_indexes());
}

// ============================================================================
// Clone tests - verify shared state
// ============================================================================

#[tokio::test]
async fn test_clone_shares_state() {
    let (_, _, manager) = create_manager();

    // Initially no indexes
    assert!(!manager.has_indexes());

    // Clone the manager
    let clone = manager.clone();

    // Both should show no indexes
    assert!(!manager.has_indexes());
    assert!(!clone.has_indexes());

    // Create index through original
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    // Both clones should see the change
    assert!(manager.has_indexes());
    assert!(clone.has_indexes());
}

#[tokio::test]
async fn test_multiple_clones_share_state() {
    let (_, _, manager) = create_manager();

    let clone1 = manager.clone();
    let clone2 = clone1.clone();
    let clone3 = manager.clone();

    // Create index through clone2
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![42])]);
    clone2.create_index(index_def).await.unwrap();

    // All should see the change
    assert!(manager.has_indexes());
    assert!(clone1.has_indexes());
    assert!(clone2.has_indexes());
    assert!(clone3.has_indexes());
}

// ============================================================================
// create_index tests
// ============================================================================

#[tokio::test]
async fn test_create_index_empty_table() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    assert!(manager.has_indexes());
}

#[tokio::test]
async fn test_create_index_with_data() {
    let (data_store, _, manager) = create_manager();

    // Insert test data into data_store
    let value = create_test_value(&[
        (1, InnerValue::Str("test".to_string())),
        (2, InnerValue::Int(42)),
    ]);
    let record_id = RecordId::new();
    data_store
        .set(record_id.to_bytes().into(), value.to_bytes().unwrap())
        .await
        .unwrap();

    // Create index on field 1
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    assert!(manager.has_indexes());
}

#[tokio::test]
async fn test_create_composite_index() {
    let (data_store, _, manager) = create_manager();

    // Insert test data
    let value = create_test_value(&[
        (1, InnerValue::Str("Alice".to_string())),
        (2, InnerValue::Int(30)),
    ]);
    let record_id = RecordId::new();
    data_store
        .set(record_id.to_bytes().into(), value.to_bytes().unwrap())
        .await
        .unwrap();

    // Create composite index on fields [1, 2]
    let index_def = IndexDefinition::new(
        1001,
        vec![IndexInfoItem::new(vec![1]), IndexInfoItem::new(vec![2])],
    );
    manager.create_index(index_def).await.unwrap();

    assert!(manager.has_indexes());
}

#[tokio::test]
async fn test_create_nested_field_index() {
    let (data_store, _, manager) = create_manager();

    // Insert nested data: {"user": {"name": "Bob"}}
    let nested = create_nested_value(&[10], InnerValue::Str("Bob".to_string()));
    let value = create_test_value(&[(1, nested)]);

    let record_id = RecordId::new();
    data_store
        .set(record_id.to_bytes().into(), value.to_bytes().unwrap())
        .await
        .unwrap();

    // Create index on nested path [1, 10] (user.name)
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1, 10])]);
    manager.create_index(index_def).await.unwrap();

    assert!(manager.has_indexes());
}

#[tokio::test]
async fn test_create_index_missing_field_skipped() {
    let (data_store, _, manager) = create_manager();

    // Insert data without the indexed field
    let value = create_test_value(&[(2, InnerValue::Int(42))]);
    let record_id = RecordId::new();
    data_store
        .set(record_id.to_bytes().into(), value.to_bytes().unwrap())
        .await
        .unwrap();

    // Create index on field 1 (which doesn't exist in data)
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    // Index should be created but with no entries
    assert!(manager.has_indexes());
}

// ============================================================================
// drop_index tests
// ============================================================================

#[tokio::test]
async fn test_drop_existing_index() {
    let (_, _, manager) = create_manager();

    // Create index
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();
    assert!(manager.has_indexes());

    // Drop index
    let result = manager.drop_index(1001).await.unwrap();
    assert!(result);
    assert!(!manager.has_indexes());
}

#[tokio::test]
async fn test_drop_non_existing_index() {
    let (_, _, manager) = create_manager();

    let result = manager.drop_index(999).await.unwrap();
    assert!(!result);
}

#[tokio::test]
async fn test_drop_last_index_updates_flag() {
    let (_, _, manager) = create_manager();

    // Create two indexes
    let index_def1 = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    let index_def2 = IndexDefinition::new(1002, vec![IndexInfoItem::new(vec![2])]);
    manager.create_index(index_def1).await.unwrap();
    manager.create_index(index_def2).await.unwrap();
    assert!(manager.has_indexes());

    // Drop first - flag should still be true
    manager.drop_index(1001).await.unwrap();
    assert!(manager.has_indexes());

    // Drop second - flag should be false
    manager.drop_index(1002).await.unwrap();
    assert!(!manager.has_indexes());
}

#[tokio::test]
async fn test_drop_index_with_data() {
    let (data_store, _, manager) = create_manager();

    // Insert data and create index
    let value = create_test_value(&[(1, InnerValue::Str("test".to_string()))]);
    let record_id = RecordId::new();
    data_store
        .set(record_id.to_bytes().into(), value.to_bytes().unwrap())
        .await
        .unwrap();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    // Drop index
    let result = manager.drop_index(1001).await.unwrap();
    assert!(result);
    assert!(!manager.has_indexes());
}

// ============================================================================
// on_record_created tests
// ============================================================================

#[tokio::test]
async fn test_on_record_created_with_index() {
    let (_, _, manager) = create_manager();

    // Create index first
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    // Create a record
    let value = create_test_value(&[(1, InnerValue::Str("test".to_string()))]);
    let record_id = RecordId::new();

    // Should not panic
    manager.on_record_created(&record_id, &value).await.unwrap();
}

#[tokio::test]
async fn test_on_record_created_without_index() {
    let (_, _, manager) = create_manager();

    // No index created
    let value = create_test_value(&[(1, InnerValue::Str("test".to_string()))]);
    let record_id = RecordId::new();

    // Should be a no-op
    manager.on_record_created(&record_id, &value).await.unwrap();
}

#[tokio::test]
async fn test_on_record_created_missing_field() {
    let (_, _, manager) = create_manager();

    // Create index on field 1
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    // Create record without field 1
    let value = create_test_value(&[(2, InnerValue::Int(42))]);
    let record_id = RecordId::new();

    // Should not panic - field is missing, so no index entry
    manager.on_record_created(&record_id, &value).await.unwrap();
}

// ============================================================================
// on_record_updated tests
// ============================================================================

#[tokio::test]
async fn test_on_record_updated_value_changed() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    let old_value = create_test_value(&[(1, InnerValue::Str("old".to_string()))]);
    let new_value = create_test_value(&[(1, InnerValue::Str("new".to_string()))]);
    let record_id = RecordId::new();

    manager
        .on_record_updated(&record_id, &old_value, &new_value)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_on_record_updated_value_same() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    let value = create_test_value(&[(1, InnerValue::Str("same".to_string()))]);
    let record_id = RecordId::new();

    // Same value - should be a no-op optimization
    manager
        .on_record_updated(&record_id, &value, &value.clone())
        .await
        .unwrap();
}

#[tokio::test]
async fn test_on_record_updated_field_appeared() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    // Old value doesn't have indexed field
    let old_value = create_test_value(&[(2, InnerValue::Int(42))]);
    // New value has indexed field
    let new_value = create_test_value(&[(1, InnerValue::Str("appeared".to_string()))]);
    let record_id = RecordId::new();

    manager
        .on_record_updated(&record_id, &old_value, &new_value)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_on_record_updated_field_disappeared() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    // Old value has indexed field
    let old_value = create_test_value(&[(1, InnerValue::Str("will_disappear".to_string()))]);
    // New value doesn't have indexed field
    let new_value = create_test_value(&[(2, InnerValue::Int(42))]);
    let record_id = RecordId::new();

    manager
        .on_record_updated(&record_id, &old_value, &new_value)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_on_record_updated_both_missing_field() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    let old_value = create_test_value(&[(2, InnerValue::Int(1))]);
    let new_value = create_test_value(&[(2, InnerValue::Int(2))]);
    let record_id = RecordId::new();

    // Both don't have field 1 - should be a complete no-op
    manager
        .on_record_updated(&record_id, &old_value, &new_value)
        .await
        .unwrap();
}

// ============================================================================
// on_record_deleted tests
// ============================================================================

#[tokio::test]
async fn test_on_record_deleted_with_index() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    let value = create_test_value(&[(1, InnerValue::Str("to_delete".to_string()))]);
    let record_id = RecordId::new();

    manager.on_record_deleted(&record_id, &value).await.unwrap();
}

#[tokio::test]
async fn test_on_record_deleted_without_index() {
    let (_, _, manager) = create_manager();

    let value = create_test_value(&[(1, InnerValue::Str("test".to_string()))]);
    let record_id = RecordId::new();

    manager.on_record_deleted(&record_id, &value).await.unwrap();
}

#[tokio::test]
async fn test_on_record_deleted_missing_field() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    // Record doesn't have the indexed field
    let value = create_test_value(&[(2, InnerValue::Int(42))]);
    let record_id = RecordId::new();

    manager.on_record_deleted(&record_id, &value).await.unwrap();
}

// ============================================================================
// Persistence tests
// ============================================================================

#[tokio::test]
async fn test_persist_index_metadata() {
    let data_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let info_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

    // Create manager and add index
    let manager = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1, 2])]);
    manager.create_index(index_def).await.unwrap();

    // Create new manager from same stores - should load the index
    let manager2 = IndexManager::new(data_store, info_store).await.unwrap();

    assert!(manager2.has_indexes());
}

#[tokio::test]
async fn test_persist_multiple_indexes() {
    let data_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let info_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

    let manager = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();

    // Create multiple indexes
    let index_def1 = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    let index_def2 = IndexDefinition::new(1002, vec![IndexInfoItem::new(vec![2])]);
    manager.create_index(index_def1).await.unwrap();
    manager.create_index(index_def2).await.unwrap();

    // Reload
    let manager2 = IndexManager::new(data_store, info_store).await.unwrap();

    assert!(manager2.has_indexes());
}

// ============================================================================
// Edge cases
// ============================================================================

#[tokio::test]
async fn test_empty_path_index() {
    let (data_store, _, manager) = create_manager();

    // Insert any data
    let value = create_test_value(&[(1, InnerValue::Int(42))]);
    let record_id = RecordId::new();
    data_store
        .set(record_id.to_bytes().into(), value.to_bytes().unwrap())
        .await
        .unwrap();

    // Create index with empty path (indexes the whole document)
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![])]);
    manager.create_index(index_def).await.unwrap();

    assert!(manager.has_indexes());
}

#[tokio::test]
async fn test_various_value_types_in_index() {
    let (data_store, _, manager) = create_manager();

    // Test different value types
    let test_values = vec![
        (1, InnerValue::Int(42)),
        (2, InnerValue::Str("hello".to_string())),
        (3, InnerValue::Bool(true)),
        (4, InnerValue::Null),
    ];

    for (key, val) in &test_values {
        let value = create_test_value(&[(*key, val.clone())]);
        let record_id = RecordId::new();
        data_store
            .set(record_id.to_bytes().into(), value.to_bytes().unwrap())
            .await
            .unwrap();
    }

    // Create indexes for each type
    for (key, _) in &test_values {
        let index_def = IndexDefinition::new(1000 + *key, vec![IndexInfoItem::new(vec![*key])]);
        manager.create_index(index_def).await.unwrap();
    }

    assert!(manager.has_indexes());
}

// ============================================================================
// lookup_by_index tests
// ============================================================================

#[tokio::test]
async fn test_lookup_by_index_empty_result() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    // Lookup non-existent value
    let result = manager
        .lookup_by_index(1001, &[InnerValue::Str("nonexistent".to_string())])
        .await
        .unwrap();

    assert!(result.is_empty());
}

#[tokio::test]
async fn test_lookup_by_index_single_record() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    let value = create_test_value(&[(1, InnerValue::Str("Alice".to_string()))]);
    let record_id = RecordId::new();
    manager.on_record_created(&record_id, &value).await.unwrap();

    // Lookup by indexed value
    let result = manager
        .lookup_by_index(1001, &[InnerValue::Str("Alice".to_string())])
        .await
        .unwrap();

    assert_eq!(result.len(), 1);
    assert!(result.contains(&record_id));
}

#[tokio::test]
async fn test_lookup_by_index_multiple_records() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    // Add multiple records with same indexed value
    let value = create_test_value(&[(1, InnerValue::Str("Bob".to_string()))]);
    let record_id1 = RecordId::new();
    let record_id2 = RecordId::new();
    let record_id3 = RecordId::new();

    manager
        .on_record_created(&record_id1, &value)
        .await
        .unwrap();
    manager
        .on_record_created(&record_id2, &value)
        .await
        .unwrap();
    manager
        .on_record_created(&record_id3, &value)
        .await
        .unwrap();

    // Lookup should return all three
    let result = manager
        .lookup_by_index(1001, &[InnerValue::Str("Bob".to_string())])
        .await
        .unwrap();

    assert_eq!(result.len(), 3);
    assert!(result.contains(&record_id1));
    assert!(result.contains(&record_id2));
    assert!(result.contains(&record_id3));
}

#[tokio::test]
async fn test_lookup_by_index_after_delete() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    let value = create_test_value(&[(1, InnerValue::Str("Charlie".to_string()))]);
    let record_id = RecordId::new();

    manager.on_record_created(&record_id, &value).await.unwrap();

    // Lookup - should find
    let result = manager
        .lookup_by_index(1001, &[InnerValue::Str("Charlie".to_string())])
        .await
        .unwrap();
    assert_eq!(result.len(), 1);

    // Delete record
    manager.on_record_deleted(&record_id, &value).await.unwrap();

    // Lookup - should be empty
    let result = manager
        .lookup_by_index(1001, &[InnerValue::Str("Charlie".to_string())])
        .await
        .unwrap();
    assert!(result.is_empty());
}

#[tokio::test]
async fn test_lookup_by_index_composite() {
    let (_, _, manager) = create_manager();

    // Composite index on [field 1, field 2]
    let index_def = IndexDefinition::new(
        1001,
        vec![IndexInfoItem::new(vec![1]), IndexInfoItem::new(vec![2])],
    );
    manager.create_index(index_def).await.unwrap();

    let value = create_test_value(&[
        (1, InnerValue::Str("Alice".to_string())),
        (2, InnerValue::Int(30)),
    ]);
    let record_id = RecordId::new();
    manager.on_record_created(&record_id, &value).await.unwrap();

    // Lookup with both values
    let result = manager
        .lookup_by_index(
            1001,
            &[InnerValue::Str("Alice".to_string()), InnerValue::Int(30)],
        )
        .await
        .unwrap();

    assert_eq!(result.len(), 1);
    assert!(result.contains(&record_id));

    // Lookup with wrong second value
    let result = manager
        .lookup_by_index(
            1001,
            &[InnerValue::Str("Alice".to_string()), InnerValue::Int(25)],
        )
        .await
        .unwrap();

    assert!(result.is_empty());
}

#[tokio::test]
async fn test_lookup_by_index_different_values() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    // Add records with different values
    let value_a = create_test_value(&[(1, InnerValue::Str("A".to_string()))]);
    let value_b = create_test_value(&[(1, InnerValue::Str("B".to_string()))]);
    let record_id_a = RecordId::new();
    let record_id_b = RecordId::new();

    manager
        .on_record_created(&record_id_a, &value_a)
        .await
        .unwrap();
    manager
        .on_record_created(&record_id_b, &value_b)
        .await
        .unwrap();

    // Lookup A
    let result_a = manager
        .lookup_by_index(1001, &[InnerValue::Str("A".to_string())])
        .await
        .unwrap();
    assert_eq!(result_a.len(), 1);
    assert!(result_a.contains(&record_id_a));

    // Lookup B
    let result_b = manager
        .lookup_by_index(1001, &[InnerValue::Str("B".to_string())])
        .await
        .unwrap();
    assert_eq!(result_b.len(), 1);
    assert!(result_b.contains(&record_id_b));
}

#[tokio::test]
async fn test_lookup_by_index_non_existing_index() {
    let (_, _, manager) = create_manager();

    // No index created - lookup should return empty (no error)
    let result = manager
        .lookup_by_index(9999, &[InnerValue::Str("test".to_string())])
        .await
        .unwrap();

    assert!(result.is_empty());
}

// ============================================================================
// index_exists and get_index_definition tests
// ============================================================================

#[tokio::test]
async fn test_index_exists() {
    let (_, _, manager) = create_manager();

    assert!(!manager.index_exists(1001));

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    assert!(manager.index_exists(1001));
    assert!(!manager.index_exists(1002));
}

#[tokio::test]
async fn test_get_index_definition() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1, 2])]);
    manager.create_index(index_def).await.unwrap();

    let retrieved = manager.get_index_definition(1001);
    assert!(retrieved.is_some());

    let def = retrieved.unwrap();
    assert_eq!(def.name_interned, 1001);
    assert_eq!(def.paths.len(), 1);
    assert_eq!(def.paths[0].path, vec![1, 2]);

    // Non-existent
    let missing = manager.get_index_definition(9999);
    assert!(missing.is_none());
}

// ============================================================================
// create_index with lookup verification - records actually indexed
// ============================================================================

#[tokio::test]
async fn test_create_index_actually_indexes_records() {
    let (data_store, _, manager) = create_manager();

    // Insert 5 records with different values
    let mut record_ids = Vec::new();
    for i in 0..5i64 {
        let value = create_test_value(&[(1, InnerValue::Str(format!("value_{}", i)))]);
        let record_id = RecordId::new();
        data_store
            .set(record_id.to_bytes().into(), value.to_bytes().unwrap())
            .await
            .unwrap();
        record_ids.push(record_id);
    }

    // Create index AFTER data exists
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    // Verify each record is indexed and findable
    for (i, expected_id) in record_ids.iter().enumerate() {
        let result = manager
            .lookup_by_index(1001, &[InnerValue::Str(format!("value_{}", i))])
            .await
            .unwrap();
        assert_eq!(result.len(), 1, "Expected 1 result for value_{}", i);
        assert!(
            result.contains(expected_id),
            "Record {} not found in index",
            i
        );
    }
}

#[tokio::test]
async fn test_create_index_with_many_records() {
    let (data_store, _, manager) = create_manager();

    // Insert 100 records
    let mut record_ids = Vec::new();
    for i in 0..100i64 {
        let value = create_test_value(&[(1, InnerValue::Int(i))]);
        let record_id = RecordId::new();
        data_store
            .set(record_id.to_bytes().into(), value.to_bytes().unwrap())
            .await
            .unwrap();
        record_ids.push(record_id);
    }

    // Create index
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    // Verify random records
    for i in [0usize, 25, 50, 75, 99] {
        let result = manager
            .lookup_by_index(1001, &[InnerValue::Int(i as i64)])
            .await
            .unwrap();
        assert_eq!(result.len(), 1);
        assert!(result.contains(&record_ids[i]));
    }
}

#[tokio::test]
async fn test_create_index_with_duplicate_values() {
    let (data_store, _, manager) = create_manager();

    // Insert 10 records, 5 with same value
    let shared_value = InnerValue::Str("shared".to_string());
    let mut shared_ids = Vec::new();
    for _ in 0..5 {
        let value = create_test_value(&[(1, shared_value.clone())]);
        let record_id = RecordId::new();
        data_store
            .set(record_id.to_bytes().into(), value.to_bytes().unwrap())
            .await
            .unwrap();
        shared_ids.push(record_id);
    }

    // Create index
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    // Lookup should return all 5
    let result = manager
        .lookup_by_index(1001, &[shared_value])
        .await
        .unwrap();
    assert_eq!(result.len(), 5);
    for id in &shared_ids {
        assert!(result.contains(id));
    }
}

// ============================================================================
// §2.4 regression: create_index (streaming, batch-by-batch) must produce
// IDENTICAL index results to create_index_from_records (materialized).
// The fix changed create_index from "accumulate all records then build" to
// "build+flush per batch" — this must be a pure memory/incrementality change,
// not a behavior change.
// ============================================================================

/// Populate a data_store with `n` records that have diverse field values
/// (str, int, f64, null, bool) on `FIELD_ID`, then build the SAME index via
/// both `create_index` (streaming path) and `create_index_from_records`
/// (materialized path) on two independent managers, and assert lookups return
/// the same RecordIds for every value.
#[tokio::test]
async fn test_create_index_streaming_matches_materialized() {
    use shamir_types::core::interner::InternerKey;
    use shamir_types::types::common::new_map;

    const FIELD_ID: u64 = 1;
    const NAME_INTERNED: u64 = 5001;
    const N: usize = 500;

    // Build records with diverse field types and unique values.
    let values: Vec<InnerValue> = (0..N)
        .map(|i| match i % 5 {
            0 => InnerValue::Str(format!("val_{i}")),
            1 => InnerValue::Int(i as i64),
            2 => InnerValue::F64(i as f64 * 1.5),
            3 => InnerValue::Null,
            _ => InnerValue::Bool(i % 2 == 0),
        })
        .collect();

    // --- Manager A: streaming path (create_index via data_store.iter_stream) ---
    let data_store_a = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let info_store_a = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let manager_a = IndexManager::new(Arc::clone(&data_store_a), Arc::clone(&info_store_a))
        .await
        .unwrap();
    // Insert records into data_store.
    let record_ids: Vec<RecordId> = (0..N)
        .map(|i| RecordId::from_ts_seq(1_700_000_000_000_000, i as u32))
        .collect();
    for (i, v) in values.iter().enumerate() {
        let mut map = new_map();
        map.insert(InternerKey::new(FIELD_ID), v.clone());
        let stored = InnerValue::Map(map);
        data_store_a
            .set(record_ids[i].to_bytes().into(), stored.to_bytes().unwrap())
            .await
            .unwrap();
    }
    let def_a = IndexDefinition::new(NAME_INTERNED, vec![IndexInfoItem::new(vec![FIELD_ID])]);
    manager_a.create_index(def_a).await.unwrap();

    // --- Manager B: materialized path (create_index_from_records) ---
    let data_store_b = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let info_store_b = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let manager_b = IndexManager::new(Arc::clone(&data_store_b), Arc::clone(&info_store_b))
        .await
        .unwrap();
    // Collect the same records (already decoded).
    let records: Vec<(RecordId, InnerValue)> = (0..N)
        .map(|i| {
            let mut map = new_map();
            map.insert(InternerKey::new(FIELD_ID), values[i].clone());
            (record_ids[i], InnerValue::Map(map))
        })
        .collect();
    let def_b = IndexDefinition::new(NAME_INTERNED, vec![IndexInfoItem::new(vec![FIELD_ID])]);
    manager_b
        .create_index_from_records(def_b, records)
        .await
        .unwrap();

    // --- Verify lookup results are identical for every distinct value. ---
    // Use a set of representative values to look up.
    use shamir_types::types::common::new_set;
    let mut distinct_values = new_set::<InnerValue>();
    for v in &values {
        distinct_values.insert(v.clone());
    }

    for lookup_val in &distinct_values {
        let result_a = manager_a
            .lookup_by_index(NAME_INTERNED, std::slice::from_ref(lookup_val))
            .await
            .unwrap();
        let result_b = manager_b
            .lookup_by_index(NAME_INTERNED, std::slice::from_ref(lookup_val))
            .await
            .unwrap();
        assert_eq!(
            result_a, result_b,
            "lookup mismatch for {:?}: streaming={:?} materialized={:?}",
            lookup_val, result_a, result_b
        );
    }

    // Also verify the total posting count is the same (excluding system keys).
    // Count records that have a non-null indexed value — those should have
    // postings. Null is also a valid posting value.
    // Both managers should report the same count for every lookup.
}

// ============================================================================
// Audit 3.2 / task #499 — sorted-slice posting representation regression tests
// ============================================================================

/// Reference oracle: recompute the posting set the way the OLD `BTreeSet`
/// path did — collect the ids and let `BTreeSet` sort+dedup them — from an
/// independent enumeration of the created records. `lookup_by_index` (new
/// `Arc<[RecordId]>` path) must return EXACTLY the same elements, in sorted
/// order, with no duplicates.
fn btreeset_oracle(ids: &[RecordId]) -> std::collections::BTreeSet<RecordId> {
    ids.iter().copied().collect()
}

/// The returned slice must equal the `BTreeSet` oracle: identical membership,
/// sorted ascending by `RecordId::Ord`, duplicate-free. Covers empty, single,
/// disjoint (query value with no records), fully-overlapping (all records
/// share one value), and a 10k low-cardinality bucket.
#[tokio::test]
async fn test_sorted_slice_matches_btreeset_oracle() {
    let (_, _, manager) = create_manager();
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    // Insert record ids in DESCENDING creation order but with ASCENDING
    // record-id bytes shuffled, so a naive "scan order == insert order"
    // assumption would fail — only true sorted-by-RecordId output passes.
    let batch_ts = RecordId::now_micros();
    let mut hot_ids: Vec<RecordId> = Vec::new();
    // 10_000 postings under the single hot value "active" (low cardinality).
    for i in 0..10_000u32 {
        // Interleave seq tails so ids are not monotonic in insertion order.
        let seq = if i % 2 == 0 { 20_000 - i } else { i };
        let rid = RecordId::from_ts_seq(batch_ts, seq);
        hot_ids.push(rid);
        let value = create_test_value(&[(1, InnerValue::Str("active".to_string()))]);
        manager.on_record_created(&rid, &value).await.unwrap();
    }

    // ── Fully-overlapping / 10k bucket ──────────────────────────────────
    let result = manager
        .lookup_by_index(1001, &[InnerValue::Str("active".to_string())])
        .await
        .unwrap();
    let oracle = btreeset_oracle(&hot_ids);
    // Same membership as the BTreeSet oracle.
    let result_set: std::collections::BTreeSet<RecordId> = result.iter().copied().collect();
    assert_eq!(
        result_set, oracle,
        "sorted-slice membership must equal the BTreeSet oracle"
    );
    // Sorted ascending — the slice equals the oracle drained in order.
    let oracle_sorted: Vec<RecordId> = oracle.iter().copied().collect();
    assert_eq!(
        result.as_ref(),
        oracle_sorted.as_slice(),
        "posting slice must be sorted ascending by RecordId::Ord"
    );
    // Duplicate-free: sorted slice has no equal adjacent pair.
    assert!(
        result.windows(2).all(|w| w[0] < w[1]),
        "posting slice must be strictly ascending (duplicate-free)"
    );
    // Explicitly the length the oracle expects (no lost/extra postings).
    assert_eq!(result.len(), 10_000);

    // ── Disjoint: a value with no records → empty slice ─────────────────
    let empty = manager
        .lookup_by_index(1001, &[InnerValue::Str("nonexistent".to_string())])
        .await
        .unwrap();
    assert!(empty.is_empty(), "disjoint query must return empty slice");
    assert_eq!(
        empty.as_ref(),
        btreeset_oracle(&[])
            .iter()
            .copied()
            .collect::<Vec<_>>()
            .as_slice()
    );
}

/// Single-element and repeated-lookup (cache HIT) both return the identical
/// sorted slice — the cache does not perturb ordering or membership.
#[tokio::test]
async fn test_sorted_slice_single_and_cache_hit_identical() {
    let (_, _, manager) = create_manager();
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    let rid = RecordId::new();
    let value = create_test_value(&[(1, InnerValue::Int(7))]);
    manager.on_record_created(&rid, &value).await.unwrap();

    // MISS → build; HIT → cached Arc. Both must be the identical slice.
    let miss = manager
        .lookup_by_index(1001, &[InnerValue::Int(7)])
        .await
        .unwrap();
    let hit = manager
        .lookup_by_index(1001, &[InnerValue::Int(7)])
        .await
        .unwrap();
    assert_eq!(
        miss.as_ref(),
        &[rid][..],
        "single-element slice must hold exactly the one id"
    );
    assert_eq!(
        miss.as_ref(),
        hit.as_ref(),
        "cache HIT must return the same slice as MISS"
    );
    // The HIT shares the cached allocation (O(1), no re-derivation).
    assert!(
        Arc::ptr_eq(&miss, &hit),
        "cache HIT must be the same Arc allocation"
    );
}
