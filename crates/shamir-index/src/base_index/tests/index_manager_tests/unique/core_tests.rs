use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_info_item::IndexInfoItem;
use crate::base_index::index_manager::IndexManager;
use crate::base_index::tests::index_manager_tests::helpers::{create_manager, create_test_value};
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
    let result = manager.drop_unique_index(1001, None, None).await.unwrap();
    assert!(result);
    assert!(!manager.has_unique_indexes());

    // Drop non-existent
    let result = manager.drop_unique_index(1002, None, None).await.unwrap();
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
        .unwrap()
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

// ---------------------------------------------------------------------------
// RV-INDEX-0577E3C4 — direct unique update batching
// ---------------------------------------------------------------------------

struct CountingUniqueStore {
    inner: Arc<dyn Store>,
    transacts: std::sync::atomic::AtomicUsize,
    direct_sets: std::sync::atomic::AtomicUsize,
    direct_removes: std::sync::atomic::AtomicUsize,
}

impl CountingUniqueStore {
    fn new(inner: Arc<dyn Store>) -> Self {
        Self {
            inner,
            transacts: std::sync::atomic::AtomicUsize::new(0),
            direct_sets: std::sync::atomic::AtomicUsize::new(0),
            direct_removes: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

type UniqueTestStream = std::pin::Pin<
    Box<
        dyn futures::Stream<
                Item = shamir_storage::error::DbResult<
                    Vec<(shamir_storage::types::RecordKey, bytes::Bytes)>,
                >,
            > + Send,
    >,
>;

#[async_trait::async_trait]
impl Store for CountingUniqueStore {
    async fn insert(
        &self,
        value: Bytes,
    ) -> shamir_storage::error::DbResult<shamir_storage::types::RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(
        &self,
        key: shamir_storage::types::RecordKey,
        value: Bytes,
    ) -> shamir_storage::error::DbResult<bool> {
        self.direct_sets
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.set(key, value).await
    }
    async fn get(
        &self,
        key: shamir_storage::types::RecordKey,
    ) -> shamir_storage::error::DbResult<Bytes> {
        self.inner.get(key).await
    }
    async fn remove(
        &self,
        key: shamir_storage::types::RecordKey,
    ) -> shamir_storage::error::DbResult<bool> {
        self.direct_removes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.remove(key).await
    }
    fn iter_stream(&self, batch_size: usize) -> UniqueTestStream {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> UniqueTestStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
    async fn transact(
        &self,
        ops: Vec<shamir_storage::types::KvOp>,
    ) -> shamir_storage::error::DbResult<()> {
        self.transacts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.transact(ops).await
    }
}

/// Old-code failure: `on_record_updated_unique` awaited
/// `remove_unique_entry_by_key` then `add_unique_entry_by_key` per changed
/// def — two separate store round-trips, zero transacts. The fix applies the
/// mixed remove-then-set batch as ONE ordered `transact`.
#[tokio::test]
async fn rv_index_0577e3c4_unique_update_batches_remove_and_set_into_one_transact() {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let counting = Arc::new(CountingUniqueStore::new(Arc::clone(&inner)));
    let manager = IndexManager::new(Arc::clone(&data), Arc::clone(&counting) as Arc<dyn Store>)
        .await
        .unwrap();
    manager
        .create_unique_index(IndexDefinition::new(
            1001,
            vec![IndexInfoItem::new(vec![1])],
        ))
        .await
        .unwrap();

    let id = RecordId::new();
    let old = create_test_value(&[(1, InnerValue::Int(10))]);
    let new = create_test_value(&[(1, InnerValue::Int(20))]);
    manager.on_record_created_unique(&id, &old).await.unwrap();
    let transacts_after_create = counting.transacts.load(std::sync::atomic::Ordering::SeqCst);
    let direct_sets_after_create = counting
        .direct_sets
        .load(std::sync::atomic::Ordering::SeqCst);

    manager
        .on_record_updated_unique(&id, &old, &new)
        .await
        .unwrap();

    assert_eq!(
        counting.transacts.load(std::sync::atomic::Ordering::SeqCst),
        transacts_after_create + 1,
        "the unique update must apply remove+set as ONE transact"
    );
    assert_eq!(
        counting
            .direct_sets
            .load(std::sync::atomic::Ordering::SeqCst),
        direct_sets_after_create,
        "no per-key direct set may bypass the batch"
    );
    assert_eq!(
        counting
            .direct_removes
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no per-key direct remove may bypass the batch"
    );

    // Correctness: old key gone, new key resolves to the same record id.
    let old_id = manager
        .lookup_by_unique_index(1001, &[InnerValue::Int(10)])
        .await
        .unwrap();
    assert!(old_id.is_none(), "old unique posting must be removed");
    let new_id = manager
        .lookup_by_unique_index(1001, &[InnerValue::Int(20)])
        .await
        .unwrap();
    assert_eq!(new_id, Some(id), "new unique posting must resolve the id");
}
