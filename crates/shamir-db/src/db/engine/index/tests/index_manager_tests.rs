use crate::core::interner::InternerKey;
use crate::db::engine::index::index_definition::IndexDefinition;
use crate::db::engine::index::index_info::IndexInfo;
use crate::db::engine::index::index_info_item::IndexInfoItem;
use crate::db::engine::index::index_manager::IndexManager;
use crate::db::storage::storage_in_memory::InMemoryStore;
use crate::db::storage::types::Store;
use crate::types::common::new_map;
use crate::types::record_id::RecordId;
use crate::types::value::InnerValue;
use std::sync::Arc;

// ============================================================================
// Helper functions
// ============================================================================

/// Creates a new IndexManager with in-memory stores
fn create_manager() -> (Arc<dyn Store>, Arc<dyn Store>, IndexManager) {
    let data_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let info_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let manager = futures::executor::block_on(IndexManager::new(
        Arc::clone(&data_store),
        Arc::clone(&info_store),
    ))
    .unwrap();
    (data_store, info_store, manager)
}

/// Creates a test InnerValue::Map with given key-value pairs
fn create_test_value(pairs: &[(u64, InnerValue)]) -> InnerValue {
    let mut map = new_map();
    for (key, value) in pairs {
        map.insert(InternerKey::new(*key), value.clone());
    }
    InnerValue::Map(map)
}

/// Creates a nested map value for testing path extraction
fn create_nested_value(keys: &[u64], leaf_value: InnerValue) -> InnerValue {
    if keys.is_empty() {
        return leaf_value;
    }
    let mut map = new_map();
    map.insert(
        InternerKey::new(keys[0]),
        create_nested_value(&keys[1..], leaf_value),
    );
    InnerValue::Map(map)
}

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
    info_store.set(indexes_key, bytes.into()).await.unwrap();

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
        .set(indexes_unique_key, bytes.into())
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
        .set(record_id.to_bytes(), value.to_bytes())
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
        .set(record_id.to_bytes(), value.to_bytes())
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
        .set(record_id.to_bytes(), value.to_bytes())
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
        .set(record_id.to_bytes(), value.to_bytes())
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
        .set(record_id.to_bytes(), value.to_bytes())
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
        .set(record_id.to_bytes(), value.to_bytes())
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
            .set(record_id.to_bytes(), value.to_bytes())
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
// Multiple records with same indexed value (BTreeSet tests)
// ============================================================================

#[tokio::test]
async fn test_multiple_records_same_indexed_value() {
    let (_, _, manager) = create_manager();

    // Create index
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    // Add multiple records with same indexed value
    let value = create_test_value(&[(1, InnerValue::Str("same_value".to_string()))]);
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

    // All three should be indexed
    // Remove one - other two should remain
    manager
        .on_record_deleted(&record_id2, &value)
        .await
        .unwrap();

    // Remove another
    manager
        .on_record_deleted(&record_id1, &value)
        .await
        .unwrap();

    // Last one should still be indexed
    // Remove last - index entry should be deleted entirely
    manager
        .on_record_deleted(&record_id3, &value)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_duplicate_record_id_not_added_twice() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    let value = create_test_value(&[(1, InnerValue::Str("test".to_string()))]);
    let record_id = RecordId::new();

    // Add same record twice - should be idempotent
    manager.on_record_created(&record_id, &value).await.unwrap();
    manager.on_record_created(&record_id, &value).await.unwrap();

    // Remove once - should work
    manager.on_record_deleted(&record_id, &value).await.unwrap();

    // Remove again - should be idempotent (no error)
    manager.on_record_deleted(&record_id, &value).await.unwrap();
}

#[tokio::test]
async fn test_update_moves_record_between_index_values() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    let record_id = RecordId::new();

    // Old value: field 1 = "old"
    let old_value = create_test_value(&[(1, InnerValue::Str("old".to_string()))]);
    manager
        .on_record_created(&record_id, &old_value)
        .await
        .unwrap();

    // New value: field 1 = "new"
    let new_value = create_test_value(&[(1, InnerValue::Str("new".to_string()))]);
    manager
        .on_record_updated(&record_id, &old_value, &new_value)
        .await
        .unwrap();

    // Delete with new value - should work
    manager
        .on_record_deleted(&record_id, &new_value)
        .await
        .unwrap();
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
// UNIQUE INDEXES tests
// ============================================================================

#[tokio::test]
async fn test_create_unique_index_empty_table() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def).await.unwrap();

    assert!(manager.has_unique_indexes());
    assert!(manager.unique_index_exists(1001));
}

#[tokio::test]
async fn test_create_unique_index_with_unique_data() {
    let (data_store, _, manager) = create_manager();

    // Insert unique data
    let value1 = create_test_value(&[(1, InnerValue::Str("Alice".to_string()))]);
    let value2 = create_test_value(&[(1, InnerValue::Str("Bob".to_string()))]);
    let id1 = RecordId::new();
    let id2 = RecordId::new();

    data_store
        .set(id1.to_bytes(), value1.to_bytes())
        .await
        .unwrap();
    data_store
        .set(id2.to_bytes(), value2.to_bytes())
        .await
        .unwrap();

    // Create unique index - should succeed
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def).await.unwrap();

    assert!(manager.has_unique_indexes());
}

#[tokio::test]
async fn test_create_unique_index_with_duplicate_data_fails() {
    let (data_store, _, manager) = create_manager();

    // Insert duplicate data (3 records with same value)
    let value = create_test_value(&[(1, InnerValue::Str("Same".to_string()))]);
    let id1 = RecordId::new();
    let id2 = RecordId::new();
    let id3 = RecordId::new();

    data_store
        .set(id1.to_bytes(), value.to_bytes())
        .await
        .unwrap();
    data_store
        .set(id2.to_bytes(), value.to_bytes())
        .await
        .unwrap();
    data_store
        .set(id3.to_bytes(), value.to_bytes())
        .await
        .unwrap();

    // Create unique index - should fail
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    let result = manager.create_unique_index(index_def).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    // Should be UniqueIndexCreationFailed with count=3 and sample value
    match &err {
        crate::db::DbError::UniqueIndexCreationFailed(index_name, count, sample) => {
            assert_eq!(*index_name, "1001");
            assert_eq!(*count, 3, "Expected 3 duplicate records");
            // The sample contains the Str value formatted as "Same"
            // Since Str uses InternerKey, the actual format may show the key number
            assert!(
                !sample.is_empty(),
                "Sample should not be empty, got: {}",
                sample
            );
        }
        _ => panic!("Expected UniqueIndexCreationFailed, got: {:?}", err),
    }
}

#[tokio::test]
async fn test_validate_unique_for_create_ok() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def).await.unwrap();

    // Add one record
    let value = create_test_value(&[(1, InnerValue::Str("Alice".to_string()))]);
    let record_id = RecordId::new();
    manager
        .on_record_created_unique(&record_id, &value)
        .await
        .unwrap();

    // Validate different value - should pass
    let new_value = create_test_value(&[(1, InnerValue::Str("Bob".to_string()))]);
    let result = manager.validate_unique_for_create(&new_value).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_validate_unique_for_create_fails() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def).await.unwrap();

    // Add one record
    let value = create_test_value(&[(1, InnerValue::Str("Alice".to_string()))]);
    let record_id = RecordId::new();
    manager
        .on_record_created_unique(&record_id, &value)
        .await
        .unwrap();

    // Validate same value - should fail
    let result = manager.validate_unique_for_create(&value).await;
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        crate::db::DbError::DuplicateKey(_)
    ));
}

#[tokio::test]
async fn test_validate_unique_for_update_same_value_ok() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def).await.unwrap();

    let value = create_test_value(&[(1, InnerValue::Str("Alice".to_string()))]);
    let record_id = RecordId::new();
    manager
        .on_record_created_unique(&record_id, &value)
        .await
        .unwrap();

    // Update to same value - should pass (same record)
    let result = manager
        .validate_unique_for_update(&record_id, &value, &value)
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_validate_unique_for_update_different_value_ok() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def).await.unwrap();

    let value = create_test_value(&[(1, InnerValue::Str("Alice".to_string()))]);
    let record_id = RecordId::new();
    manager
        .on_record_created_unique(&record_id, &value)
        .await
        .unwrap();

    // Update to different value - should pass
    let new_value = create_test_value(&[(1, InnerValue::Str("Bob".to_string()))]);
    let result = manager
        .validate_unique_for_update(&record_id, &value, &new_value)
        .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_validate_unique_for_update_duplicate_fails() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def).await.unwrap();

    // Add two records
    let value1 = create_test_value(&[(1, InnerValue::Str("Alice".to_string()))]);
    let value2 = create_test_value(&[(1, InnerValue::Str("Bob".to_string()))]);
    let id1 = RecordId::new();
    let id2 = RecordId::new();
    manager
        .on_record_created_unique(&id1, &value1)
        .await
        .unwrap();
    manager
        .on_record_created_unique(&id2, &value2)
        .await
        .unwrap();

    // Try to update id2 to "Alice" - should fail
    let result = manager
        .validate_unique_for_update(&id2, &value2, &value1)
        .await;
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        crate::db::DbError::DuplicateKey(_)
    ));
}

#[tokio::test]
async fn test_lookup_by_unique_index() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def).await.unwrap();

    let value = create_test_value(&[(1, InnerValue::Str("Alice".to_string()))]);
    let record_id = RecordId::new();
    manager
        .on_record_created_unique(&record_id, &value)
        .await
        .unwrap();

    // Lookup
    let result = manager
        .lookup_by_unique_index(1001, &[InnerValue::Str("Alice".to_string())])
        .await
        .unwrap();

    assert!(result.is_some());
    assert_eq!(result.unwrap(), record_id);

    // Lookup non-existent
    let result = manager
        .lookup_by_unique_index(1001, &[InnerValue::Str("Bob".to_string())])
        .await
        .unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_drop_unique_index() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def).await.unwrap();
    assert!(manager.has_unique_indexes());

    // Drop
    let result = manager.drop_unique_index(1001).await.unwrap();
    assert!(result);
    assert!(!manager.has_unique_indexes());

    // Drop non-existent
    let result = manager.drop_unique_index(1002).await.unwrap();
    assert!(!result);
}

#[tokio::test]
async fn test_unique_index_on_record_deleted() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def).await.unwrap();

    let value = create_test_value(&[(1, InnerValue::Str("Alice".to_string()))]);
    let record_id = RecordId::new();
    manager
        .on_record_created_unique(&record_id, &value)
        .await
        .unwrap();

    // Should find
    let result = manager
        .lookup_by_unique_index(1001, &[InnerValue::Str("Alice".to_string())])
        .await
        .unwrap();
    assert!(result.is_some());

    // Delete
    manager
        .on_record_deleted_unique(&record_id, &value)
        .await
        .unwrap();

    // Should not find
    let result = manager
        .lookup_by_unique_index(1001, &[InnerValue::Str("Alice".to_string())])
        .await
        .unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_unique_index_on_record_updated() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def).await.unwrap();

    let old_value = create_test_value(&[(1, InnerValue::Str("Alice".to_string()))]);
    let new_value = create_test_value(&[(1, InnerValue::Str("Bob".to_string()))]);
    let record_id = RecordId::new();
    manager
        .on_record_created_unique(&record_id, &old_value)
        .await
        .unwrap();

    // Update
    manager
        .on_record_updated_unique(&record_id, &old_value, &new_value)
        .await
        .unwrap();

    // Old value should not exist
    let result = manager
        .lookup_by_unique_index(1001, &[InnerValue::Str("Alice".to_string())])
        .await
        .unwrap();
    assert!(result.is_none());

    // New value should exist
    let result = manager
        .lookup_by_unique_index(1001, &[InnerValue::Str("Bob".to_string())])
        .await
        .unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap(), record_id);
}

#[tokio::test]
async fn test_unique_index_persistence() {
    let data_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let info_store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;

    // Create manager and add unique index
    let manager = IndexManager::new(Arc::clone(&data_store), Arc::clone(&info_store))
        .await
        .unwrap();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def).await.unwrap();

    let value = create_test_value(&[(1, InnerValue::Str("Alice".to_string()))]);
    let record_id = RecordId::new();
    manager
        .on_record_created_unique(&record_id, &value)
        .await
        .unwrap();

    // Reload manager
    let manager2 = IndexManager::new(data_store, info_store).await.unwrap();

    assert!(manager2.has_unique_indexes());
    assert!(manager2.unique_index_exists(1001));

    // Should find the record
    let result = manager2
        .lookup_by_unique_index(1001, &[InnerValue::Str("Alice".to_string())])
        .await
        .unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap(), record_id);
}

#[tokio::test]
async fn test_unique_index_missing_field() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def).await.unwrap();

    // Value without indexed field
    let value = create_test_value(&[(2, InnerValue::Int(42))]);
    let record_id = RecordId::new();

    // Should pass validation (field missing)
    let result = manager.validate_unique_for_create(&value).await;
    assert!(result.is_ok());

    // Should not add to index
    manager
        .on_record_created_unique(&record_id, &value)
        .await
        .unwrap();

    // Should not find
    let result = manager
        .lookup_by_unique_index(1001, &[InnerValue::Int(42)])
        .await
        .unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_regular_and_unique_indexes_coexist() {
    let (_, _, manager) = create_manager();

    // Create both types
    let regular_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    let unique_def = IndexDefinition::new(2001, vec![IndexInfoItem::new(vec![2])]);
    manager.create_index(regular_def).await.unwrap();
    manager.create_unique_index(unique_def).await.unwrap();

    assert!(manager.has_indexes());
    assert!(manager.has_unique_indexes());

    // Add record
    let value = create_test_value(&[
        (1, InnerValue::Str("regular".to_string())),
        (2, InnerValue::Str("unique".to_string())),
    ]);
    let record_id = RecordId::new();
    manager.on_record_created(&record_id, &value).await.unwrap();
    manager
        .on_record_created_unique(&record_id, &value)
        .await
        .unwrap();

    // Lookup regular
    let regular_result = manager
        .lookup_by_index(1001, &[InnerValue::Str("regular".to_string())])
        .await
        .unwrap();
    assert_eq!(regular_result.len(), 1);

    // Lookup unique
    let unique_result = manager
        .lookup_by_unique_index(2001, &[InnerValue::Str("unique".to_string())])
        .await
        .unwrap();
    assert!(unique_result.is_some());
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
            .set(record_id.to_bytes(), value.to_bytes())
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
            .set(record_id.to_bytes(), value.to_bytes())
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
            .set(record_id.to_bytes(), value.to_bytes())
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
// Tests with various value types (Null, List, Map, Set)
// ============================================================================

#[tokio::test]
async fn test_create_index_with_null_value() {
    let (data_store, _, manager) = create_manager();

    let value = create_test_value(&[(1, InnerValue::Null)]);
    let record_id = RecordId::new();
    data_store
        .set(record_id.to_bytes(), value.to_bytes())
        .await
        .unwrap();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    let result = manager
        .lookup_by_index(1001, &[InnerValue::Null])
        .await
        .unwrap();
    assert_eq!(result.len(), 1);
    assert!(result.contains(&record_id));
}

#[tokio::test]
async fn test_create_index_with_list_value() {
    let (data_store, _, manager) = create_manager();

    let list = InnerValue::List(vec![
        InnerValue::Int(1),
        InnerValue::Int(2),
        InnerValue::Int(3),
    ]);
    let value = create_test_value(&[(1, list.clone())]);
    let record_id = RecordId::new();
    data_store
        .set(record_id.to_bytes(), value.to_bytes())
        .await
        .unwrap();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    let result = manager.lookup_by_index(1001, &[list]).await.unwrap();
    assert_eq!(result.len(), 1);
    assert!(result.contains(&record_id));
}

#[tokio::test]
async fn test_create_index_with_map_value() {
    let (data_store, _, manager) = create_manager();

    let mut inner_map = crate::types::common::new_map();
    inner_map.insert(InternerKey::new(100), InnerValue::Str("inner".to_string()));
    let map = InnerValue::Map(inner_map);

    let value = create_test_value(&[(1, map.clone())]);
    let record_id = RecordId::new();
    data_store
        .set(record_id.to_bytes(), value.to_bytes())
        .await
        .unwrap();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    let result = manager.lookup_by_index(1001, &[map]).await.unwrap();
    assert_eq!(result.len(), 1);
    assert!(result.contains(&record_id));
}

#[tokio::test]
async fn test_create_index_with_set_value() {
    // Note: Set values are hashable but cloning may produce different hashes
    // due to IndexSet internal ordering. This test verifies Set can be indexed.
    let (_, _, manager) = create_manager();

    use crate::types::common::TSet;

    let mut set: TSet<InnerValue> = TSet::default();
    set.insert(InnerValue::Int(1));
    set.insert(InnerValue::Int(2));
    let set_value = InnerValue::Set(set);

    // Just verify Set can be used as indexed value without error
    let value = create_test_value(&[(1, set_value)]);
    let record_id = RecordId::new();

    // Create index first (empty table)
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    // Add record - should not panic
    manager.on_record_created(&record_id, &value).await.unwrap();
}

// ============================================================================
// Composite index update tests - partial field change
// ============================================================================

#[tokio::test]
async fn test_composite_index_update_one_field() {
    let (_, _, manager) = create_manager();

    // Composite index on [field 1, field 2]
    let index_def = IndexDefinition::new(
        1001,
        vec![IndexInfoItem::new(vec![1]), IndexInfoItem::new(vec![2])],
    );
    manager.create_index(index_def).await.unwrap();

    // Initial record: name=Alice, age=30
    let old_value = create_test_value(&[
        (1, InnerValue::Str("Alice".to_string())),
        (2, InnerValue::Int(30)),
    ]);
    // Updated record: name=Alice, age=31 (only age changed)
    let new_value = create_test_value(&[
        (1, InnerValue::Str("Alice".to_string())),
        (2, InnerValue::Int(31)),
    ]);
    let record_id = RecordId::new();

    manager
        .on_record_created(&record_id, &old_value)
        .await
        .unwrap();

    // Verify initial index entry
    let result = manager
        .lookup_by_index(
            1001,
            &[InnerValue::Str("Alice".to_string()), InnerValue::Int(30)],
        )
        .await
        .unwrap();
    assert_eq!(result.len(), 1);

    // Update
    manager
        .on_record_updated(&record_id, &old_value, &new_value)
        .await
        .unwrap();

    // Old composite key should be empty
    let old_result = manager
        .lookup_by_index(
            1001,
            &[InnerValue::Str("Alice".to_string()), InnerValue::Int(30)],
        )
        .await
        .unwrap();
    assert!(old_result.is_empty());

    // New composite key should have the record
    let new_result = manager
        .lookup_by_index(
            1001,
            &[InnerValue::Str("Alice".to_string()), InnerValue::Int(31)],
        )
        .await
        .unwrap();
    assert_eq!(new_result.len(), 1);
    assert!(new_result.contains(&record_id));
}

#[tokio::test]
async fn test_composite_index_update_both_fields() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(
        1001,
        vec![IndexInfoItem::new(vec![1]), IndexInfoItem::new(vec![2])],
    );
    manager.create_index(index_def).await.unwrap();

    let old_value = create_test_value(&[
        (1, InnerValue::Str("Alice".to_string())),
        (2, InnerValue::Int(30)),
    ]);
    let new_value = create_test_value(&[
        (1, InnerValue::Str("Bob".to_string())),
        (2, InnerValue::Int(25)),
    ]);
    let record_id = RecordId::new();

    manager
        .on_record_created(&record_id, &old_value)
        .await
        .unwrap();
    manager
        .on_record_updated(&record_id, &old_value, &new_value)
        .await
        .unwrap();

    // Both fields changed - both old and new keys should be correct
    let old_result = manager
        .lookup_by_index(
            1001,
            &[InnerValue::Str("Alice".to_string()), InnerValue::Int(30)],
        )
        .await
        .unwrap();
    assert!(old_result.is_empty());

    let new_result = manager
        .lookup_by_index(
            1001,
            &[InnerValue::Str("Bob".to_string()), InnerValue::Int(25)],
        )
        .await
        .unwrap();
    assert_eq!(new_result.len(), 1);
}

// ============================================================================
// Unique index update tests - None↔Some transitions
// ============================================================================

#[tokio::test]
async fn test_unique_index_update_field_appears() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def).await.unwrap();

    // Old value: no indexed field
    let old_value = create_test_value(&[(2, InnerValue::Int(42))]);
    // New value: indexed field appears
    let new_value = create_test_value(&[(1, InnerValue::Str("appeared".to_string()))]);
    let record_id = RecordId::new();

    manager
        .on_record_created_unique(&record_id, &old_value)
        .await
        .unwrap();

    // Should not find with new value before update
    let before = manager
        .lookup_by_unique_index(1001, &[InnerValue::Str("appeared".to_string())])
        .await
        .unwrap();
    assert!(before.is_none());

    // Update: field appears
    manager
        .on_record_updated_unique(&record_id, &old_value, &new_value)
        .await
        .unwrap();

    // Now should find
    let after = manager
        .lookup_by_unique_index(1001, &[InnerValue::Str("appeared".to_string())])
        .await
        .unwrap();
    assert!(after.is_some());
    assert_eq!(after.unwrap(), record_id);
}

#[tokio::test]
async fn test_unique_index_update_field_disappears() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def).await.unwrap();

    // Old value: indexed field exists
    let old_value = create_test_value(&[(1, InnerValue::Str("will_disappear".to_string()))]);
    // New value: indexed field gone
    let new_value = create_test_value(&[(2, InnerValue::Int(42))]);
    let record_id = RecordId::new();

    manager
        .on_record_created_unique(&record_id, &old_value)
        .await
        .unwrap();

    // Should find before update
    let before = manager
        .lookup_by_unique_index(1001, &[InnerValue::Str("will_disappear".to_string())])
        .await
        .unwrap();
    assert!(before.is_some());

    // Update: field disappears
    manager
        .on_record_updated_unique(&record_id, &old_value, &new_value)
        .await
        .unwrap();

    // Should not find after update
    let after = manager
        .lookup_by_unique_index(1001, &[InnerValue::Str("will_disappear".to_string())])
        .await
        .unwrap();
    assert!(after.is_none());
}

#[tokio::test]
async fn test_unique_index_validate_update_field_appears() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def).await.unwrap();

    // Existing record with value
    let existing = create_test_value(&[(1, InnerValue::Str("existing".to_string()))]);
    let existing_id = RecordId::new();
    manager
        .on_record_created_unique(&existing_id, &existing)
        .await
        .unwrap();

    // Another record without the field
    let old_value = create_test_value(&[(2, InnerValue::Int(42))]);
    let record_id = RecordId::new();
    manager
        .on_record_created_unique(&record_id, &old_value)
        .await
        .unwrap();

    // Try to update to existing value - should fail
    let result = manager
        .validate_unique_for_update(&record_id, &old_value, &existing)
        .await;
    assert!(result.is_err());

    // Update to new unique value - should pass
    let new_value = create_test_value(&[(1, InnerValue::Str("new_unique".to_string()))]);
    let result = manager
        .validate_unique_for_update(&record_id, &old_value, &new_value)
        .await;
    assert!(result.is_ok());
}

// ============================================================================
// Composite unique index tests
// ============================================================================

#[tokio::test]
async fn test_composite_unique_index_lookup() {
    let (_, _, manager) = create_manager();

    // Unique composite index on [field 1, field 2]
    let index_def = IndexDefinition::new(
        1001,
        vec![IndexInfoItem::new(vec![1]), IndexInfoItem::new(vec![2])],
    );
    manager.create_unique_index(index_def).await.unwrap();

    // Add record
    let value = create_test_value(&[
        (1, InnerValue::Str("Alice".to_string())),
        (2, InnerValue::Int(30)),
    ]);
    let record_id = RecordId::new();
    manager
        .on_record_created_unique(&record_id, &value)
        .await
        .unwrap();

    // Lookup with both fields
    let result = manager
        .lookup_by_unique_index(
            1001,
            &[InnerValue::Str("Alice".to_string()), InnerValue::Int(30)],
        )
        .await
        .unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap(), record_id);

    // Lookup with wrong second field
    let result = manager
        .lookup_by_unique_index(
            1001,
            &[InnerValue::Str("Alice".to_string()), InnerValue::Int(25)],
        )
        .await
        .unwrap();
    assert!(result.is_none());

    // Lookup with wrong first field
    let result = manager
        .lookup_by_unique_index(
            1001,
            &[InnerValue::Str("Bob".to_string()), InnerValue::Int(30)],
        )
        .await
        .unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn test_composite_unique_index_creation_fails_with_duplicates() {
    let (data_store, _, manager) = create_manager();

    // Insert two records with same composite value
    let value = create_test_value(&[
        (1, InnerValue::Str("Alice".to_string())),
        (2, InnerValue::Int(30)),
    ]);
    let id1 = RecordId::new();
    let id2 = RecordId::new();
    data_store
        .set(id1.to_bytes(), value.to_bytes())
        .await
        .unwrap();
    data_store
        .set(id2.to_bytes(), value.to_bytes())
        .await
        .unwrap();

    // Create unique composite index - should fail
    let index_def = IndexDefinition::new(
        1001,
        vec![IndexInfoItem::new(vec![1]), IndexInfoItem::new(vec![2])],
    );
    let result = manager.create_unique_index(index_def).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        crate::db::DbError::UniqueIndexCreationFailed(name, count, _sample) => {
            assert_eq!(name, "1001");
            assert_eq!(count, 2);
        }
        _ => panic!("Expected UniqueIndexCreationFailed"),
    }
}

#[tokio::test]
async fn test_composite_unique_index_creation_succeeds_with_unique_data() {
    let (data_store, _, manager) = create_manager();

    // Insert records with different composite values
    let value1 = create_test_value(&[
        (1, InnerValue::Str("Alice".to_string())),
        (2, InnerValue::Int(30)),
    ]);
    let value2 = create_test_value(&[
        (1, InnerValue::Str("Alice".to_string())),
        (2, InnerValue::Int(25)),
    ]);
    let value3 = create_test_value(&[
        (1, InnerValue::Str("Bob".to_string())),
        (2, InnerValue::Int(30)),
    ]);

    for v in [&value1, &value2, &value3] {
        let id = RecordId::new();
        data_store.set(id.to_bytes(), v.to_bytes()).await.unwrap();
    }

    // Create unique composite index - should succeed
    let index_def = IndexDefinition::new(
        1001,
        vec![IndexInfoItem::new(vec![1]), IndexInfoItem::new(vec![2])],
    );
    let result = manager.create_unique_index(index_def).await;
    assert!(result.is_ok());
}

// ============================================================================
// Duplicate index creation test
// ============================================================================

#[tokio::test]
async fn test_create_same_index_twice() {
    let (_, _, manager) = create_manager();

    let index_def1 = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def1).await.unwrap();
    assert!(manager.has_indexes());

    // Create same index again - should add duplicate entry in IndexInfo
    let index_def2 = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def2).await.unwrap();

    // Index should still exist
    assert!(manager.has_indexes());
    assert!(manager.index_exists(1001));
}

#[tokio::test]
async fn test_create_same_unique_index_twice() {
    let (_, _, manager) = create_manager();

    let index_def1 = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def1).await.unwrap();
    assert!(manager.has_unique_indexes());

    // Create same unique index again
    let index_def2 = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def2).await.unwrap();

    assert!(manager.has_unique_indexes());
    assert!(manager.unique_index_exists(1001));
}

// ============================================================================
// Deep nesting path test
// ============================================================================

#[tokio::test]
async fn test_deeply_nested_path_index() {
    let (data_store, _, manager) = create_manager();

    // Create deeply nested structure: level1.level2.level3.level4.level5 = "deep"
    let deep_value = InnerValue::Str("deep_value".to_string());
    let level5 = create_nested_value(&[50], deep_value.clone());
    let level4 = create_nested_value(&[40], level5);
    let level3 = create_nested_value(&[30], level4);
    let level2 = create_nested_value(&[20], level3);
    let level1 = create_test_value(&[(10, level2)]);

    let record_id = RecordId::new();
    data_store
        .set(record_id.to_bytes(), level1.to_bytes())
        .await
        .unwrap();

    // Create index on path [10, 20, 30, 40, 50]
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![10, 20, 30, 40, 50])]);
    manager.create_index(index_def).await.unwrap();

    // Lookup by deep value
    let result = manager.lookup_by_index(1001, &[deep_value]).await.unwrap();
    assert_eq!(result.len(), 1);
    assert!(result.contains(&record_id));
}

#[tokio::test]
async fn test_deeply_nested_unique_index() {
    let (_, _, manager) = create_manager();

    let deep_value = InnerValue::Int(999);
    let level5 = create_nested_value(&[50], deep_value.clone());
    let level4 = create_nested_value(&[40], level5);
    let level3 = create_nested_value(&[30], level4);
    let level2 = create_nested_value(&[20], level3);
    let level1 = create_test_value(&[(10, level2)]);

    // Create unique index on deep path
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![10, 20, 30, 40, 50])]);
    manager.create_unique_index(index_def).await.unwrap();

    let record_id = RecordId::new();
    manager
        .on_record_created_unique(&record_id, &level1)
        .await
        .unwrap();

    // Lookup
    let result = manager
        .lookup_by_unique_index(1001, &[deep_value])
        .await
        .unwrap();
    assert!(result.is_some());
    assert_eq!(result.unwrap(), record_id);
}

// ============================================================================
// Delete last record from BTreeSet - verify key cleanup
// ============================================================================

#[tokio::test]
async fn test_delete_last_record_removes_index_key() {
    let (_, info_store, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    let value = create_test_value(&[(1, InnerValue::Str("unique_value".to_string()))]);
    let record_id = RecordId::new();

    // Add record
    manager.on_record_created(&record_id, &value).await.unwrap();

    // Verify index entry exists by lookup
    let before = manager
        .lookup_by_index(1001, &[InnerValue::Str("unique_value".to_string())])
        .await
        .unwrap();
    assert_eq!(before.len(), 1);

    // Delete the only record
    manager.on_record_deleted(&record_id, &value).await.unwrap();

    // Verify index entry is gone
    let after = manager
        .lookup_by_index(1001, &[InnerValue::Str("unique_value".to_string())])
        .await
        .unwrap();
    assert!(after.is_empty());

    // Verify key is actually removed from store (not just empty BTreeSet)
    // The index key should not exist at all
    use crate::db::engine::index::index_record_key::IndexRecordKey;
    let index_key = IndexRecordKey::new(false, 1001)
        .with_values(&[&InnerValue::Str("unique_value".to_string())])
        .to_bytes();
    let store_result = info_store.get(index_key).await;
    assert!(
        matches!(store_result, Err(crate::db::DbError::NotFound(_))),
        "Index key should be removed from store after last record deleted"
    );
}

// ============================================================================
// Concurrency test
// ============================================================================

#[tokio::test]
async fn test_concurrent_index_operations() {
    let (_, _, manager) = create_manager();

    // Create index first
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    // Spawn multiple concurrent tasks
    let mut handles = Vec::new();
    for i in 0..10 {
        let mgr = manager.clone();
        let handle = tokio::spawn(async move {
            let value = create_test_value(&[(1, InnerValue::Int(i))]);
            let record_id = RecordId::new();

            // Create
            mgr.on_record_created(&record_id, &value).await.unwrap();

            // Lookup
            let result = mgr
                .lookup_by_index(1001, &[InnerValue::Int(i)])
                .await
                .unwrap();
            assert_eq!(result.len(), 1);

            // Delete
            mgr.on_record_deleted(&record_id, &value).await.unwrap();

            // Verify deleted
            let result = mgr
                .lookup_by_index(1001, &[InnerValue::Int(i)])
                .await
                .unwrap();
            assert!(result.is_empty());
        });
        handles.push(handle);
    }

    // Wait for all tasks
    for handle in handles {
        handle.await.unwrap();
    }
}

#[tokio::test]
async fn test_concurrent_unique_index_validation() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def).await.unwrap();

    // One record exists
    let existing = create_test_value(&[(1, InnerValue::Str("exists".to_string()))]);
    let existing_id = RecordId::new();
    manager
        .on_record_created_unique(&existing_id, &existing)
        .await
        .unwrap();

    // Multiple concurrent validations
    let mut handles = Vec::new();
    for _ in 0..5 {
        let mgr = manager.clone();
        let val = existing.clone();
        let handle = tokio::spawn(async move {
            // All should fail because value exists
            let result = mgr.validate_unique_for_create(&val).await;
            result
        });
        handles.push(handle);
    }

    // All should fail
    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.is_err());
    }
}

#[tokio::test]
async fn test_concurrent_reads_with_index() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    // Add some records
    for i in 0..20i64 {
        let value = create_test_value(&[(1, InnerValue::Int(i))]);
        let record_id = RecordId::new();
        manager.on_record_created(&record_id, &value).await.unwrap();
    }

    // Concurrent reads
    let mut handles = Vec::new();
    for _ in 0..10 {
        let mgr = manager.clone();
        let handle = tokio::spawn(async move {
            for j in 0..20i64 {
                let result = mgr
                    .lookup_by_index(1001, &[InnerValue::Int(j)])
                    .await
                    .unwrap();
                assert_eq!(result.len(), 1);
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.await.unwrap();
    }
}
