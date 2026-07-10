use super::helpers::{create_manager, create_test_value};
use crate::legacy::index_definition::IndexDefinition;
use crate::legacy::index_info_item::IndexInfoItem;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

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
// Delete last record from BTreeSet - verify key cleanup
// ============================================================================

#[tokio::test]
async fn test_delete_last_record_removes_index_key() {
    use crate::legacy::index_record_key::IndexRecordKey;

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
    let index_key = IndexRecordKey::new(false, 1001)
        .with_values(&[&InnerValue::Str("unique_value".to_string())])
        .to_bytes();
    let store_result = info_store.get(index_key.into()).await;
    assert!(
        matches!(
            store_result,
            Err(shamir_storage::error::DbError::NotFound(_))
        ),
        "Index key should be removed from store after last record deleted"
    );
}
