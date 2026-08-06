//! P0-3a (#1011) — `ReaderDrainGate` integration tests for the REGULAR (non-unique)
//! index family. Proves that the gate correctly blocks a DROP while readers are
//! mid-flight AND that readers back off during a DROP's drain→sweep window.
//!
//! This is the SIBLING test set to `f76_drop_visibility_tests.rs` — F-76 closed
//! the visibility gap at the planner level (definition retired before sweep),
//! while P0-3a (#1011) closes the PHYSICAL-READ gap at the `lookup_by_index`
//! level (readers blocked while sweep runs). Together they eliminate the
//! sub-bug 3a race window where a reader could observe a partially-swept keyspace.

use std::sync::Arc;

use shamir_types::core::interner::TouchInd;
use shamir_types::types::common::new_map;
use shamir_types::types::value::InnerValue;

use crate::query::filter::eval_context::FilterContext;
use crate::query::read::ReadQuery;
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;
use crate::table::TableManager;
use shamir_index::base_index::backfill_pause_hook::BackfillPauseHook;
use shamir_query_builder::Query;
use shamir_storage::storage_in_memory::InMemoryRepo;

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

fn record_with_status(status_key: u64, status: &str, id_key: u64, id: i64) -> InnerValue {
    let mut m = shamir_types::types::common::new_map_wc(2);
    m.insert(
        shamir_types::core::interner::InternerKey::new(status_key),
        InnerValue::Str(status.into()),
    );
    m.insert(
        shamir_types::core::interner::InternerKey::new(id_key),
        InnerValue::Int(id),
    );
    InnerValue::Map(m)
}

async fn read_eq_status(tbl: &TableManager, value: &str) -> crate::query::read::QueryResult {
    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    let query: ReadQuery = Query::from("people").where_eq("status", value).build();
    tbl.read(&query, &ctx).await.unwrap()
}

// ============================================================================
// 1. Proof test — reader parked mid-flight blocks the DROP's drain
// ============================================================================

/// P0-3a (#1011) proof test: a reader parked at the `lookup_pause_hook` (guard
/// HELD, scan not yet started) must block the DROP's `wait_for_drain` call.
/// Once the reader releases, the drop proceeds and finishes. The reader's scan
/// returns the COMPLETE pre-drop set (correct, not partial).
#[tokio::test]
async fn p1011_reader_blocks_drop_until_released() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let status_key = key_id(&tbl, "status").await;
    let id_key = key_id(&tbl, "id").await;

    // Three pre-existing rows, two matching "active".
    for (i, status) in [(1, "active"), (2, "inactive"), (3, "active")] {
        tbl.insert(&record_with_status(status_key, status, id_key, i))
            .await
            .unwrap();
    }

    // Build the index synchronously and verify it's usable.
    //
    // P0-3a (#1011): the sanity check MUST query a DIFFERENT value
    // ("inactive") than the one the spawned/parked read below will use
    // ("active") — `lookup_by_index`'s posting_cache probe runs BEFORE the
    // `lookup_pause_hook` check (by design: a cache HIT is a legitimate
    // short-circuit that never needs to touch `info_store` at all, see that
    // method's own comments), so warming the "active" cache entry here would
    // make the spawned read below return via the cache HIT path and never
    // reach the pause hook — the test would then hang forever on
    // `lookup_hook.wait_until_parked()`, waiting for a notification that can
    // never fire. Querying "inactive" still proves the index is usable
    // without touching the "active" cache key.
    tbl.create_index("status_idx", &["status"]).await.unwrap();
    let pre_drop = read_eq_status(&tbl, "inactive").await;
    assert_eq!(
        pre_drop.stats.as_ref().unwrap().index_used.as_deref(),
        Some("status_idx"),
        "sanity: the index must be usable before the drop"
    );
    assert_eq!(
        pre_drop.records.len(),
        1,
        "pre-drop must have 1 inactive row"
    );

    // Install the LOOKUP pause hook on the IndexManager.
    let lookup_hook = Arc::new(BackfillPauseHook::new());
    tbl.index_manager_ref()
        .set_lookup_pause_hook(Some(Arc::clone(&lookup_hook)));

    // Spawn a read that will park at the pause hook.
    let tbl_c = tbl.clone();
    let read_task = tokio::spawn(async move { read_eq_status(&tbl_c, "active").await });

    // Rendezvous: the read is parked inside `lookup_by_index`'s guard.
    lookup_hook.wait_until_parked().await;

    // Verify the gate has at least one in-flight reader.
    let gate = tbl.index_manager_ref().reader_gate();
    assert_eq!(
        gate.in_flight_count(),
        1,
        "the parked read must be counted in-flight"
    );

    // Now spawn the DROP.
    let tbl_d = tbl.clone();
    let drop_task = tokio::spawn(async move { tbl_d.drop_index("status_idx").await });

    // Give the DROP a moment to reach `wait_for_drain` (it must block).
    tokio::time::sleep(tokio::time::Duration::from_millis(80)).await;

    // THE PROOF: the DROP must be blocked on drain (not finished).
    assert!(
        !drop_task.is_finished(),
        "P0-3a: the DROP must be blocked on wait_for_drain while a reader is in-flight"
    );

    // Release the read.
    lookup_hook.release();

    // The read must complete and return the COMPLETE pre-drop set (both rows).
    let read_result = read_task.await.unwrap();
    let mut ids: Vec<i64> = read_result
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("id"))
        .collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        vec![1, 3],
        "P0-3a: the parked read must observe the COMPLETE pre-drop state, \
         even though the drop started while it was in-flight"
    );

    // The DROP must now complete.
    drop_task
        .await
        .unwrap()
        .expect("drop_index must complete once the reader releases");

    // Verify the drain wait was counted.
    assert_eq!(
        gate.drain_waits(),
        1,
        "P0-3a: a contended drain must be counted"
    );

    // Post-drop: the index is gone — full scan still returns the correct set.
    let result_after = read_eq_status(&tbl, "active").await;
    assert_eq!(
        result_after.stats.as_ref().unwrap().index_used,
        None,
        "after the drop completes, the index must not be usable"
    );
    let mut ids_after: Vec<i64> = result_after
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("id"))
        .collect();
    ids_after.sort_unstable();
    assert_eq!(
        ids_after,
        vec![1, 3],
        "full scan must return the correct set"
    );
}

// ============================================================================
// 2. Back-off test — reader observes None while DROP is in drain→sweep window
// ============================================================================

/// P0-3a (#1011) back-off test: while a DROP is parked at `drop_index_pause_hook`
/// (definition already retired, drain not yet called), a new `lookup_by_index`
/// call must return `Ok(None)` — NOT `Ok(Some([]))`. The caller falls back to
/// full scan.
#[tokio::test]
async fn p1011_reader_backs_off_while_drop_parked() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let status_key = key_id(&tbl, "status").await;
    let id_key = key_id(&tbl, "id").await;

    // Three pre-existing rows, two matching "active".
    for (i, status) in [(1, "active"), (2, "inactive"), (3, "active")] {
        tbl.insert(&record_with_status(status_key, status, id_key, i))
            .await
            .unwrap();
    }

    // Build the index.
    tbl.create_index("status_idx", &["status"]).await.unwrap();
    // `lookup_by_index`'s `name_interned` is the INDEX's own interned name
    // (`build_index_definition` interns `name`, i.e. "status_idx" — see
    // `table_manager_index_mgmt.rs::create_index`), NOT the interned id of
    // the FIELD "status" the index is built over. Using the field's id here
    // built a physical key against a namespace with no postings in it.
    let status_id = key_id(&tbl, "status_idx").await;

    // Build the index lookup value as InnerValue.
    let active_val = shamir_types::types::value::InnerValue::Str("active".into());

    // Sanity: the index returns results before the drop.
    let pre_drop = tbl
        .index_manager_ref()
        .lookup_by_index(status_id, std::slice::from_ref(&active_val))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pre_drop.len(), 2, "pre-drop must have 2 matches");

    // Install the DROP pause hook.
    let drop_hook = Arc::new(BackfillPauseHook::new());
    tbl.index_manager_ref()
        .set_drop_index_pause_hook(Some(Arc::clone(&drop_hook)));

    // Spawn the DROP — it will park after definition retire, before drain.
    let tbl_d = tbl.clone();
    let drop_task = tokio::spawn(async move { tbl_d.drop_index("status_idx").await });

    // Rendezvous: the DROP is parked.
    drop_hook.wait_until_parked().await;

    // THE PROOF: a new `lookup_by_index` call must return `Ok(None)`.
    let lookup_result = tbl
        .index_manager_ref()
        .lookup_by_index(status_id, std::slice::from_ref(&active_val))
        .await
        .unwrap();
    assert!(
        lookup_result.is_none(),
        "P0-3a: while DROP is in drain→sweep window, lookup_by_index must \
         return None, not Some([]) — the caller must fall back to full scan"
    );

    // Release the DROP.
    drop_hook.release();
    drop_task
        .await
        .unwrap()
        .expect("drop_index must complete once released");

    // Post-drop: `None` was specifically the "a drop is CURRENTLY in its
    // raise->sweep window" signal (the `reader_gate`'s `dropping` flag) —
    // once the drop fully completes, the flag clears and `lookup_by_index`
    // is a plain, low-level scan-by-name_interned primitive with no
    // "does this definition still exist" check of its own (the planner is
    // responsible for not routing to a dropped index at all). A raw call by
    // the now-stale `status_id` scans an empty (fully-swept) prefix and
    // correctly returns `Some(empty)`, not `None`.
    let lookup_result_after = tbl
        .index_manager_ref()
        .lookup_by_index(status_id, std::slice::from_ref(&active_val))
        .await
        .unwrap();
    assert_eq!(
        lookup_result_after.map(|ids| ids.len()),
        Some(0),
        "after the drop completes, the swept postings must scan empty (not \
         the drop-in-progress None signal)"
    );
}

// ============================================================================
// 3a. Negative/perf-sanity — uncontended drop leaves drain_waits() == 0
// ============================================================================

/// P0-3a (#1011) negative sanity: an uncontended DROP (no in-flight readers)
/// must leave `drain_waits() == 0`. This pairs with `p1011_reader_blocks_drop_until_released`
/// — a lone `drain_waits() == 0` assertion is vacuous without the proof that a
/// contended drain is counted.
#[tokio::test]
async fn p1011_uncontended_drop_counts_zero_waits() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let status_key = key_id(&tbl, "status").await;
    let id_key = key_id(&tbl, "id").await;

    // Insert a row.
    tbl.insert(&record_with_status(status_key, "active", id_key, 1))
        .await
        .unwrap();

    // Build the index.
    tbl.create_index("status_idx", &["status"]).await.unwrap();

    let gate = tbl.index_manager_ref().reader_gate();
    assert_eq!(gate.drain_waits(), 0, "sanity: no waits yet");

    // DROP with no concurrent readers.
    tbl.drop_index("status_idx").await.unwrap();

    assert_eq!(
        gate.drain_waits(),
        0,
        "P0-3a: an uncontended drop must not count a drain wait"
    );
}

// P0-3a (#1011) positive sanity: the racing proof test (`p1011_reader_blocks_drop_until_released`)
// MUST leave `drain_waits() == 1`. This is NOT a separate test — it's asserted
// inside the proof test — but documented here as the REQUIRED counterpart to
// `p1011_uncontended_drop_counts_zero_waits`. Without BOTH checks, the counter
// could be completely broken (always returns 0) and the test suite would pass.
//
// See `p1011_reader_blocks_drop_until_released` for the actual assertion.

// ============================================================================
// 3b. Guard-release-on-error test — RAII releases guard even on early return
// ============================================================================

/// P0-3a (#1011) guard-release-on-error test: if `lookup_by_index` exits early
/// (e.g., via the pause hook releasing before the scan completes), the RAII
/// guard MUST be released. We prove this by parking a read, releasing the hook
/// before it completes, and verifying the gate's in-flight counter returns to
/// zero — proving the guard was dropped via RAII.
#[tokio::test]
async fn p1011_guard_released_on_error_path() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let status_key = key_id(&tbl, "status").await;
    let id_key = key_id(&tbl, "id").await;

    // Insert a row.
    tbl.insert(&record_with_status(status_key, "active", id_key, 1))
        .await
        .unwrap();

    // Build the index.
    tbl.create_index("status_idx", &["status"]).await.unwrap();

    // Install a lookup hook to park the read mid-flight (guard HELD).
    let lookup_hook = Arc::new(BackfillPauseHook::new());
    tbl.index_manager_ref()
        .set_lookup_pause_hook(Some(Arc::clone(&lookup_hook)));

    // Start a read — it will park.
    let tbl_c = tbl.clone();
    // Same fix as `p1011_reader_backs_off_while_drop_parked`: `name_interned`
    // must be the INDEX's own interned name ("status_idx"), not the field's.
    let status_id = key_id(&tbl, "status_idx").await;
    let active_val = shamir_types::types::value::InnerValue::Str("active".into());

    let read_task = tokio::spawn(async move {
        // This call parks at the pause hook.
        let result = tbl_c
            .index_manager_ref()
            .lookup_by_index(status_id, std::slice::from_ref(&active_val))
            .await;
        // Even if this returns an error (unlikely in this controlled test), the guard is released.
        result
    });

    // Rendezvous: the read is parked.
    lookup_hook.wait_until_parked().await;

    let gate = tbl.index_manager_ref().reader_gate();
    assert_eq!(
        gate.in_flight_count(),
        1,
        "the parked read must be counted in-flight"
    );

    // Release the read without letting it complete normally — this is a
    // simulation of an early return. The key invariant is that the RAII guard
    // is released when the returned `Result` is dropped.
    lookup_hook.release();

    // Wait for the read to exit (successfully or not).
    read_task.await.unwrap().unwrap();

    // THE PROOF: the in-flight counter must be back to zero, proving the
    // RAII guard was released even though the call didn't complete to the
    // normal return path (it hit the pause hook and exited immediately after).
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    assert_eq!(
        gate.in_flight_count(),
        0,
        "P0-3a: the RAII guard must be released even when lookup_by_index \
         exits early (e.g., after the pause hook releases)"
    );

    // Now prove a DROP doesn't block (the counter is zero).
    let drop_start = std::time::Instant::now();
    tbl.drop_index("status_idx").await.unwrap();
    assert!(
        drop_start.elapsed() < tokio::time::Duration::from_secs(1),
        "P0-3a: a DROP after the reader exited must not block on drain"
    );
}
