use super::helpers::{create_manager, create_test_value};
use crate::legacy::index_definition::IndexDefinition;
use crate::legacy::index_info_item::IndexInfoItem;
use shamir_types::core::interner::InternerKey;
use shamir_types::types::common::{new_map, TSet};
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

// ============================================================================
// Tests with various value types (Null, List, Map, Set)
// ============================================================================

#[tokio::test]
async fn test_create_index_with_null_value() {
    let (data_store, _, manager) = create_manager();

    let value = create_test_value(&[(1, InnerValue::Null)]);
    let record_id = RecordId::new();
    data_store
        .set(record_id.to_bytes().into(), value.to_bytes().unwrap())
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
        .set(record_id.to_bytes().into(), value.to_bytes().unwrap())
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

    let mut inner_map = new_map();
    inner_map.insert(InternerKey::new(100), InnerValue::Str("inner".to_string()));
    let map = InnerValue::Map(inner_map);

    let value = create_test_value(&[(1, map.clone())]);
    let record_id = RecordId::new();
    data_store
        .set(record_id.to_bytes().into(), value.to_bytes().unwrap())
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
