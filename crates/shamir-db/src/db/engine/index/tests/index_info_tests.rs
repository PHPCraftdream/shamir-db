use crate::codecs::basic::bincode;
use crate::db::engine::index::index_definition::IndexDefinition;
use crate::db::engine::index::index_info::IndexInfo;
use crate::db::engine::index::index_info_item::IndexInfoItem;
use crate::db::engine::index::index_status::IndexStatus;

#[test]
fn test_selective_mode_with_definitions() {
    let simple_index = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    let composite_index = IndexDefinition::new(
        1002,
        vec![IndexInfoItem::new(vec![2]), IndexInfoItem::new(vec![3])],
    );

    let target = IndexInfo::from_definitions(vec![simple_index.clone()]);
    assert!(target.is_enabled());
    assert_eq!(target.len(), 1);

    target.add_index(composite_index.clone());
    assert_eq!(target.len(), 2);
    assert!(target.contains(1001));
    assert!(target.contains(1002));
}

#[test]
fn test_add_and_remove_index() {
    let target = IndexInfo::new();
    let index1 = IndexDefinition::new(1003, vec![IndexInfoItem::new(vec![1])]);
    let index2 = IndexDefinition::new(1004, vec![IndexInfoItem::new(vec![2])]);

    target.add_index(index1.clone());
    assert!(target.is_enabled());
    assert_eq!(target.len(), 1);

    target.add_index(index2.clone());
    assert_eq!(target.len(), 2);

    // Test removing an index
    assert!(target.remove_index(1003));
    assert_eq!(target.len(), 1);
    assert_eq!(target.get_index(1004), Some(index2));

    // Test removing last index
    assert!(target.remove_index(1004));
    assert!(!target.is_enabled());
}

#[test]
fn test_add_duplicate_name_replaces() {
    let target = IndexInfo::from_definitions(vec![IndexDefinition::new(1005, vec![])]);
    let index_v1 = IndexDefinition::new(1006, vec![IndexInfoItem::new(vec![1])]);
    let index_v2 = IndexDefinition::new(1006, vec![IndexInfoItem::new(vec![2])]);

    target.add_index(index_v1);
    assert_eq!(target.len(), 2);
    assert_ne!(target.get_index(1006), Some(index_v2.clone()));

    target.add_index(index_v2.clone());
    assert_eq!(target.len(), 2);
    assert_eq!(target.get_index(1006), Some(index_v2));
}

#[test]
fn test_serialization() {
    let index_def = IndexDefinition::new(1008, vec![IndexInfoItem::new(vec![1])]);
    let target = IndexInfo::from_definitions(vec![index_def]);
    target.mark_pending();

    let serialized = bincode::to_bytes(&target).unwrap();
    let deserialized: IndexInfo = bincode::from_bytes(&serialized).unwrap();

    // PartialEq compares indexes only
    assert_eq!(deserialized, target);
    // Status is not serialized and should be reset to default (Actual)
    assert_eq!(deserialized.status(), IndexStatus::Actual);
}

#[test]
fn test_roundtrip() {
    let index_def = IndexDefinition::new(1009, vec![IndexInfoItem::new(vec![1, 2])]);
    let target = IndexInfo::from_definitions(vec![index_def]);
    target.mark_pending();

    let bytes = bincode::to_bytes(&target).unwrap();
    let deserialized: IndexInfo = bincode::from_bytes(&bytes).unwrap();

    // Indexes should be preserved
    assert_eq!(deserialized, target);
    // Status is omitted and should be reset to default (Actual)
    assert_eq!(deserialized.status(), IndexStatus::Actual);
    // Original target still has Pending status
    assert_eq!(target.status(), IndexStatus::Pending);
}

#[test]
fn test_zero_copy() {
    let index_def = IndexDefinition::new(
        1010,
        vec![IndexInfoItem::new(vec![1]), IndexInfoItem::new(vec![2, 3])],
    );
    let target = IndexInfo::from_definitions(vec![index_def]);

    let bytes = bincode::to_bytes(&target).unwrap();
    let info2 = bincode::from_bytes::<IndexInfo>(&bytes).unwrap();

    // Can access indexes without allocation - IndexInfo PartialEq compares indexes
    assert_eq!(info2, target);
}
