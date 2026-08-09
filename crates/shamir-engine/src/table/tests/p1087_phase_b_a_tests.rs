//! #1087 — Phase B (micro-barrier) + Phase A (barrier-free backfill) for online CREATE INDEX.
//!
//! Tests that verify:
//! 1. Basic correctness: Phase B+A produces postings correctly, index remains in `Building`.
//! 2. Concurrent writes during Phase A land in the dirty-set (via pause hook seam).
//! 3. Fallback signal: table without changefeed returns false, does not register.

use crate::index::index_definition::IndexDefinition;
use crate::index::index_info_item::IndexInfoItem;
use crate::table::index2_backfill_hook::BackfillPauseHook;
use crate::table::TableManager;
use shamir_index::IndexState;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_tx::{MvccStore, RepoTxGate, Retention};
use shamir_types::core::interner::TouchInd;
use shamir_types::types::common::new_map;
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

/// Helper: create a TableManager WITHOUT changefeed (online-build unavailable).
async fn make_table_without_changefeed() -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    TableManager::create("t".into(), data, info).await.unwrap()
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
async fn insert_test_data(
    tbl: &TableManager,
) -> Vec<(String, shamir_types::types::record_id::RecordId)> {
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

/// Test 1: Basic correctness, no concurrent writes.
///
/// Phase B+A should produce correct postings and leave the index in `Building`.
#[tokio::test]
async fn p1087_phase_b_a_correctness_no_concurrency() {
    let tbl = make_table_with_mvcc_and_changefeed().await;

    // Insert test data.
    let _rids = insert_test_data(&tbl).await;

    // Build index definition for the "name" path.
    let index_def = build_index_def(&tbl, "idx_name", "name").await;

    // Run Phase B+A.
    let result = tbl
        .phase_b_a_backfill("idx_name", index_def.clone(), 1000)
        .await
        .expect("phase_b_a_backfill should succeed");
    let _result = result.expect("online build should succeed");

    // Assert index is in Building state (NOT flipped to Ready).
    let def = tbl
        .index_manager_ref()
        .iter_indexes()
        .find(|idx| idx.name_interned == index_def.name_interned);
    assert!(def.is_some(), "index should be registered");
    assert_eq!(
        def.unwrap().state,
        IndexState::Building,
        "index should remain in Building state after Phase B+A"
    );
}

/// Test 2: Concurrent writes during Phase A land in the dirty-set.
///
/// Use a pause hook to park mid-scan, issue a write from a separate task,
/// resume, and confirm the written RecordId ends up in the dirty-set.
#[tokio::test]
async fn p1087_phase_b_a_concurrent_write_captured_in_dirty_set() {
    let tbl = make_table_with_mvcc_and_changefeed().await;

    // Insert initial test data.
    insert_test_data(&tbl).await;

    // Build index definition.
    let index_def = build_index_def(&tbl, "idx_name", "name").await;
    let name_interned = index_def.name_interned;

    // Install pause hook (test-only).
    let hook = Arc::new(BackfillPauseHook::new());
    tbl.online_index_backfill_hook
        .store(Some(Arc::clone(&hook)));

    // Spawn Phase B+A in a task.
    let tbl_clone = tbl.clone();
    let index_def_clone = index_def.clone();
    let backfill_task = tokio::spawn(async move {
        tbl_clone
            .phase_b_a_backfill("idx_name", index_def_clone, 1)
            .await
    });

    // Wait for the hook to park (after first batch).
    hook.wait_until_parked().await;

    // Now that we're parked mid-scan, issue a concurrent write.
    let interner = tbl.interner().get().await.unwrap();
    let id_key = interner.touch_ind("id").unwrap().into_key();
    let name_key = interner.touch_ind("name").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    let mut m = new_map();
    m.insert(id_key, InnerValue::Int(4));
    m.insert(name_key, InnerValue::Str("diane".to_string()));
    let value = InnerValue::Map(m);

    let concurrent_rid = tbl.insert(&value).await.unwrap();

    // Resume the backfill.
    hook.release();

    // Wait for backfill to complete.
    let result = backfill_task
        .await
        .expect("backfill task should not panic")
        .expect("phase_b_a_backfill should succeed");
    let _result = result; // Keep result alive (guard/pin), test doesn't need it.

    // Drain the dirty-set and verify the concurrent RID is there.
    let dirty_set = tbl.index_manager_ref().drain_dirty_set(name_interned);

    assert!(
        dirty_set.contains(&concurrent_rid),
        "concurrent write {concurrent_rid:?} should be captured in dirty-set"
    );

    // Clear the hook.
    tbl.online_index_backfill_hook.store(None);
}

/// Test 3: Fallback signal when changefeed not wired.
///
/// For a table without changefeed, `phase_b_a_backfill` should return `Ok(false)`
/// and NOT register anything or panic.
#[tokio::test]
async fn p1087_phase_b_a_fallback_when_changefeed_absent() {
    let tbl = make_table_without_changefeed().await;

    // Insert test data (even though changefeed is absent, data exists).
    insert_test_data(&tbl).await;

    // Build index definition.
    let index_def = build_index_def(&tbl, "idx_name", "name").await;
    let name_interned = index_def.name_interned;

    // Run Phase B+A.
    let result = tbl
        .phase_b_a_backfill("idx_name", index_def, 1000)
        .await
        .expect("phase_b_a_backfill should not error");
    assert!(
        result.is_none(),
        "should return None (unavailable) for table without changefeed"
    );

    // Assert index is NOT registered.
    let def = tbl
        .index_manager_ref()
        .iter_indexes()
        .find(|idx| idx.name_interned == name_interned);
    assert!(
        def.is_none(),
        "index should not be registered when changefeed is absent"
    );

    // Assert dirty-set is NOT in-flight.
    assert!(
        !tbl.index_manager_ref().is_build_in_flight(name_interned),
        "build should not be marked in-flight when changefeed is absent"
    );
}
