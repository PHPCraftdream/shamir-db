use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_info::IndexInfo;
use crate::base_index::index_info_item::IndexInfoItem;
use crate::base_index::index_status::IndexStatus;
use shamir_types::codecs::basic::bincode;

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

#[test]
fn rv_index_8f977bf5_snapshot_shares_one_arc_no_per_definition_clone() {
    let target = IndexInfo::from_definitions(vec![
        IndexDefinition::new(1011, vec![IndexInfoItem::new(vec![1])]),
        IndexDefinition::new(1012, vec![IndexInfoItem::new(vec![2])]),
    ]);

    let snap = target.snapshot();
    assert_eq!(snap.len(), 2);
    // A definition clone copies the owned paths; shared snapshot identity alone
    // does not prove a planner iterates the borrowed snapshot.
    let snap_again = target.snapshot();
    assert!(
        std::sync::Arc::ptr_eq(&snap, &snap_again),
        "snapshot() must return the shared node-local Arc, not a rebuilt Vec"
    );
    let names: Vec<u64> = snap.iter().map(|d| d.name_interned).collect();
    assert!(names.contains(&1011) && names.contains(&1012));
}
