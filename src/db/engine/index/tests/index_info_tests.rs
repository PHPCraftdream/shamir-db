use crate::db::engine::index::index_info::IndexInfo;
use crate::db::engine::index::index_definition::IndexDefinition;
use crate::db::engine::index::index_info_item::IndexInfoItem;
use crate::codecs::bytes;

#[test]
fn test_selective_mode_with_definitions() {
    let simple_index = IndexDefinition::new("by_email", vec![IndexInfoItem::new(vec![1])]);
    let composite_index = IndexDefinition::new("by_city_and_age", vec![IndexInfoItem::new(vec![2]), IndexInfoItem::new(vec![3])]);

    let mut target = IndexInfo::selective(vec![simple_index.clone()]);
    assert!(target.is_enabled());
    assert_eq!(target.definitions().unwrap().len(), 1);

    target.add_index(composite_index.clone());
    assert_eq!(target.definitions().unwrap().len(), 2);
    assert!(target.definitions().unwrap().contains(&simple_index));
    assert!(target.definitions().unwrap().contains(&composite_index));
}

#[test]
fn test_add_and_remove_index() {
    let mut target = IndexInfo::disabled();
    let index1 = IndexDefinition::new("by_name", vec![IndexInfoItem::new(vec![1])]);
    let index2 = IndexDefinition::new("by_age", vec![IndexInfoItem::new(vec![2])]);

    target.add_index(index1.clone());
    assert!(target.is_enabled());
    assert!(target.definitions().is_some());
    assert_eq!(target.definitions().unwrap().len(), 1);

    target.add_index(index2.clone());
    assert_eq!(target.definitions().unwrap().len(), 2);

    // Test removing an index
    assert!(target.remove_index("by_name"));
    assert_eq!(target.definitions().unwrap().len(), 1);
    assert_eq!(target.definitions().unwrap()[0], index2);

    // Test removing last index
    assert!(target.remove_index("by_age"));
    assert!(!target.is_enabled());
}

#[test]
fn test_add_duplicate_name_replaces() {
    let mut target = IndexInfo::selective(vec![IndexDefinition::new("other", vec![])]);
    let index_v1 = IndexDefinition::new("my_index", vec![IndexInfoItem::new(vec![1])]);
    let index_v2 = IndexDefinition::new("my_index", vec![IndexInfoItem::new(vec![2])]);

    target.add_index(index_v1);
    assert_eq!(target.definitions().unwrap().len(), 2);
    assert_ne!(target.definitions().unwrap()[1], index_v2);

    target.add_index(index_v2.clone());
    assert_eq!(target.definitions().unwrap().len(), 2);
    assert_eq!(target.definitions().unwrap()[1], index_v2);
}

#[test]
fn test_serialization() {
    let index_def = IndexDefinition::new("by_email", vec![IndexInfoItem::new(vec![1])]);
    let target = IndexInfo::selective(vec![index_def]);
    target.mark_pending();

    let serialized = bincode::serialize(&target).unwrap();
    let deserialized: IndexInfo = bincode::deserialize(&serialized).unwrap();

    // PartialEq compares mode only
    assert_eq!(deserialized, target);
    // Status is not serialized and should be reset to default (Actual)
    assert_eq!(deserialized.status(), crate::db::engine::index::index_info::IndexStatus::Actual);
}

#[test]
fn test_roundtrip() {
    let index_def = IndexDefinition::new("by_email", vec![IndexInfoItem::new(vec![1, 2])]);
    let target = IndexInfo::selective(vec![index_def]);
    target.mark_pending();

    let bytes = bytes::to_bytes(&target).unwrap();
    let deserialized: IndexInfo = bytes::from_bytes(&bytes).unwrap();

    // Mode should be preserved (IndexInfo PartialEq compares mode only)
    assert_eq!(deserialized, target);
    // Status is omitted and should be reset to default (Actual)
    assert_eq!(deserialized.status(), crate::db::engine::index::index_info::IndexStatus::Actual);
    // Original target still has Pending status
    assert_eq!(target.status(), crate::db::engine::index::index_info::IndexStatus::Pending);
}

#[test]
fn test_zero_copy() {
    let index_def = IndexDefinition::new("composite", vec![
        IndexInfoItem::new(vec![1]),
        IndexInfoItem::new(vec![2, 3]),
    ]);
    let target = IndexInfo::selective(vec![index_def]);

    let bytes = bytes::to_bytes(&target).unwrap();
    let info2 = bytes::from_bytes::<IndexInfo>(&bytes).unwrap();

    // Can access mode without allocation - IndexInfo PartialEq compares mode
    assert_eq!(info2, target);
}
