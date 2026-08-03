//! F-72 (#899, P0) — base_index regular-hash / sorted CREATE INDEX must be
//! planner-invisible until backfill completes.
//!
//! F-57 (#883) and F-70 (#897) serialise CREATE INDEX against WRITERS
//! (write-barrier + drain + `unique_write_lock`). That lock is held by
//! WRITERS, not READERS — it does nothing to hide a partially-built index
//! from the query PLANNER. Pre-fix, `TableManager::create_index` /
//! `create_sorted_index_with_include` published the new definition into the
//! planner-visible registry (`IndexManager::iter_indexes` /
//! `SortedIndexManager::find_by_field`) BEFORE the streamed backfill loop
//! populated any postings — so a concurrent read issued while the backfill
//! was still in flight could be planned against a half-built index and
//! silently return FEWER rows than actually exist (the read never even
//! attempts the full-scan fallback that would have found them).
//!
//! The fix (F-72): both families register at `state = Building`, which is
//! invisible to every PLANNER lookup (`IndexManager::iter_indexes_ready`,
//! `SortedIndexManager::find_by_field_ready`). The definition flips to
//! `Ready` only once the backfill has fully completed. A concurrent read
//! during the backfill therefore falls through to the full-scan path and
//! returns the complete, correct row set.
//!
//! Each test below proves this independently, per family, using a
//! deterministic pause-seam hook (no `sleep`-based timing — mirrors the
//! `BackfillPauseHook` convention `index2_create_barrier_tests.rs` uses for
//! `create_index_v2`):
//!
//! 1. Insert N pre-existing rows.
//! 2. Install the family's backfill pause hook and spawn CREATE INDEX.
//! 3. Rendezvous on `wait_until_parked()` — the create is now guaranteed to
//!    be mid-backfill, with its definition registered at `Building`.
//! 4. Issue a concurrent read that WOULD use the index if it were visible
//!    (an `Eq` for the regular-hash family, a `Between` for the sorted
//!    family).
//! 5. Assert the read's `index_used` is `None` (full scan, not the
//!    half-built index) AND the row set is the COMPLETE, correct set —
//!    proving the read did not silently truncate.
//! 6. Release the create, let it finish, and assert a SECOND read of the
//!    same query now DOES use the index and still returns the same complete
//!    row set (the index is fully queryable once `Ready`).

use std::sync::Arc;

use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::value::InnerValue;

use crate::query::filter::eval_context::FilterContext;
use crate::query::read::ReadQuery;
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;
use crate::table::TableManager;
use shamir_query_builder::Query;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::types::common::new_map;

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
    let mut m = new_map_wc(2);
    m.insert(InternerKey::new(status_key), InnerValue::Str(status.into()));
    m.insert(InternerKey::new(id_key), InnerValue::Int(id));
    InnerValue::Map(m)
}

fn record_with_score(score_key: u64, score: i64) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(InternerKey::new(score_key), InnerValue::Int(score));
    InnerValue::Map(m)
}

async fn read_eq_status(tbl: &TableManager, value: &str) -> crate::query::read::QueryResult {
    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    let query: ReadQuery = Query::from("people").where_eq("status", value).build();
    tbl.read(&query, &ctx).await.unwrap()
}

async fn read_between_score(
    tbl: &TableManager,
    lo: i64,
    hi: i64,
) -> crate::query::read::QueryResult {
    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    let query: ReadQuery = Query::from("nums").where_between("score", lo, hi).build();
    tbl.read(&query, &ctx).await.unwrap()
}

// ============================================================================
// 1. Regular hash index — `create_index` planner-invisibility during backfill
// ============================================================================

/// F-72: a concurrent `Eq` read issued while `create_index`'s backfill is
/// mid-stream (definition registered at `Building`) must NOT be planned
/// against the half-built index — it must fall back to a full scan and
/// return the COMPLETE, correct row set. Once the create finishes (state
/// flips to `Ready`), the SAME query must then use the index.
#[tokio::test]
async fn f72_regular_index_planner_invisible_during_backfill() {
    use shamir_index::base_index::backfill_pause_hook::BackfillPauseHook;

    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let status_key = key_id(&tbl, "status").await;
    let id_key = key_id(&tbl, "id").await;

    // Three pre-existing rows, two matching "active" — this is the COMPLETE
    // correct set the concurrent read must observe regardless of whether the
    // index is used.
    for (i, status) in [(1, "active"), (2, "inactive"), (3, "active")] {
        tbl.insert(&record_with_status(status_key, status, id_key, i))
            .await
            .unwrap();
    }

    // Install the pause hook on the low-level IndexManager (create_index's
    // backfill lives in `shamir-index`, a lower crate than `TableManager`).
    let hook = Arc::new(BackfillPauseHook::new());
    tbl.index_manager_ref()
        .set_create_index_backfill_hook(Some(Arc::clone(&hook)));

    // Spawn the create; it registers the def at `Building`, then parks
    // mid-backfill (postings written, still `Building`, pre-`Ready`-flip).
    let tbl_c = tbl.clone();
    let create = tokio::spawn(async move { tbl_c.create_index("status_idx", &["status"]).await });

    // Rendezvous: the create is now guaranteed to be mid-backfill.
    hook.wait_until_parked().await;

    // THE PROOF: a concurrent Eq read on "status" must fall back to a full
    // scan (index_used == None) and return the COMPLETE correct set, not a
    // truncated one from a half-built index.
    let result = read_eq_status(&tbl, "active").await;
    assert_eq!(
        result.stats.as_ref().unwrap().index_used,
        None,
        "F-72: a Building (mid-backfill) regular index must be invisible to \
         the planner — this read must fall back to a full scan, not use the \
         half-built index"
    );
    let mut ids: Vec<i64> = result
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("id"))
        .collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        vec![1, 3],
        "F-72: the full-scan fallback must return the COMPLETE correct row \
         set — a truncated result here would mean the read was (wrongly) \
         planned against the half-built index"
    );

    // Release the create; it finishes the backfill and flips Building →
    // Ready.
    hook.release();
    create
        .await
        .unwrap()
        .expect("create_index must complete once released");

    // Post-create: the SAME query now DOES use the index, and still returns
    // the same complete, correct set.
    let result_after = read_eq_status(&tbl, "active").await;
    assert_eq!(
        result_after.stats.as_ref().unwrap().index_used,
        Some("status_idx".to_string()),
        "once Ready, the index must be used by the planner"
    );
    let mut ids_after: Vec<i64> = result_after
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("id"))
        .collect();
    ids_after.sort_unstable();
    assert_eq!(ids_after, vec![1, 3]);
}

// ============================================================================
// 2. Sorted index — `create_sorted_index_with_include` planner-invisibility
//    during backfill
// ============================================================================

/// F-72: a concurrent `Between` read issued while
/// `create_sorted_index_with_include`'s backfill is mid-stream (definition
/// registered at `Building`) must NOT be planned against the half-built
/// sorted index — it must fall back to a full scan and return the COMPLETE,
/// correct row set. Once the create finishes, the SAME query must then use
/// the sorted index.
#[tokio::test]
async fn f72_sorted_index_planner_invisible_during_backfill() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("nums"));
    let tbl = repo.get_table("nums").await.unwrap();
    let score_key = key_id(&tbl, "score").await;

    // Five pre-existing rows; the correct answer to `score BETWEEN 10 AND 30`
    // is {10, 20, 30} — this is the COMPLETE correct set the concurrent read
    // must observe regardless of whether the sorted index is used.
    for score in [5, 10, 20, 30, 40] {
        tbl.insert(&record_with_score(score_key, score))
            .await
            .unwrap();
    }

    let hook = Arc::new(crate::table::index2_backfill_hook::BackfillPauseHook::new());
    tbl.set_create_sorted_index_backfill_hook(Some(Arc::clone(&hook)));

    // Spawn the create; it registers the def at `Building`, then parks
    // mid-backfill.
    let tbl_c = tbl.clone();
    let create =
        tokio::spawn(async move { tbl_c.create_sorted_index("score_sorted", &["score"]).await });

    // Rendezvous: the create is now guaranteed to be mid-backfill.
    hook.wait_until_parked().await;

    // THE PROOF: a concurrent Between read on "score" must fall back to a
    // full scan (index_used == None) and return the COMPLETE correct set.
    let result = read_between_score(&tbl, 10, 30).await;
    assert_eq!(
        result.stats.as_ref().unwrap().index_used,
        None,
        "F-72: a Building (mid-backfill) sorted index must be invisible to \
         the planner — this read must fall back to a full scan, not use the \
         half-built index"
    );
    let mut scores: Vec<i64> = result
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("score"))
        .collect();
    scores.sort_unstable();
    assert_eq!(
        scores,
        vec![10, 20, 30],
        "F-72: the full-scan fallback must return the COMPLETE correct row \
         set — a truncated result here would mean the read was (wrongly) \
         planned against the half-built sorted index"
    );

    // Release the create; it finishes the backfill and flips Building →
    // Ready.
    hook.release();
    create
        .await
        .unwrap()
        .expect("create_sorted_index must complete once released");

    // Post-create: the SAME query now DOES use the sorted index, and still
    // returns the same complete, correct set. Sorted-index scans report
    // `index_used` as `sorted_idx_<name_interned>` (the numeric interned id,
    // not the string name) — see `read_exec.rs`'s `try_plan_and_index_scan`
    // ExplainPlan arm — so just assert it is `Some` (i.e. NOT the `None`
    // full-scan fallback asserted above).
    let result_after = read_between_score(&tbl, 10, 30).await;
    assert!(
        result_after
            .stats
            .as_ref()
            .unwrap()
            .index_used
            .as_deref()
            .is_some_and(|s| s.starts_with("sorted_idx_")),
        "once Ready, the sorted index must be used by the planner, got: {:?}",
        result_after.stats.as_ref().unwrap().index_used
    );
    let mut scores_after: Vec<i64> = result_after
        .records
        .iter()
        .filter_map(|r| r.get_value_i64("score"))
        .collect();
    scores_after.sort_unstable();
    assert_eq!(scores_after, vec![10, 20, 30]);
}
