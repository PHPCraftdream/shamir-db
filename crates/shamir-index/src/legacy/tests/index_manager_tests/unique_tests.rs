use super::helpers::{create_manager, create_test_value};
use crate::legacy::index_definition::IndexDefinition;
use crate::legacy::index_info_item::IndexInfoItem;
use crate::legacy::index_keys::build_index_key;
use crate::legacy::index_manager::IndexManager;
use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::sync::Arc;

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
        .set(id1.to_bytes().into(), value1.to_bytes().unwrap())
        .await
        .unwrap();
    data_store
        .set(id2.to_bytes().into(), value2.to_bytes().unwrap())
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
        .set(id1.to_bytes().into(), value.to_bytes().unwrap())
        .await
        .unwrap();
    data_store
        .set(id2.to_bytes().into(), value.to_bytes().unwrap())
        .await
        .unwrap();
    data_store
        .set(id3.to_bytes().into(), value.to_bytes().unwrap())
        .await
        .unwrap();

    // Create unique index - should fail
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    let result = manager.create_unique_index(index_def).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    // Should be UniqueIndexCreationFailed with count=3 and sample value
    match &err {
        shamir_storage::error::DbError::UniqueIndexCreationFailed(index_name, count, sample) => {
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
        shamir_storage::error::DbError::DuplicateKey(_)
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
        shamir_storage::error::DbError::DuplicateKey(_)
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
        .set(id1.to_bytes().into(), value.to_bytes().unwrap())
        .await
        .unwrap();
    data_store
        .set(id2.to_bytes().into(), value.to_bytes().unwrap())
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
        shamir_storage::error::DbError::UniqueIndexCreationFailed(name, count, _sample) => {
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
        data_store
            .set(id.to_bytes().into(), v.to_bytes().unwrap())
            .await
            .unwrap();
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
// validate_unique_for_create_with_defs — unit tests
// ============================================================================

#[tokio::test]
async fn test_validate_unique_for_create_with_defs_empty_defs_early_exit() {
    let (_, _, manager) = create_manager();

    // Even if a unique index exists, empty defs slice → Ok immediately.
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def).await.unwrap();

    let value = create_test_value(&[(1, InnerValue::Str("Alice".to_string()))]);
    // Seed a record so a real check would fail.
    let rid = RecordId::new();
    manager
        .on_record_created_unique(&rid, &value)
        .await
        .unwrap();

    // Empty defs — should return Ok regardless of index state.
    let result = manager
        .validate_unique_for_create_with_defs(&value, &[])
        .await;
    assert!(
        result.is_ok(),
        "empty defs must return Ok, got: {:?}",
        result
    );
}

#[tokio::test]
async fn test_validate_unique_for_create_with_defs_single_def_accept() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager
        .create_unique_index(index_def.clone())
        .await
        .unwrap();

    let existing = create_test_value(&[(1, InnerValue::Str("Alice".to_string()))]);
    let rid = RecordId::new();
    manager
        .on_record_created_unique(&rid, &existing)
        .await
        .unwrap();

    // Different value — should pass.
    let new_value = create_test_value(&[(1, InnerValue::Str("Bob".to_string()))]);
    let defs = vec![index_def];
    let result = manager
        .validate_unique_for_create_with_defs(&new_value, &defs)
        .await;
    assert!(
        result.is_ok(),
        "non-duplicate should pass, got: {:?}",
        result
    );
}

#[tokio::test]
async fn test_validate_unique_for_create_with_defs_single_def_reject() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager
        .create_unique_index(index_def.clone())
        .await
        .unwrap();

    let value = create_test_value(&[(1, InnerValue::Str("Alice".to_string()))]);
    let rid = RecordId::new();
    manager
        .on_record_created_unique(&rid, &value)
        .await
        .unwrap();

    // Same value — must reject with DuplicateKey.
    let defs = vec![index_def];
    let result = manager
        .validate_unique_for_create_with_defs(&value, &defs)
        .await;
    assert!(result.is_err(), "duplicate should be rejected");
    assert!(
        matches!(
            result.unwrap_err(),
            shamir_storage::error::DbError::DuplicateKey(_)
        ),
        "expected DuplicateKey error"
    );
}

#[tokio::test]
async fn test_validate_unique_for_create_with_defs_multi_def_trip_on_second() {
    let (_, _, manager) = create_manager();

    let def1 = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    let def2 = IndexDefinition::new(1002, vec![IndexInfoItem::new(vec![2])]);
    manager.create_unique_index(def1.clone()).await.unwrap();
    manager.create_unique_index(def2.clone()).await.unwrap();

    // Seed "Alice" in index 1001 and "X" in index 1002.
    let existing = create_test_value(&[
        (1, InnerValue::Str("Alice".to_string())),
        (2, InnerValue::Str("X".to_string())),
    ]);
    let rid = RecordId::new();
    manager
        .on_record_created_unique(&rid, &existing)
        .await
        .unwrap();

    // New value has unique field-1 but duplicates field-2 — should trip on def2.
    let new_value = create_test_value(&[
        (1, InnerValue::Str("Bob".to_string())),
        (2, InnerValue::Str("X".to_string())),
    ]);
    let defs = vec![def1, def2];
    let result = manager
        .validate_unique_for_create_with_defs(&new_value, &defs)
        .await;
    assert!(result.is_err(), "should trip on the second def");
    assert!(
        matches!(
            result.unwrap_err(),
            shamir_storage::error::DbError::DuplicateKey(_)
        ),
        "expected DuplicateKey error"
    );
}

// ============================================================================
// plan_* tests (Stage 1.1.E) — unique index collision
// ============================================================================

#[tokio::test]
async fn unique_index_collision_in_plan_phase() {
    let data = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let info = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
    let mgr = IndexManager::new(Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();

    let idx = IndexDefinition::new(5001, vec![IndexInfoItem::new(vec![1])]);
    mgr.create_unique_index(idx).await.unwrap();

    let rid1 = RecordId::new();
    let val = create_test_value(&[(1, InnerValue::Str("dup".to_string()))]);

    // Insert the first record.
    mgr.validate_unique_for_create(&val).await.unwrap();
    mgr.on_record_created_unique(&rid1, &val).await.unwrap();

    // Second record with same value — should fail with DuplicateKey.
    let result = mgr.validate_unique_for_create(&val).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        shamir_storage::error::DbError::DuplicateKey(msg) => {
            assert!(msg.contains("5001"), "error should mention index name");
        }
        other => panic!("expected DuplicateKey, got {:?}", other),
    }
}

// ============================================================================
// P0-4 (#960) — corrupt unique posting must fail CLOSED (Err), not Ok(None)
// ============================================================================
//
// A unique-index posting value MUST be exactly a 16-byte RecordId. Any other
// length is genuine corruption. The fix changes `check_unique_key`'s prior
// behavior (which logged a warning and returned `Ok(None)` — the SAME signal
// callers use for "key is free") to surface a typed `DbError::Codec`, so a
// write that would have relied on this check aborts instead of silently
// committing a duplicate on top of corrupted storage. These tests prove:
//   * every malformed length (0/15/17/64) surfaces an error via the lookup
//     path (`lookup_by_unique_index` → `check_unique_constraint` →
//     `check_unique_key`);
//   * the genuine not-found path (key never written) is UNCHANGED and still
//     returns `Ok(None)`;
//   * a well-formed 16-byte posting still decodes to `Ok(Some(RecordId))`;
//   * `validate_unique_for_create` / `validate_unique_for_update` against a
//     corrupted posting ABORT (Err), not silently succeed.

/// Write `corrupt_value` into the info_store at the exact 25-byte unique-index
/// key that `check_unique_key` reads for `(name_interned, lookup_values)`.
/// Simulates a torn/corrupt posting left behind by a crash or a buggy writer.
async fn seed_corrupt_unique_posting(
    info_store: &Arc<dyn Store>,
    name_interned: u64,
    lookup_values: &[InnerValue],
    corrupt_value: Bytes,
) {
    let index_key = build_index_key(true, name_interned, lookup_values).to_bytes();
    info_store
        .set(index_key.into(), corrupt_value)
        .await
        .unwrap();
}

/// Shared harness: build a manager with one unique index on field 1 and return
/// its info_store handle (for seeding corruption) plus the manager.
async fn manager_with_unique_index() -> (Arc<dyn Store>, IndexManager) {
    let (_, info_store, manager) = create_manager();
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def).await.unwrap();
    (info_store, manager)
}

#[tokio::test]
async fn corrupt_unique_posting_length_0_errors() {
    let (info_store, manager) = manager_with_unique_index().await;
    seed_corrupt_unique_posting(
        &info_store,
        1001,
        &[InnerValue::Str("Alice".to_string())],
        Bytes::new(),
    )
    .await;

    let result = manager
        .lookup_by_unique_index(1001, &[InnerValue::Str("Alice".to_string())])
        .await;
    assert!(result.is_err(), "0-byte posting must error, not Ok(None)");
    assert!(
        matches!(
            result.as_ref().unwrap_err(),
            shamir_storage::error::DbError::Codec(_)
        ),
        "expected Codec corruption error, got {:?}",
        result
    );
}

#[tokio::test]
async fn corrupt_unique_posting_length_15_errors() {
    let (info_store, manager) = manager_with_unique_index().await;
    seed_corrupt_unique_posting(
        &info_store,
        1001,
        &[InnerValue::Str("Alice".to_string())],
        Bytes::from_static(&[0xAB; 15]),
    )
    .await;

    let result = manager
        .lookup_by_unique_index(1001, &[InnerValue::Str("Alice".to_string())])
        .await;
    assert!(result.is_err(), "15-byte posting must error, not Ok(None)");
    assert!(
        matches!(
            result.as_ref().unwrap_err(),
            shamir_storage::error::DbError::Codec(_)
        ),
        "expected Codec corruption error, got {:?}",
        result
    );
}

#[tokio::test]
async fn corrupt_unique_posting_length_17_errors() {
    let (info_store, manager) = manager_with_unique_index().await;
    seed_corrupt_unique_posting(
        &info_store,
        1001,
        &[InnerValue::Str("Alice".to_string())],
        Bytes::from_static(&[0xCD; 17]),
    )
    .await;

    let result = manager
        .lookup_by_unique_index(1001, &[InnerValue::Str("Alice".to_string())])
        .await;
    assert!(result.is_err(), "17-byte posting must error, not Ok(None)");
    assert!(
        matches!(
            result.as_ref().unwrap_err(),
            shamir_storage::error::DbError::Codec(_)
        ),
        "expected Codec corruption error, got {:?}",
        result
    );
}

#[tokio::test]
async fn corrupt_unique_posting_length_64_errors() {
    let (info_store, manager) = manager_with_unique_index().await;
    seed_corrupt_unique_posting(
        &info_store,
        1001,
        &[InnerValue::Str("Alice".to_string())],
        Bytes::from_static(&[0xEF; 64]),
    )
    .await;

    let result = manager
        .lookup_by_unique_index(1001, &[InnerValue::Str("Alice".to_string())])
        .await;
    assert!(result.is_err(), "64-byte posting must error, not Ok(None)");
    assert!(
        matches!(
            result.as_ref().unwrap_err(),
            shamir_storage::error::DbError::Codec(_)
        ),
        "expected Codec corruption error, got {:?}",
        result
    );
}

#[tokio::test]
async fn genuine_not_found_unique_posting_returns_ok_none() {
    // Regression guard: the genuine "key is free" path (key NEVER written)
    // must remain Ok(None) — the corruption fix must NOT bleed into the
    // legitimate NotFound arm.
    let (_, manager) = manager_with_unique_index().await;

    let result = manager
        .lookup_by_unique_index(1001, &[InnerValue::Str("NeverWritten".to_string())])
        .await;
    assert!(
        result.is_ok(),
        "genuine NotFound must remain Ok, got {:?}",
        result
    );
    assert!(
        result.unwrap().is_none(),
        "a never-written key must resolve to None (free)"
    );
}

#[tokio::test]
async fn valid_16_byte_unique_posting_returns_ok_some() {
    // Positive control: a well-formed 16-byte RecordId posting decodes
    // correctly — the corruption fix did not break the happy path.
    let (info_store, manager) = manager_with_unique_index().await;
    let rid = RecordId::new();
    seed_corrupt_unique_posting(
        &info_store,
        1001,
        &[InnerValue::Str("Alice".to_string())],
        Bytes::copy_from_slice(rid.as_bytes()),
    )
    .await;

    let result = manager
        .lookup_by_unique_index(1001, &[InnerValue::Str("Alice".to_string())])
        .await
        .unwrap();
    assert_eq!(
        result,
        Some(rid),
        "16-byte posting must decode to its RecordId"
    );
}

#[tokio::test]
async fn corrupt_unique_posting_aborts_validate_create() {
    // A create against a corrupted unique posting must ABORT (Err), not
    // silently pass validation and commit a duplicate on top of corruption.
    let (info_store, manager) = manager_with_unique_index().await;
    seed_corrupt_unique_posting(
        &info_store,
        1001,
        &[InnerValue::Str("Alice".to_string())],
        Bytes::from_static(&[0x99; 12]),
    )
    .await;

    let value = create_test_value(&[(1, InnerValue::Str("Alice".to_string()))]);
    let result = manager.validate_unique_for_create(&value).await;
    assert!(
        result.is_err(),
        "create validation against a corrupt posting must abort, got {:?}",
        result
    );
    assert!(
        matches!(
            result.unwrap_err(),
            shamir_storage::error::DbError::Codec(_)
        ),
        "expected Codec corruption error to propagate to the create path"
    );
}

#[tokio::test]
async fn corrupt_unique_posting_aborts_validate_update() {
    // An update whose NEW value collides with a corrupted posting must ABORT
    // (Err). The update path (`validate_unique_for_update`) reads the same
    // posting via `check_unique_key`, so corruption must propagate there too.
    let (info_store, manager) = manager_with_unique_index().await;
    seed_corrupt_unique_posting(
        &info_store,
        1001,
        &[InnerValue::Str("Alice".to_string())],
        Bytes::from_static(&[0x77; 20]),
    )
    .await;

    let record_id = RecordId::new();
    // The updating record currently holds a DIFFERENT value ("Bob"), and wants
    // to move to the corrupted value ("Alice"). The check_unique_key read of
    // "Alice" must surface the corruption error.
    let old_value = create_test_value(&[(1, InnerValue::Str("Bob".to_string()))]);
    let new_value = create_test_value(&[(1, InnerValue::Str("Alice".to_string()))]);
    let result = manager
        .validate_unique_for_update(&record_id, &old_value, &new_value)
        .await;
    assert!(
        result.is_err(),
        "update validation against a corrupt posting must abort, got {:?}",
        result
    );
    assert!(
        matches!(
            result.unwrap_err(),
            shamir_storage::error::DbError::Codec(_)
        ),
        "expected Codec corruption error to propagate to the update path"
    );
}
