//! P-1060: crash-recovery invariant matrix for online CREATE INDEX.
//!
//! Prove the ONE invariant that actually matters: a crash never leaves the
//! index durably `Ready` with incomplete or incorrect postings. Either the
//! index stays durably `Building` (safe — planner-invisible), or the crash
//! landed AFTER the final Phase D flip, in which case the index is durably
//! `Ready` AND fully, correctly built.
//!
//! This does NOT test "recovery completes the build" — nothing completes it
//! automatically for the base_index family; recovery requires a manual
//! `TableManager::repair()` call.

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

/// Matrix row 1: Before Phase B (no index creation at all).
///
/// Insert some data, do NOT call create_index at all. Reopen. Assert:
/// no index registered. Degenerate but cheap — confirms the baseline.
#[tokio::test]
async fn p1060_crash_before_phase_b() {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    mvcc.set_retention(Retention::keep_history()).unwrap();

    let tbl = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();
    let tbl = tbl.with_mvcc_store(mvcc).with_changefeed(gate);

    // Insert test data, but DO NOT create any index.
    let interner = tbl.interner().get().await.unwrap();
    let id_key = interner.touch_ind("id").unwrap().into_key();
    let name_key = interner.touch_ind("name").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    let mut m = new_map();
    m.insert(id_key.clone(), InnerValue::Int(1));
    m.insert(name_key.clone(), InnerValue::Str("alice".to_string()));
    let _ = tbl.insert(&InnerValue::Map(m)).await.unwrap();

    // "Crash" by dropping the manager.
    drop(tbl);

    // Reopen — simulate server restart.
    let tbl = TableManager::create("t".into(), data, info).await.unwrap();

    // Assert: no index registered at all.
    let indexes = tbl.index_manager_ref().iter_indexes().collect::<Vec<_>>();
    assert!(
        indexes.is_empty(),
        "no index should be registered before Phase B, found {}",
        indexes.len()
    );
}

/// Matrix row 2: Inside Phase B (after Building persist, before barrier drop).
///
/// Race phase_b_a_backfill against phase_b_pause_hook. After crash+reopen:
/// the index IS registered and its state == Building (planner-invisible).
#[tokio::test]
async fn p1060_crash_inside_phase_b() {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    mvcc.set_retention(Retention::keep_history()).unwrap();

    let tbl = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();
    let tbl = tbl.with_mvcc_store(mvcc).with_changefeed(gate);

    // Insert test data.
    let interner = tbl.interner().get().await.unwrap();
    let id_key = interner.touch_ind("id").unwrap().into_key();
    let name_key = interner.touch_ind("name").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    let mut m = new_map();
    m.insert(id_key.clone(), InnerValue::Int(1));
    m.insert(name_key.clone(), InnerValue::Str("alice".to_string()));
    let _ = tbl.insert(&InnerValue::Map(m)).await.unwrap();

    // Build index definition.
    let index_def = build_index_def(&tbl, "idx_name", "name").await;
    let name_interned = index_def.name_interned;

    // Install pause hook (test-only).
    let pause_hook = Arc::new(BackfillPauseHook::new());
    tbl.set_phase_b_pause_hook(Some(Arc::clone(&pause_hook)));

    // Race phase_b_a_backfill against the pause hook.
    let tbl_c = tbl.clone();
    let index_def_c = index_def.clone();
    tokio::select! {
        _ = tbl_c.phase_b_a_backfill("idx_name", index_def_c, 1000) => {
            panic!("phase_b_a_backfill completed before the pause hook fired");
        }
        _ = pause_hook.wait_until_parked() => {
            // Parked: Building durably persisted, in-flight marked, barrier still held.
        }
    }

    // Drop both manager instances to simulate crash.
    drop(tbl_c);
    drop(tbl);

    // Reopen — simulate server restart.
    let tbl = TableManager::create("t".into(), data, info).await.unwrap();

    // Assert: the index IS registered and in Building state.
    let def = tbl
        .index_manager_ref()
        .iter_indexes()
        .find(|d| d.name_interned == name_interned);
    let def = def.expect("index should be registered after crash inside Phase B");
    assert_eq!(
        def.state,
        IndexState::Building,
        "index should be Building after crash inside Phase B"
    );

    // Assert: planner-invisible (absent from iter_indexes_ready).
    let ready_indexes = tbl
        .index_manager_ref()
        .iter_indexes_ready()
        .collect::<Vec<_>>();
    assert!(
        ready_indexes.is_empty(),
        "index should be planner-invisible (not in iter_indexes_ready), found {}",
        ready_indexes.len()
    );
}

/// Matrix row 3: Inside Phase A (mid-backfill scan).
///
/// Reuse the existing online_index_backfill_hook. Insert enough rows to force
/// ≥2 stream batches. Race against the hook. After crash+reopen: Building,
/// planner-invisible. (Same assertions as test 2.)
#[tokio::test]
async fn p1060_crash_inside_phase_a() {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    mvcc.set_retention(Retention::keep_history()).unwrap();

    let tbl = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();
    let tbl = tbl.with_mvcc_store(mvcc).with_changefeed(gate);

    // Insert enough rows to force ≥2 stream batches (batch_size = 1).
    let interner = tbl.interner().get().await.unwrap();
    let id_key = interner.touch_ind("id").unwrap().into_key();
    let name_key = interner.touch_ind("name").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    for i in 1..=3 {
        let mut m = new_map();
        m.insert(id_key.clone(), InnerValue::Int(i));
        m.insert(
            name_key.clone(),
            InnerValue::Str(format!("user_{}", i).to_string()),
        );
        let _ = tbl.insert(&InnerValue::Map(m)).await.unwrap();
    }

    // Build index definition.
    let index_def = build_index_def(&tbl, "idx_name", "name").await;
    let name_interned = index_def.name_interned;

    // Install pause hook (test-only).
    let pause_hook = Arc::new(BackfillPauseHook::new());
    tbl.online_index_backfill_hook
        .store(Some(Arc::clone(&pause_hook)));

    // Race phase_b_a_backfill against the pause hook (batch_size = 1 to ensure ≥2 batches).
    let tbl_c = tbl.clone();
    let index_def_c = index_def.clone();
    tokio::select! {
        _ = tbl_c.phase_b_a_backfill("idx_name", index_def_c, 1) => {
            panic!("phase_b_a_backfill completed before the pause hook fired");
        }
        _ = pause_hook.wait_until_parked() => {
            // Parked: mid-scan, some postings written, Building on disk.
        }
    }

    // Drop both manager instances to simulate crash.
    drop(tbl_c);
    drop(tbl);

    // Reopen — simulate server restart.
    let tbl = TableManager::create("t".into(), data, info).await.unwrap();

    // Assert: the index IS registered and in Building state.
    let def = tbl
        .index_manager_ref()
        .iter_indexes()
        .find(|d| d.name_interned == name_interned);
    let def = def.expect("index should be registered after crash inside Phase A");
    assert_eq!(
        def.state,
        IndexState::Building,
        "index should be Building after crash inside Phase A"
    );

    // Assert: planner-invisible (absent from iter_indexes_ready).
    let ready_indexes = tbl
        .index_manager_ref()
        .iter_indexes_ready()
        .collect::<Vec<_>>();
    assert!(
        ready_indexes.is_empty(),
        "index should be planner-invisible (not in iter_indexes_ready), found {}",
        ready_indexes.len()
    );
}

/// Matrix row 4: Inside Phase C (at top of catch-up loop).
///
/// Run phase_b_a_backfill to completion normally, then race
/// phase_c_d_catchup_and_publish against phase_c_pause_hook. After crash+reopen:
/// Building, planner-invisible.
#[tokio::test]
async fn p1060_crash_inside_phase_c() {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    mvcc.set_retention(Retention::keep_history()).unwrap();

    let tbl = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();
    let tbl = tbl.with_mvcc_store(mvcc).with_changefeed(gate);

    // Insert test data.
    let interner = tbl.interner().get().await.unwrap();
    let id_key = interner.touch_ind("id").unwrap().into_key();
    let name_key = interner.touch_ind("name").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    let mut m = new_map();
    m.insert(id_key.clone(), InnerValue::Int(1));
    m.insert(name_key.clone(), InnerValue::Str("alice".to_string()));
    let _ = tbl.insert(&InnerValue::Map(m)).await.unwrap();

    // Build index definition.
    let index_def = build_index_def(&tbl, "idx_name", "name").await;
    let name_interned = index_def.name_interned;

    // Run Phase B+A to completion (uninterrupted).
    let phase_ba = tbl
        .phase_b_a_backfill("idx_name", index_def, 1000)
        .await
        .expect("phase_b_a_backfill should succeed")
        .expect("online build should succeed");

    // Install Phase C pause hook (test-only).
    let pause_hook = Arc::new(BackfillPauseHook::new());
    tbl.set_phase_c_pause_hook(Some(Arc::clone(&pause_hook)));

    // Race phase_c_d_catchup_and_publish against the pause hook.
    let tbl_c = tbl.clone();
    tokio::select! {
        _ = tbl_c.phase_c_d_catchup_and_publish(name_interned, phase_ba) => {
            panic!("phase_c_d_catchup_and_publish completed before the pause hook fired");
        }
        _ = pause_hook.wait_until_parked() => {
            // Parked: at top of catch-up loop, Building still on disk.
        }
    }

    // Drop both manager instances to simulate crash.
    drop(tbl_c);
    drop(tbl);

    // Reopen — simulate server restart.
    let tbl = TableManager::create("t".into(), data, info).await.unwrap();

    // Assert: the index IS registered and in Building state.
    let def = tbl
        .index_manager_ref()
        .iter_indexes()
        .find(|d| d.name_interned == name_interned);
    let def = def.expect("index should be registered after crash inside Phase C");
    assert_eq!(
        def.state,
        IndexState::Building,
        "index should be Building after crash inside Phase C"
    );

    // Assert: planner-invisible (absent from iter_indexes_ready).
    let ready_indexes = tbl
        .index_manager_ref()
        .iter_indexes_ready()
        .collect::<Vec<_>>();
    assert!(
        ready_indexes.is_empty(),
        "index should be planner-invisible (not in iter_indexes_ready), found {}",
        ready_indexes.len()
    );
}

/// Matrix row 5: Inside Phase D (after barrier, before final drain).
///
/// Run phase_b_a_backfill to completion normally, then race
/// phase_c_d_catchup_and_publish against phase_d_pause_hook. After crash+reopen:
/// Building, planner-invisible.
///
/// Note: this exposure window is now milliseconds (bounded residual under the
/// barrier) vs. the old design's minutes (whole-table scan under the barrier),
/// even though the RECOVERY ACTION (stays Building, manual repair()) is unchanged.
#[tokio::test]
async fn p1060_crash_inside_phase_d() {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    mvcc.set_retention(Retention::keep_history()).unwrap();

    let tbl = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();
    let tbl = tbl.with_mvcc_store(mvcc).with_changefeed(gate);

    // Insert test data.
    let interner = tbl.interner().get().await.unwrap();
    let id_key = interner.touch_ind("id").unwrap().into_key();
    let name_key = interner.touch_ind("name").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    let mut m = new_map();
    m.insert(id_key.clone(), InnerValue::Int(1));
    m.insert(name_key.clone(), InnerValue::Str("alice".to_string()));
    let _ = tbl.insert(&InnerValue::Map(m)).await.unwrap();

    // Build index definition.
    let index_def = build_index_def(&tbl, "idx_name", "name").await;
    let name_interned = index_def.name_interned;

    // Run Phase B+A to completion (uninterrupted).
    let phase_ba = tbl
        .phase_b_a_backfill("idx_name", index_def, 1000)
        .await
        .expect("phase_b_a_backfill should succeed")
        .expect("online build should succeed");

    // Install Phase D pause hook (test-only).
    let pause_hook = Arc::new(BackfillPauseHook::new());
    tbl.set_phase_d_pause_hook(Some(Arc::clone(&pause_hook)));

    // Race phase_c_d_catchup_and_publish against the pause hook.
    let tbl_c = tbl.clone();
    tokio::select! {
        _ = tbl_c.phase_c_d_catchup_and_publish(name_interned, phase_ba) => {
            panic!("phase_c_d_catchup_and_publish completed before the pause hook fired");
        }
        _ = pause_hook.wait_until_parked() => {
            // Parked: after barrier acquisition, before final drain, Building still on disk.
        }
    }

    // Drop both manager instances to simulate crash.
    drop(tbl_c);
    drop(tbl);

    // Reopen — simulate server restart.
    let tbl = TableManager::create("t".into(), data, info).await.unwrap();

    // Assert: the index IS registered and in Building state.
    let def = tbl
        .index_manager_ref()
        .iter_indexes()
        .find(|d| d.name_interned == name_interned);
    let def = def.expect("index should be registered after crash inside Phase D");
    assert_eq!(
        def.state,
        IndexState::Building,
        "index should be Building after crash inside Phase D"
    );

    // Assert: planner-invisible (absent from iter_indexes_ready).
    let ready_indexes = tbl
        .index_manager_ref()
        .iter_indexes_ready()
        .collect::<Vec<_>>();
    assert!(
        ready_indexes.is_empty(),
        "index should be planner-invisible (not in iter_indexes_ready), found {}",
        ready_indexes.len()
    );
}

/// Matrix row 6: After Ready, before in-flight cleanup.
///
/// Run phase_b_a_backfill + phase_c_d_catchup_and_publish to full, uninterrupted
/// completion (no select!, no pause hook). Reopen and assert the index is Ready,
/// iter_indexes_ready finds it, and its postings are correct.
///
/// This is really "prove reopening after a successful build changes nothing" —
/// a corollary, not a crash-simulation, but it completes the matrix's 6th row.
#[tokio::test]
async fn p1060_after_ready_before_cleanup() {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let history: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    mvcc.set_retention(Retention::keep_history()).unwrap();

    let tbl = TableManager::create("t".into(), Arc::clone(&data), Arc::clone(&info))
        .await
        .unwrap();
    let tbl = tbl.with_mvcc_store(mvcc).with_changefeed(gate);

    // Insert test data with DISTINCT status values for unambiguous lookups.
    let interner = tbl.interner().get().await.unwrap();
    let id_key = interner.touch_ind("id").unwrap().into_key();
    let name_key = interner.touch_ind("name").unwrap().into_key();
    tbl.interner().persist().await.unwrap();

    let mut m = new_map();
    m.insert(id_key.clone(), InnerValue::Int(1));
    m.insert(name_key.clone(), InnerValue::Str("alice".to_string()));
    let alice_rid = tbl.insert(&InnerValue::Map(m)).await.unwrap();

    let mut m = new_map();
    m.insert(id_key.clone(), InnerValue::Int(2));
    m.insert(name_key.clone(), InnerValue::Str("bob".to_string()));
    let bob_rid = tbl.insert(&InnerValue::Map(m)).await.unwrap();

    let mut m = new_map();
    m.insert(id_key.clone(), InnerValue::Int(3));
    m.insert(name_key.clone(), InnerValue::Str("charlie".to_string()));
    let charlie_rid = tbl.insert(&InnerValue::Map(m)).await.unwrap();

    // Build index definition.
    let index_def = build_index_def(&tbl, "idx_name", "name").await;
    let name_interned = index_def.name_interned;

    // Run Phase B+A to completion (uninterrupted).
    let phase_ba = tbl
        .phase_b_a_backfill("idx_name", index_def, 1000)
        .await
        .expect("phase_b_a_backfill should succeed")
        .expect("online build should succeed");

    // Run Phase C+D to completion (uninterrupted).
    tbl.phase_c_d_catchup_and_publish(name_interned, phase_ba)
        .await
        .expect("phase_c_d_catchup_and_publish should succeed");

    // "Crash" by dropping the manager.
    drop(tbl);

    // Reopen — simulate server restart.
    let tbl = TableManager::create("t".into(), data, info).await.unwrap();

    // Assert: the index IS registered and in Ready state.
    let def = tbl
        .index_manager_ref()
        .iter_indexes()
        .find(|d| d.name_interned == name_interned);
    let def = def.expect("index should be registered after successful build");
    assert_eq!(
        def.state,
        IndexState::Ready,
        "index should be Ready after successful build"
    );

    // Assert: planner-visible (present in iter_indexes_ready).
    let ready_indexes = tbl
        .index_manager_ref()
        .iter_indexes_ready()
        .collect::<Vec<_>>();
    assert_eq!(
        ready_indexes.len(),
        1,
        "index should be planner-visible (in iter_indexes_ready)"
    );
    assert_eq!(ready_indexes[0].name_interned, name_interned);

    // Assert: postings are correct via lookups.
    let alice_value = InnerValue::Str("alice".to_string());
    let results = tbl
        .index_manager_ref()
        .lookup_by_index(name_interned, &[alice_value])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(results.len(), 1, "should find alice");
    assert_eq!(results[0], alice_rid, "should return alice's record ID");

    let bob_value = InnerValue::Str("bob".to_string());
    let results = tbl
        .index_manager_ref()
        .lookup_by_index(name_interned, &[bob_value])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(results.len(), 1, "should find bob");
    assert_eq!(results[0], bob_rid, "should return bob's record ID");

    let charlie_value = InnerValue::Str("charlie".to_string());
    let results = tbl
        .index_manager_ref()
        .lookup_by_index(name_interned, &[charlie_value])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(results.len(), 1, "should find charlie");
    assert_eq!(results[0], charlie_rid, "should return charlie's record ID");
}
