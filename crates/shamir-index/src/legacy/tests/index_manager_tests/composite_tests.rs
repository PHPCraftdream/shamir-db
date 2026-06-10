use super::helpers::{create_manager, create_nested_value, create_test_value};
use crate::legacy::index_definition::IndexDefinition;
use crate::legacy::index_info_item::IndexInfoItem;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

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
        .set(record_id.to_bytes(), level1.to_bytes().unwrap())
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
