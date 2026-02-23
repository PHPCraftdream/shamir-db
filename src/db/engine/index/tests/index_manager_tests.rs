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
        (4, InnerValue::Nil),
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
