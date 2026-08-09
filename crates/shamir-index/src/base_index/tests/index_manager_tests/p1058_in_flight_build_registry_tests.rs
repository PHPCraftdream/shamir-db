// #1058: In-flight online build registry + dirty-set capture tests.
//
// Tests the regular (hash) family only — not SortedIndexManager or unique-family
// logic beyond what the shared planning methods already cover.
//
// Validates that writes during an in-flight online build are captured to the
// dirty-set instead of producing direct posting ops, and that normal behavior
// is preserved for indexes not mid-build.

use super::helpers::{create_manager, create_test_value};
use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_info_item::IndexInfoItem;
use crate::write_ops::IndexWriteOp;
use crate::IndexState;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

// ============================================================================
// Scenario 1: Index not in in-flight registry → dirty-set never grows,
// SetPosting/RemovePosting still produced exactly as before this change.
// ============================================================================

#[tokio::test]
async fn test_scenario_1_not_in_flight_normal_ops() {
    let (_, _, manager) = create_manager();

    // Create a regular index on field 1.
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def.clone()).await.unwrap();

    // Do NOT mark the index as in-flight.
    // Verify is_build_in_flight returns false.
    assert!(!manager.is_build_in_flight(1001));

    // Write a record touching field 1.
    let rid = RecordId::new();
    let value = create_test_value(&[(1, InnerValue::Str("alice".into()))]);
    let ops = manager.plan_record_created(&rid, &value).await.unwrap();

    // Verify SetPosting is produced (normal behavior).
    assert_eq!(ops.len(), 1);
    match &ops[0] {
        IndexWriteOp::SetPosting { .. } => {}
        _ => panic!("Expected SetPosting, got {:?}", ops[0]),
    }

    // Verify dirty-set is empty (never created since not in-flight).
    let dirty = manager.dirty_sets.lock().unwrap();
    assert!(dirty.is_empty());
}

#[tokio::test]
async fn test_scenario_1_not_in_flight_delete_normal_ops() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def.clone()).await.unwrap();

    let rid = RecordId::new();
    let value = create_test_value(&[(1, InnerValue::Str("alice".into()))]);
    manager.on_record_created(&rid, &value).await.unwrap();

    // Delete the record (not in-flight).
    let ops = manager.plan_record_deleted(&rid, &value).await.unwrap();

    // Verify RemovePosting is produced.
    assert_eq!(ops.len(), 1);
    match &ops[0] {
        IndexWriteOp::RemovePosting { .. } => {}
        _ => panic!("Expected RemovePosting, got {:?}", ops[0]),
    }

    // Verify dirty-set is still empty.
    let dirty = manager.dirty_sets.lock().unwrap();
    assert!(dirty.is_empty());
}

#[tokio::test]
async fn test_scenario_1_not_in_flight_update_normal_ops() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def.clone()).await.unwrap();

    let rid = RecordId::new();
    let old_value = create_test_value(&[(1, InnerValue::Str("alice".into()))]);
    let new_value = create_test_value(&[(1, InnerValue::Str("bob".into()))]);

    // Update the record (not in-flight).
    let ops = manager
        .plan_record_updated(&rid, &old_value, &new_value)
        .await
        .unwrap();

    // Verify RemovePosting + SetPosting are produced.
    assert_eq!(ops.len(), 2);
    match &ops[0] {
        IndexWriteOp::RemovePosting { .. } => {}
        _ => panic!("Expected RemovePosting, got {:?}", ops[0]),
    }
    match &ops[1] {
        IndexWriteOp::SetPosting { .. } => {}
        _ => panic!("Expected SetPosting, got {:?}", ops[1]),
    }

    // Verify dirty-set is empty.
    let dirty = manager.dirty_sets.lock().unwrap();
    assert!(dirty.is_empty());
}

// ============================================================================
// Scenario 2: Index IS in in-flight registry → RecordId added to dirty-set,
// no SetPosting/RemovePosting produced for that specific def.
// ============================================================================

#[tokio::test]
async fn test_scenario_2_in_flight_capture_to_dirty_set() {
    let (_, _, manager) = create_manager();

    // Create an index and mark it as Building + in-flight.
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def.clone()).await.unwrap();

    // Use add_index to replace the Ready definition with a Building one.
    let mut building_def = index_def.clone();
    building_def.state = IndexState::Building;
    manager.indexes.add_index(building_def);
    manager.mark_build_in_flight(1001);

    // Verify is_build_in_flight returns true.
    assert!(manager.is_build_in_flight(1001));

    // Write a record touching field 1.
    let rid = RecordId::new();
    let value = create_test_value(&[(1, InnerValue::Str("alice".into()))]);
    let ops = manager.plan_record_created(&rid, &value).await.unwrap();

    // Verify NO SetPosting is produced for this def.
    assert_eq!(ops.len(), 0);

    // Verify RecordId is in the dirty-set.
    let dirty = manager.dirty_sets.lock().unwrap();
    assert!(dirty.contains_key(&1001));
    let dirty_set = dirty.get(&1001).unwrap();
    let set = dirty_set.lock().unwrap();
    assert!(set.contains(&rid));
}

#[tokio::test]
async fn test_scenario_2_in_flight_delete_capture_to_dirty_set() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def.clone()).await.unwrap();

    // Manually set state to Building and mark in-flight.
    // Use add_index to replace the Ready definition with a Building one.
    let mut building_def = index_def.clone();
    building_def.state = IndexState::Building;
    manager.indexes.add_index(building_def);
    manager.mark_build_in_flight(1001);

    let rid = RecordId::new();
    let value = create_test_value(&[(1, InnerValue::Str("alice".into()))]);

    // Delete the record (in-flight).
    let ops = manager.plan_record_deleted(&rid, &value).await.unwrap();

    // Verify NO RemovePosting is produced.
    assert_eq!(ops.len(), 0);

    // Verify RecordId is in the dirty-set.
    let dirty = manager.dirty_sets.lock().unwrap();
    let dirty_set = dirty.get(&1001).unwrap();
    let set = dirty_set.lock().unwrap();
    assert!(set.contains(&rid));
}

#[tokio::test]
async fn test_scenario_2_in_flight_update_capture_to_dirty_set() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def.clone()).await.unwrap();

    // Manually set state to Building and mark in-flight.
    // Use add_index to replace the Ready definition with a Building one.
    let mut building_def = index_def.clone();
    building_def.state = IndexState::Building;
    manager.indexes.add_index(building_def);
    manager.mark_build_in_flight(1001);

    let rid = RecordId::new();
    let old_value = create_test_value(&[(1, InnerValue::Str("alice".into()))]);
    let new_value = create_test_value(&[(1, InnerValue::Str("bob".into()))]);

    // Update the record (in-flight).
    let ops = manager
        .plan_record_updated(&rid, &old_value, &new_value)
        .await
        .unwrap();

    // Verify NO RemovePosting/SetPosting are produced.
    assert_eq!(ops.len(), 0);

    // Verify RecordId is in the dirty-set.
    let dirty = manager.dirty_sets.lock().unwrap();
    let dirty_set = dirty.get(&1001).unwrap();
    let set = dirty_set.lock().unwrap();
    assert!(set.contains(&rid));
}

// ============================================================================
// Scenario 3: Write not touching in-flight index's fields → RecordId NOT in
// dirty-set (otherwise Phase C degrades into a full rescan of every write).
// ============================================================================

#[tokio::test]
async fn test_scenario_3_in_flight_not_touched_no_dirty_set() {
    let (_, _, manager) = create_manager();

    // Create an index on field 1.
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def.clone()).await.unwrap();

    // Mark as Building + in-flight.
    // Use add_index to replace the Ready definition with a Building one.
    let mut building_def = index_def.clone();
    building_def.state = IndexState::Building;
    manager.indexes.add_index(building_def);
    manager.mark_build_in_flight(1001);

    // Write a record touching field 2 (NOT field 1).
    let rid = RecordId::new();
    let value = create_test_value(&[(2, InnerValue::Str("unrelated".into()))]);
    let ops = manager.plan_record_created(&rid, &value).await.unwrap();

    // Verify NO SetPosting is produced (index on field 1, write on field 2).
    assert_eq!(ops.len(), 0);

    // Verify dirty-set is either empty or does NOT contain this RecordId.
    // The dirty-set entry is created lazily, so it might not even exist.
    let dirty = manager.dirty_sets.lock().unwrap();
    if let Some(dirty_set) = dirty.get(&1001) {
        let set = dirty_set.lock().unwrap();
        assert!(
            !set.contains(&rid),
            "RecordId should not be in dirty-set for unrelated write"
        );
    }
}

// ============================================================================
// Scenario 4: Two indexes on the same table, ONE in-flight and ONE Ready →
// a write touching both fields: the Ready index gets its normal SetPosting,
// the in-flight index's RecordId goes to its dirty-set — confirming a build
// in progress for one index does not degrade live support for a sibling.
// ============================================================================

#[tokio::test]
async fn test_scenario_4_two_indexes_one_in_flight_one_ready() {
    let (_, _, manager) = create_manager();

    // Create two indexes: one on field 1, one on field 2.
    let index1_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    let index2_def = IndexDefinition::new(1002, vec![IndexInfoItem::new(vec![2])]);
    manager.create_index(index1_def.clone()).await.unwrap();
    manager.create_index(index2_def.clone()).await.unwrap();

    // Mark index 1 as Building + in-flight, leave index 2 as Ready.
    let mut building_def = index1_def.clone();
    building_def.state = IndexState::Building;
    manager.indexes.add_index(building_def);
    manager.mark_build_in_flight(1001);

    // Write a record touching BOTH fields 1 and 2.
    let rid = RecordId::new();
    let value = create_test_value(&[
        (1, InnerValue::Str("alice".into())),
        (2, InnerValue::Int(42)),
    ]);
    let ops = manager.plan_record_created(&rid, &value).await.unwrap();

    // Verify exactly ONE SetPosting is produced (for index 2, which is Ready).
    assert_eq!(ops.len(), 1);
    match &ops[0] {
        IndexWriteOp::SetPosting { .. } => {}
        _ => panic!("Expected SetPosting, got {:?}", ops[0]),
    }

    // Verify index 1's dirty-set contains the RecordId.
    let dirty = manager.dirty_sets.lock().unwrap();
    let dirty_set_1 = dirty.get(&1001).unwrap();
    let set_1 = dirty_set_1.lock().unwrap();
    assert!(set_1.contains(&rid));

    // Verify index 2 has no dirty-set entry (Ready, not in-flight).
    assert!(!dirty.contains_key(&1002));
}

// ============================================================================
// Scenario 5: Exercise through at least TWO of the three callers found by
// the audit. Test the planning methods directly (unit test level) and
// through the on_record_* wrappers (non-tx path).
// ============================================================================

#[tokio::test]
async fn test_scenario_5_planning_methods_direct() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def.clone()).await.unwrap();

    // Manually set state to Building and mark in-flight.
    // Use add_index to replace the Ready definition with a Building one.
    let mut building_def = index_def.clone();
    building_def.state = IndexState::Building;
    manager.indexes.add_index(building_def);
    manager.mark_build_in_flight(1001);

    // Exercise plan_record_created directly.
    let rid1 = RecordId::new();
    let value = create_test_value(&[(1, InnerValue::Str("test".into()))]);
    let ops = manager.plan_record_created(&rid1, &value).await.unwrap();
    assert_eq!(ops.len(), 0);

    let dirty = manager.dirty_sets.lock().unwrap();
    let dirty_set = dirty.get(&1001).unwrap();
    let set = dirty_set.lock().unwrap();
    assert!(set.contains(&rid1));
}

#[tokio::test]
async fn test_scenario_5_non_tx_path_wrappers() {
    let (_, _, manager) = create_manager();

    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def.clone()).await.unwrap();

    // Manually set state to Building and mark in-flight.
    // Use add_index to replace the Ready definition with a Building one.
    let mut building_def = index_def.clone();
    building_def.state = IndexState::Building;
    manager.indexes.add_index(building_def);
    manager.mark_build_in_flight(1001);

    // Exercise through on_record_created wrapper (non-tx path).
    let rid = RecordId::new();
    let value = create_test_value(&[(1, InnerValue::Str("test".into()))]);
    manager.on_record_created(&rid, &value).await.unwrap();

    // Verify RecordId is in dirty-set (even though on_record_created applies ops
    // directly, the capture still happens in plan_record_created).
    let dirty = manager.dirty_sets.lock().unwrap();
    let dirty_set = dirty.get(&1001).unwrap();
    let set = dirty_set.lock().unwrap();
    assert!(set.contains(&rid));
}

// ============================================================================
// Registry API tests (unit tests for the helper methods themselves).
// ============================================================================

#[tokio::test]
async fn test_registry_mark_and_query() {
    let (_, _, manager) = create_manager();

    // Initially not in-flight.
    assert!(!manager.is_build_in_flight(1001));

    // Mark in-flight.
    manager.mark_build_in_flight(1001);
    assert!(manager.is_build_in_flight(1001));

    // Clear in-flight.
    manager.clear_build_in_flight(1001);
    assert!(!manager.is_build_in_flight(1001));
}

#[tokio::test]
async fn test_registry_clear_removes_dirty_set() {
    let (_, _, manager) = create_manager();

    manager.mark_build_in_flight(1001);

    // Force dirty-set creation by doing a plan.
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def.clone()).await.unwrap();

    // Set to Building so capture logic triggers.
    let mut building_def = index_def.clone();
    building_def.state = IndexState::Building;
    manager.indexes.add_index(building_def);

    let rid = RecordId::new();
    let value = create_test_value(&[(1, InnerValue::Str("test".into()))]);
    manager.plan_record_created(&rid, &value).await.unwrap();

    // Verify dirty-set exists.
    let dirty = manager.dirty_sets.lock().unwrap();
    assert!(dirty.contains_key(&1001));
    drop(dirty);

    // Clear in-flight.
    manager.clear_build_in_flight(1001);

    // Verify dirty-set is removed.
    let dirty = manager.dirty_sets.lock().unwrap();
    assert!(!dirty.contains_key(&1001));
}

#[tokio::test]
async fn test_registry_idempotent_mark() {
    let (_, _, manager) = create_manager();

    // Mark twice should be idempotent.
    manager.mark_build_in_flight(1001);
    manager.mark_build_in_flight(1001);
    assert!(manager.is_build_in_flight(1001));

    // Clear twice should be idempotent.
    manager.clear_build_in_flight(1001);
    manager.clear_build_in_flight(1001);
    assert!(!manager.is_build_in_flight(1001));
}

#[tokio::test]
async fn test_building_not_in_flight_direct_writes() {
    let (_, _, manager) = create_manager();

    // Create a Building index but do NOT mark it as in-flight.
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def.clone()).await.unwrap();

    // Set to Building but do NOT mark in-flight.
    let mut building_def = index_def.clone();
    building_def.state = IndexState::Building;
    manager.indexes.add_index(building_def);

    // Verify is_build_in_flight returns false.
    assert!(!manager.is_build_in_flight(1001));

    // Write a record touching field 1.
    let rid = RecordId::new();
    let value = create_test_value(&[(1, InnerValue::Str("alice".into()))]);
    let ops = manager.plan_record_created(&rid, &value).await.unwrap();

    // Verify SetPosting IS produced (Building but not yet in-flight-registered).
    assert_eq!(ops.len(), 1);
    match &ops[0] {
        IndexWriteOp::SetPosting { .. } => {}
        _ => panic!("Expected SetPosting, got {:?}", ops[0]),
    }

    // Verify dirty-set is NOT created.
    let dirty = manager.dirty_sets.lock().unwrap();
    assert!(!dirty.contains_key(&1001));
}
