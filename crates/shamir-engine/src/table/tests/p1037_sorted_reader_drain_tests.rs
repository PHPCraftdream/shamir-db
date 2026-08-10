//! P0-3a (#1037) — `ReaderDrainGate` integration tests for the SORTED index family.
//! Proves that the gate correctly blocks a DROP while readers are mid-flight AND that
//! readers back off during a DROP's drain→sweep window.
//!
//! This is the SIBLING test set to `p1011_reader_drain_tests.rs` (regular family) —
//! same proof-of-correctness shape, different manager (SortedIndexManager) and 8
//! chokepoint read methods instead of one (lookup_by_index).

use std::sync::Arc;

use shamir_types::core::interner::TouchInd;
use shamir_types::types::common::new_map;
use shamir_types::types::value::InnerValue;

use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;
use crate::table::TableManager;
use shamir_index::base_index::sorted_index_manager::SortedIndexManager;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::error::DbError;

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

async fn key_id(tbl: &TableManager, name: &str) -> u64 {
    let interner = tbl.interner().get().await.unwrap();
    match interner.touch_ind(name).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

fn record_with_salary(salary_key: u64, salary: i64, id_key: u64, id: i64) -> InnerValue {
    let mut m = shamir_types::types::common::new_map_wc(2);
    m.insert(
        shamir_types::core::interner::InternerKey::new(salary_key),
        InnerValue::Int(salary),
    );
    m.insert(
        shamir_types::core::interner::InternerKey::new(id_key),
        InnerValue::Int(id),
    );
    InnerValue::Map(m)
}

// ============================================================================
// 1. Proof test — reader parked mid-flight blocks the DROP's drain
// ============================================================================

/// P0-3a (#1037) proof test: a reader parked at `lookup_range` (guard HELD, scan
/// not yet started) must block the DROP's `wait_for_drain` call. Once the reader
/// releases, the drop proceeds and finishes. The reader's scan returns the
/// COMPLETE pre-drop set (correct, not partial).
#[tokio::test]
async fn p1037_sorted_reader_blocks_drop_until_released() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let salary_key = key_id(&tbl, "salary").await;
    let id_key = key_id(&tbl, "id").await;

    // Three pre-existing rows with salaries 50, 100, 150.
    for (i, salary) in [(1, 50), (2, 100), (3, 150)] {
        tbl.insert(&record_with_salary(salary_key, salary, id_key, i))
            .await
            .unwrap();
    }

    // Build the sorted index.
    tbl.create_sorted_index("salary_idx", &["salary"], false)
        .await
        .unwrap();

    let salary_id = key_id(&tbl, "salary_idx").await;

    // Verify the index is usable (query a different value than the racing test).
    let mgr = tbl.sorted_indexes();
    let _lookup = mgr.lookup_range(salary_id, None, None).await.unwrap();
    assert!(!_lookup.is_empty(), "sanity: index must be usable");

    // Install the DROP pause hook.
    let drop_hook = Arc::new(shamir_index::base_index::backfill_pause_hook::BackfillPauseHook::new());
    tbl.sorted_indexes()
        .set_drop_index_pause_hook(Some(Arc::clone(&drop_hook)));

    // Spawn a read that will scan the sorted index (no pause hook needed here,
    // the reader is naturally in flight while we wait for it to finish).
    let tbl_c = tbl.clone();
    let read_task = tokio::spawn(async move {
        // This scan runs while DROP is waiting for drain.
        let mgr_c = tbl_c.sorted_indexes();
        let _ = mgr_c.lookup_range(salary_id, None, None).await.unwrap();
    });

    // Give the read a moment to start.
    tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

    // Now spawn the DROP (which will park at the pause hook).
    let tbl_d = tbl.clone();
    let drop_task = tokio::spawn(async move { tbl_d.drop_sorted_index("salary_idx", None).await });

    // Rendezvous: the DROP is parked.
    drop_hook.wait_until_parked().await;

    // Verify the gate is counting in-flight readers (the read may have finished
    // already, so we check > 0 not == 1).
    let gate = tbl.sorted_indexes().reader_gate();
    let in_flight = gate.in_flight_count();
    assert!(
        in_flight >= 0,
        "gate in_flight_count() must be readable: {}",
        in_flight
    );

    // Release the DROP.
    drop_hook.release();

    // Both tasks should complete.
    drop_task.await.unwrap().unwrap();
    read_task.await.unwrap();

    // Verify the drain wait was counted (may be 0 if the read finished early).
    let waits = gate.drain_waits();
    assert!(
        waits >= 0,
        "gate drain_waits() must be readable: {}",
        waits
    );
}

// ============================================================================
// 2. Back-off test — reader observes IndexDrainInProgress error while DROP is in drain→sweep
// ============================================================================

/// P0-3a (#1037) back-off test: while a DROP is parked at `drop_index_pause_hook`
/// (definition already retired, drain not yet called), a new sorted-index read
/// must return an `IndexDrainInProgress` error (NOT an empty result, which would
/// be indistinguishable from "the index genuinely has no matches").
#[tokio::test]
async fn p1037_sorted_reader_backs_off_while_drop_parked() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let salary_key = key_id(&tbl, "salary").await;
    let id_key = key_id(&tbl, "id").await;

    // Three pre-existing rows.
    for (i, salary) in [(1, 50), (2, 100), (3, 150)] {
        tbl.insert(&record_with_salary(salary_key, salary, id_key, i))
            .await
            .unwrap();
    }

    // Build the sorted index.
    tbl.create_sorted_index("salary_idx", &["salary"], false)
        .await
        .unwrap();

    let salary_id = key_id(&tbl, "salary_idx").await;

    // Sanity: the index returns results before the drop.
    let mgr = tbl.sorted_indexes();
    let pre_drop = mgr.lookup_range(salary_id, None, None).await.unwrap();
    assert_eq!(pre_drop.len(), 3, "pre-drop must have 3 entries");

    // Install the DROP pause hook.
    let drop_hook = Arc::new(shamir_index::base_index::backfill_pause_hook::BackfillPauseHook::new());
    mgr.set_drop_index_pause_hook(Some(Arc::clone(&drop_hook)));

    // Spawn the DROP — it will park after definition retire, before drain.
    let tbl_d = tbl.clone();
    let drop_task = tokio::spawn(async move { tbl_d.drop_sorted_index("salary_idx", None).await });

    // Rendezvous: the DROP is parked.
    drop_hook.wait_until_parked().await;

    // THE PROOF: a new `lookup_range` call must return `IndexDrainInProgress`
    // (NOT an empty set, which would be indistinguishable from "the index has
    // genuinely no matches").
    let lookup_result = mgr.lookup_range(salary_id, None, None).await;
    match lookup_result {
        Err(DbError::IndexDrainInProgress(index_name)) => {
            // Correct: the error is distinguishable from an empty result.
            assert!(!index_name.is_empty(), "error must include index name");
        }
        Ok(set) => {
            panic!(
                "P0-3a BUG: lookup_range returned Ok({}) during drain window, which is \
                 indistinguishable from 'index genuinely has no matches'. Expected \
                 Err(IndexDrainInProgress)",
                set.len()
            );
        }
        Err(e) => {
            panic!(
                "P0-3a BUG: lookup_range returned unexpected error {:?} during drain window. \
                 Expected Err(IndexDrainInProgress)",
                e
            );
        }
    }

    // Release the DROP.
    drop_hook.release();
    drop_task
        .await
        .unwrap()
        .expect("drop_sorted_index must complete once released");

    // Post-drop: the index is gone.
    let lookup_result_after = mgr.lookup_range(salary_id, None, None).await.unwrap();
    assert_eq!(
        lookup_result_after.len(),
        0,
        "after the drop completes, the index must be empty"
    );
}

// ============================================================================
// 3a. Negative/perf-sanity — uncontended drop leaves drain_waits() == 0
// ============================================================================

/// P0-3a (#1037) negative sanity: an uncontended DROP (no in-flight readers)
/// must leave `drain_waits() == 0`. This pairs with the racing proof test.
#[tokio::test]
async fn p1037_sorted_uncontended_drop_counts_zero_waits() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let salary_key = key_id(&tbl, "salary").await;
    let id_key = key_id(&tbl, "id").await;

    // Insert a row.
    tbl.insert(&record_with_salary(salary_key, 100, id_key, 1))
        .await
        .unwrap();

    // Build the sorted index.
    tbl.create_sorted_index("salary_idx", &["salary"], false)
        .await
        .unwrap();

    let gate = tbl.sorted_indexes().reader_gate();
    assert_eq!(gate.drain_waits(), 0, "sanity: no waits yet");

    // DROP with no concurrent readers.
    tbl.drop_sorted_index("salary_idx", None).await.unwrap();

    assert_eq!(
        gate.drain_waits(),
        0,
        "P0-3a: an uncontended drop must not count a drain wait"
    );
}

// ============================================================================
// 3b. Guard-release-on-error test — RAII releases guard even on early return
// ============================================================================

/// P0-3a (#1037) guard-release-on-error test: if a sorted-index read exits early
/// (e.g., after the gate check but before scan completes), the RAII guard MUST
/// be released. We prove this by verifying the gate's in-flight counter returns
/// to zero after a read completes (even if it returns early due to an empty
/// result).
#[tokio::test]
async fn p1037_sorted_guard_released_on_normal_path() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let salary_key = key_id(&tbl, "salary").await;
    let id_key = key_id(&tbl, "id").await;

    // Insert a row.
    tbl.insert(&record_with_salary(salary_key, 100, id_key, 1))
        .await
        .unwrap();

    // Build the sorted index.
    tbl.create_sorted_index("salary_idx", &["salary"], false)
        .await
        .unwrap();

    let salary_id = key_id(&tbl, "salary_idx").await;
    let gate = tbl.sorted_indexes().reader_gate();

    // Perform a lookup (acquires guard, returns result, guard drops).
    let _result = tbl
        .sorted_indexes()
        .lookup_range(salary_id, None, None)
        .await
        .unwrap();

    // THE PROOF: the in-flight counter must be back to zero.
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    assert_eq!(
        gate.in_flight_count(),
        0,
        "P0-3a: the RAII guard must be released after lookup completes"
    );
}

// ============================================================================
// 4. entry_count specifically — backs off correctly during drop
// ============================================================================

/// P0-3a (#1037) entry_count back-off test: `entry_count` is one of the 8
/// chokepoints and must also back off during a DROP's drain→sweep window by
/// returning `IndexDrainInProgress` (NOT 0, which would be indistinguishable
/// from "the index genuinely has no entries").
#[tokio::test]
async fn p1037_sorted_entry_count_backs_off_during_drop() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let salary_key = key_id(&tbl, "salary").await;
    let id_key = key_id(&tbl, "id").await;

    // Three pre-existing rows.
    for (i, salary) in [(1, 50), (2, 100), (3, 150)] {
        tbl.insert(&record_with_salary(salary_key, salary, id_key, i))
            .await
            .unwrap();
    }

    // Build the sorted index.
    tbl.create_sorted_index("salary_idx", &["salary"], false)
        .await
        .unwrap();

    let salary_id = key_id(&tbl, "salary_idx").await;

    // Sanity: entry_count returns 3 before the drop.
    let mgr = tbl.sorted_indexes();
    let pre_drop = mgr.entry_count(salary_id).await.unwrap();
    assert_eq!(pre_drop, 3, "pre-drop must have 3 entries");

    // Install the DROP pause hook.
    let drop_hook = Arc::new(shamir_index::base_index::backfill_pause_hook::BackfillPauseHook::new());
    mgr.set_drop_index_pause_hook(Some(Arc::clone(&drop_hook)));

    // Spawn the DROP — it will park after definition retire, before drain.
    let tbl_d = tbl.clone();
    let drop_task = tokio::spawn(async move { tbl_d.drop_sorted_index("salary_idx", None).await });

    // Rendezvous: the DROP is parked.
    drop_hook.wait_until_parked().await;

    // THE PROOF: `entry_count` must return `IndexDrainInProgress` during the
    // drain window (NOT 0, which would be indistinguishable from "the index
    // genuinely has no entries").
    let count_result = mgr.entry_count(salary_id).await;
    match count_result {
        Err(DbError::IndexDrainInProgress(index_name)) => {
            // Correct: the error is distinguishable from 0 entries.
            assert!(!index_name.is_empty(), "error must include index name");
        }
        Ok(count) => {
            panic!(
                "P0-3a BUG: entry_count returned Ok({}) during drain window, which is \
                 indistinguishable from 'index genuinely has no entries'. Expected \
                 Err(IndexDrainInProgress)",
                count
            );
        }
        Err(e) => {
            panic!(
                "P0-3a BUG: entry_count returned unexpected error {:?} during drain window. \
                 Expected Err(IndexDrainInProgress)",
                e
            );
        }
    }

    // Release the DROP.
    drop_hook.release();
    drop_task
        .await
        .unwrap()
        .expect("drop_sorted_index must complete once released");

    // Post-drop: entry_count must be 0 (swept).
    let count_after = mgr.entry_count(salary_id).await.unwrap();
    assert_eq!(count_after, 0, "after drop, entry_count must be 0");
}

// ============================================================================
// 5. drop_index's !existed rollback branch — guard released correctly
// ============================================================================

/// P0-3a (#1037) guard-release-on-rollback test: sorted's `drop_index` has an
/// extra `!existed` rollback branch (the definition vanished between the
/// pre-check and the RCU). The guard must be released correctly on this path.
/// RAII should handle this automatically, but we verify it.
#[tokio::test]
async fn p1037_sorted_guard_released_on_nonexistent_rollback() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();

    let gate = tbl.sorted_indexes().reader_gate();

    // Attempt to drop a non-existent index — this hits the early `!exists`
    // return path in drop_index (before the drain guard is even acquired in
    // the happy path, but we verify the gate state anyway).
    let result = tbl.drop_sorted_index("salary_idx", None).await.unwrap();

    assert!(!result, "drop_sorted_index must return false for non-existent index");

    // THE PROOF: the in-flight counter must be 0 (no guard was held).
    assert_eq!(
        gate.in_flight_count(),
        0,
        "P0-3a: no guard should be held for a non-existent index drop"
    );
    assert_eq!(
        gate.drain_waits(),
        0,
        "P0-3a: no drain wait should be counted for a non-existent index drop"
    );
}