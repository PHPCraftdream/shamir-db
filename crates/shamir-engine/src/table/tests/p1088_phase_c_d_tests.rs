//! #1088 — Phase C (catch-up loop) + Phase D (publish barrier) for online CREATE INDEX.
//!
//! Tests that verify:
//! 1. Basic correctness: Phase C+D catch-up and flip to Ready.
//! 2. Insert during window captured and applied.
//! 3. Update-to-different-value during window handled correctly (the v2 bug case).
//! 4. Delete during window cleaned up correctly.
//! 5. Hard iteration cap forces final residual apply.
//! 6. Post-Phase-D normal writes work without catch-up.

use crate::index::index_definition::IndexDefinition;
use crate::index::index_info_item::IndexInfoItem;
use crate::table::TableManager;
use shamir_index::IndexState;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{MvccStore, RepoTxGate, Retention};
use shamir_types::core::interner::TouchInd;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::sync::Arc;

/// Helper: get the interned u64 key for a string.
async fn key_id(tbl: &TableManager, name: &str) -> u64 {
    let interner = tbl.interner().get().await.unwrap();
    match interner.touch_ind(name).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

/// Helper: create a TableManager with MVCC and changefeed attached (online-build capable).
async fn make_table_with_mvcc_and_changefeed() -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    mvcc.set_retention(Retention::keep_history()).unwrap();

    let tbl = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();
    tbl.with_mvcc_store(mvcc).with_changefeed(gate)
}

/// Helper: build an IndexDefinition for a simple path.
async fn build_index_def(tbl: &TableManager, name: &str, path: &str) -> IndexDefinition {
    let idx_name = key_id(tbl, name).await;
    let path_key = key_id(tbl, path).await;

    IndexDefinition {
        name_interned: idx_name,
        paths: vec![IndexInfoItem::new(vec![path_key])],
        state: IndexState::Building,
        instance_epoch: 0,
    }
}

/// Insert test records: {id: 1, name: "alice"}, {id: 2, name: "bob"}, {id: 3, name: "charlie"}.
async fn insert_test_data(tbl: &TableManager) -> Vec<(String, RecordId)> {
    let mut rids = Vec::new();
    for (id, name) in [(1, "alice"), (2, "bob"), (3, "charlie")] {
        let interner = tbl.interner().get().await.unwrap();
        let id_key = interner.touch_ind("id").unwrap().into_key();
        let name_key = interner.touch_ind("name").unwrap().into_key();
        tbl.interner().persist().await.unwrap();

        let mut m = new_map();
        m.insert(id_key, InnerValue::Int(id));
        m.insert(name_key, InnerValue::Str(name.to_string()));
        let value = InnerValue::Map(m);

        let rid = tbl.insert(&value).await.unwrap();
        rids.push((name.to_string(), rid));
    }
    rids
}

/// Test 1: No concurrent writes.
///
/// Phase C converges on its first empty drain, Phase D flips to `Ready`.
#[tokio::test]
async fn p1088_phase_c_d_no_concurrent_writes() {
    let tbl = make_table_with_mvcc_and_changefeed().await;

    // Insert test data.
    let _rids = insert_test_data(&tbl).await;

    // Build index definition for the "name" path.
    let index_def = build_index_def(&tbl, "idx_name", "name").await;
    let name_interned = index_def.name_interned;

    // Run Phase B+A.
    let phase_ba = tbl
        .phase_b_a_backfill(index_def.clone(), 1000)
        .await
        .expect("phase_b_a_backfill should succeed")
        .expect("online build should succeed");

    // Run Phase C+D.
    tbl.phase_c_d_catchup_and_publish(name_interned, phase_ba)
        .await
        .expect("phase_c_d_catchup_and_publish should succeed");

    // Assert index is in Ready state.
    let def = tbl
        .index_manager_ref()
        .iter_indexes()
        .find(|idx| idx.name_interned == name_interned);
    assert!(def.is_some(), "index should be registered");
    assert_eq!(
        def.unwrap().state,
        IndexState::Ready,
        "index should be Ready after Phase C+D"
    );

    // Assert build is no longer in-flight.
    assert!(
        !tbl.index_manager_ref().is_build_in_flight(name_interned),
        "build should NOT be in-flight after Phase D"
    );

    // Assert dirty-set is empty.
    let dirty = tbl.index_manager_ref().drain_dirty_set(name_interned);
    assert!(
        dirty.is_empty(),
        "dirty-set should be empty after Phase C+D"
    );
}

/// Test 2: Insert during the window.
///
/// A row created AFTER the pin lands in the dirty-set via the live write hook.
#[tokio::test]
async fn p1088_phase_c_d_insert_during_window() {
    let tbl = make_table_with_mvcc_and_changefeed().await;

    // Insert initial test data.
    let _rids = insert_test_data(&tbl).await;

    // Build index definition.
    let index_def = build_index_def(&tbl, "idx_name", "name").await;
    let name_interned = index_def.name_interned;

    // Run Phase B+A.
    let phase_ba = tbl
        .phase_b_a_backfill(index_def.clone(), 1000)
        .await
        .expect("phase_b_a_backfill should succeed")
        .expect("online build should succeed");

    // Now insert a new row (after Phase A but before Phase C+D).
    let interner = tbl.interner().get().await.unwrap();
    let id_key = interner.touch_ind("id").unwrap().into_key();
    let name_key = interner.touch_ind("name").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    let mut m = new_map();
    m.insert(id_key, InnerValue::Int(4));
    m.insert(name_key, InnerValue::Str("diane".to_string()));
    let value = InnerValue::Map(m);

    let new_rid = tbl.insert(&value).await.unwrap();

    // Run Phase C+D.
    tbl.phase_c_d_catchup_and_publish(name_interned, phase_ba)
        .await
        .expect("phase_c_d_catchup_and_publish should succeed");

    // Assert index is in Ready state.
    let def = tbl
        .index_manager_ref()
        .iter_indexes()
        .find(|idx| idx.name_interned == name_interned);
    assert_eq!(
        def.unwrap().state,
        IndexState::Ready,
        "index should be Ready"
    );

    // Query for the new row by name using the interned ID.
    let _interner = tbl.interner().get().await.unwrap();
    let diane_value = InnerValue::Str("diane".to_string());
    let results = tbl
        .index_manager_ref()
        .lookup_by_index(name_interned, &[diane_value])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(results.len(), 1, "should find the newly inserted row");
    assert_eq!(results[0], new_rid, "should return the correct record ID");
}

/// Test 3: Update-to-different-value during the window — THE case v2 got wrong.
///
/// A row that EXISTED at the pin is updated to a DIFFERENT indexed value during Phase A/B's window.
/// Assert AFTER Phase D:
/// (a) the OLD indexed value no longer matches this record
/// (b) the NEW indexed value DOES match this record.
#[tokio::test]
async fn p1088_phase_c_d_update_to_different_value_during_window() {
    let tbl = make_table_with_mvcc_and_changefeed().await;

    // Insert initial test data.
    let rids = insert_test_data(&tbl).await;
    let alice_rid = rids
        .iter()
        .find(|(name, _)| name == "alice")
        .map(|(_, rid)| *rid)
        .unwrap();

    // Build index definition.
    let index_def = build_index_def(&tbl, "idx_name", "name").await;
    let name_interned = index_def.name_interned;

    // Run Phase B+A.
    let phase_ba = tbl
        .phase_b_a_backfill(index_def.clone(), 1000)
        .await
        .expect("phase_b_a_backfill should succeed")
        .expect("online build should succeed");

    // Update alice to "aaron" (different indexed value).
    let interner = tbl.interner().get().await.unwrap();
    let name_key = interner.touch_ind("name").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    let mut m = new_map();
    m.insert(name_key, InnerValue::Str("aaron".to_string()));
    tbl.set(alice_rid, &InnerValue::Map(m))
        .await
        .expect("set should succeed");

    // Run Phase C+D.
    tbl.phase_c_d_catchup_and_publish(name_interned, phase_ba)
        .await
        .expect("phase_c_d_catchup_and_publish should succeed");

    // Assert index is in Ready state.
    let def = tbl
        .index_manager_ref()
        .iter_indexes()
        .find(|idx| idx.name_interned == name_interned);
    assert_eq!(
        def.unwrap().state,
        IndexState::Ready,
        "index should be Ready"
    );

    // CRITICAL: Query for OLD value "alice" - should NOT return alice_rid.
    let old_value = InnerValue::Str("alice".to_string());
    let old_results_opt = tbl
        .index_manager_ref()
        .lookup_by_index(name_interned, &[old_value])
        .await
        .unwrap();
    let old_results = old_results_opt.unwrap_or_else(|| Arc::new([]));
    assert!(
        !old_results.iter().any(|r| r == &alice_rid),
        "alice_rid should NOT be found under OLD value 'alice' - this is the v2 bug we're fixing"
    );

    // Query for NEW value "aaron" - SHOULD return alice_rid.
    let new_value = InnerValue::Str("aaron".to_string());
    let new_results = tbl
        .index_manager_ref()
        .lookup_by_index(name_interned, &[new_value])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        new_results.len(),
        1,
        "should find alice_rid under new value 'aaron'"
    );
    assert_eq!(
        new_results[0], alice_rid,
        "should return the correct record ID for 'aaron'"
    );
}

/// Test 4: Delete during the window.
///
/// A row that existed at the pin is deleted during the window.
/// Assert AFTER Phase D: querying by its old indexed value returns nothing (no orphaned posting).
#[tokio::test]
async fn p1088_phase_c_d_delete_during_window() {
    let tbl = make_table_with_mvcc_and_changefeed().await;

    // Insert initial test data.
    let rids = insert_test_data(&tbl).await;
    let bob_rid = rids
        .iter()
        .find(|(name, _)| name == "bob")
        .map(|(_, rid)| *rid)
        .unwrap();

    // Build index definition.
    let index_def = build_index_def(&tbl, "idx_name", "name").await;
    let name_interned = index_def.name_interned;

    // Run Phase B+A.
    let phase_ba = tbl
        .phase_b_a_backfill(index_def.clone(), 1000)
        .await
        .expect("phase_b_a_backfill should succeed")
        .expect("online build should succeed");

    // Delete bob.
    tbl.delete_returning_version(bob_rid)
        .await
        .expect("delete should succeed");

    // Run Phase C+D.
    tbl.phase_c_d_catchup_and_publish(name_interned, phase_ba)
        .await
        .expect("phase_c_d_catchup_and_publish should succeed");

    // Assert index is in Ready state.
    let def = tbl
        .index_manager_ref()
        .iter_indexes()
        .find(|idx| idx.name_interned == name_interned);
    assert_eq!(
        def.unwrap().state,
        IndexState::Ready,
        "index should be Ready"
    );

    // Query for bob by name - should return nothing (no orphaned posting).
    let bob_value = InnerValue::Str("bob".to_string());
    let results_opt = tbl
        .index_manager_ref()
        .lookup_by_index(name_interned, &[bob_value])
        .await
        .unwrap();
    let results = results_opt.unwrap_or_else(|| Arc::new([]));
    assert_eq!(
        results.len(),
        0,
        "bob should NOT be found via index - orphaned posting should be cleaned"
    );
}

/// Test 5: Hard iteration cap.
///
/// Simulate sustained dirty-set growth forces the loop to exit via the cap.
/// Assert Phase D still correctly applies the final residual and flips to Ready.
#[tokio::test]
async fn p1088_phase_c_d_hard_iteration_cap() {
    let tbl = make_table_with_mvcc_and_changefeed().await;

    // Insert initial test data.
    let _rids = insert_test_data(&tbl).await;

    // Build index definition.
    let index_def = build_index_def(&tbl, "idx_name", "name").await;
    let name_interned = index_def.name_interned;

    // Run Phase B+A.
    let phase_ba = tbl
        .phase_b_a_backfill(index_def.clone(), 1000)
        .await
        .expect("phase_b_a_backfill should succeed")
        .expect("online build should succeed");

    // Insert many records BEFORE calling phase_c_d, ensuring dirty-set is large.
    let interner = tbl.interner().get().await.unwrap();
    let id_key = interner.touch_ind("id").unwrap().into_key();
    let name_key = interner.touch_ind("name").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    for i in 100..200 {
        let mut m = new_map();
        m.insert(id_key.clone(), InnerValue::Int(i));
        m.insert(name_key.clone(), InnerValue::Str(format!("user_{}", i)));
        let value = InnerValue::Map(m);
        tbl.insert(&value).await.unwrap();
    }

    // Run Phase C+D.
    tbl.phase_c_d_catchup_and_publish(name_interned, phase_ba)
        .await
        .expect("phase_c_d_catchup_and_publish should succeed");

    // Assert index is in Ready state.
    let def = tbl
        .index_manager_ref()
        .iter_indexes()
        .find(|idx| idx.name_interned == name_interned);
    assert_eq!(
        def.unwrap().state,
        IndexState::Ready,
        "index should be Ready"
    );

    // Verify dirty-set is empty (all dirty records were caught up).
    let dirty = tbl.index_manager_ref().drain_dirty_set(name_interned);
    assert!(
        dirty.is_empty(),
        "dirty-set should be empty after Phase D - all records should have been caught up"
    );

    // Verify at least some of the newly inserted records are indexed.
    let user_150_value = InnerValue::Str("user_150".to_string());
    let results = tbl
        .index_manager_ref()
        .lookup_by_index(name_interned, &[user_150_value])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        results.len(),
        1,
        "should find one of the newly inserted records"
    );
}

/// Test 6: Post-Phase-D normal writes.
///
/// After the index is `Ready`, an ordinary write goes straight to `SetPosting` again
/// (registry cleared, `is_build_in_flight` now `false`).
#[tokio::test]
async fn p1088_phase_c_d_post_phase_d_normal_writes() {
    let tbl = make_table_with_mvcc_and_changefeed().await;

    // Insert initial test data.
    let _rids = insert_test_data(&tbl).await;

    // Build index definition.
    let index_def = build_index_def(&tbl, "idx_name", "name").await;
    let name_interned = index_def.name_interned;

    // Run Phase B+A.
    let phase_ba = tbl
        .phase_b_a_backfill(index_def.clone(), 1000)
        .await
        .expect("phase_b_a_backfill should succeed")
        .expect("online build should succeed");

    // Run Phase C+D.
    tbl.phase_c_d_catchup_and_publish(name_interned, phase_ba)
        .await
        .expect("phase_c_d_catchup_and_publish should succeed");

    // Verify build is no longer in-flight.
    assert!(
        !tbl.index_manager_ref().is_build_in_flight(name_interned),
        "build should NOT be in-flight after Phase D"
    );

    // Insert a new row after Phase D.
    let interner = tbl.interner().get().await.unwrap();
    let id_key = interner.touch_ind("id").unwrap().into_key();
    let name_key = interner.touch_ind("name").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    let mut m = new_map();
    m.insert(id_key, InnerValue::Int(999));
    m.insert(name_key, InnerValue::Str("final_user".to_string()));
    let value = InnerValue::Map(m);

    let new_rid = tbl.insert(&value).await.unwrap();

    // Query immediately - should find the new row without any catch-up.
    let final_value = InnerValue::Str("final_user".to_string());
    let results = tbl
        .index_manager_ref()
        .lookup_by_index(name_interned, &[final_value])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(results.len(), 1, "should find the newly inserted row");
    assert_eq!(results[0], new_rid, "should return the correct record ID");

    // Verify dirty-set is empty (normal path wrote directly, no capture).
    let dirty = tbl.index_manager_ref().drain_dirty_set(name_interned);
    assert!(
        dirty.is_empty(),
        "dirty-set should be empty after normal post-Phase-D write"
    );
}
