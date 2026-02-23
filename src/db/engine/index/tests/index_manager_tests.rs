use crate::db::engine::index::index_definition::IndexDefinition;
use crate::db::engine::index::index_info::IndexInfo;
use crate::db::engine::index::index_info_item::IndexInfoItem;
use crate::db::engine::index::index_manager::IndexManager;
use crate::db::storage::storage_in_memory::InMemoryStore;
use crate::db::storage::types::Store;
use crate::types::common::new_map;
use crate::types::record_id::RecordId;
use crate::types::value::InnerValue;
use crate::core::interner::InternerKey;
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
        vec![
            IndexInfoItem::new(vec![1]),
            IndexInfoItem::new(vec![2]),
        ],
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

    manager
        .on_record_deleted(&record_id, &value)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_on_record_deleted_without_index() {
    let (_, _, manager) = create_manager();

    let value = create_test_value(&[(1, InnerValue::Str("test".to_string()))]);
    let record_id = RecordId::new();

    manager
        .on_record_deleted(&record_id, &value)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_on_record_deleted_missing_field() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();

    // Record doesn't have the indexed field
    let value = create_test_value(&[(2, InnerValue::Int(42))]);
    let record_id = RecordId::new();

    manager
        .on_record_deleted(&record_id, &value)
        .await
        .unwrap();
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
