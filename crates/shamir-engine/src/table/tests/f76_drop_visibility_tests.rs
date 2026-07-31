//! F-76 (#903) — DROP INDEX visibility window: a concurrent reader during a
//! DROP must observe EITHER the complete index's correct result OR a
//! full-scan fallback — NEVER a registered-but-partially-emptied index
//! returning wrong/incomplete results.
//!
//! Mirror image of F-72 (#899): F-72 closed the CREATE-side window (a
//! Building index is invisible to the planner until backfill completes).
//! F-76 closes the DROP-side window: the definition is retired from the
//! planner-visible registry BEFORE the posting sweep starts, so a
//! concurrent reader can never select a registered-but-emptied index.
//!
//! Each test uses the codebase's deterministic pause-seam convention
//! (no `sleep`-based timing): the DROP is parked between the definition
//! retirement and the posting sweep, and a concurrent read / planner probe
//! is issued deterministically into that exact window.

use std::sync::Arc;

use shamir_query_types::admin::types::CreateIndexOp;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::{new_map, new_map_wc};
use shamir_types::types::value::InnerValue;

use crate::query::filter::eval_context::FilterContext;
use crate::query::filter::{Filter, FilterValue};
use crate::query::read::ReadQuery;
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;
use crate::table::TableManager;
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
    let mut m = new_map_wc(2);
    m.insert(InternerKey::new(status_key), InnerValue::Str(status.into()));
    m.insert(InternerKey::new(id_key), InnerValue::Int(id));
    InnerValue::Map(m)
}

fn record_with_str(key: u64, val: &str) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(InternerKey::new(key), InnerValue::Str(val.into()));
    InnerValue::Map(m)
}

async fn read_eq_status(tbl: &TableManager, value: &str) -> crate::query::read::QueryResult {
    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    let query: ReadQuery = Query::from("people").where_eq("status", value).build();
    tbl.read(&query, &ctx).await.unwrap()
}

/// A functional `lower(<field>)` index create op (mirrors
/// `index2_lifecycle_state_tests.rs`).
fn functional_lower_op(name: &str, table: &str, field: &str) -> CreateIndexOp {
    CreateIndexOp {
        create_index: name.into(),
        table: table.into(),
        fields: vec![vec![field.into()]],
        unique: false,
        sorted: false,
        repo: "main".into(),
        index_type: Some("functional".into()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: Some("lower".into()),
        functional_args: None,
        vector_dim: None,
        vector_metric: None,
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    }
}

// ============================================================================
// 1. Regular hash index — DROP retires the definition BEFORE the posting sweep
// ============================================================================

/// F-76: a concurrent `Eq` read issued while `drop_index`'s posting sweep is
/// parked (definition already retired, postings not yet swept) must NOT be
/// planned against the index — it must fall back to a full scan and return
/// the COMPLETE, correct row set. Once the drop finishes, the same query
/// still returns the correct set via full scan (the index no longer exists).
#[tokio::test]
async fn f76_regular_hash_drop_invisible_during_sweep() {
    use shamir_index::legacy::backfill_pause_hook::BackfillPauseHook;

    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let status_key = key_id(&tbl, "status").await;
    let id_key = key_id(&tbl, "id").await;

    // Three pre-existing rows, two matching "active" — this is the COMPLETE
    // correct set the concurrent read must observe regardless of the drop.
    for (i, status) in [(1, "active"), (2, "inactive"), (3, "active")] {
        tbl.insert(&record_with_status(status_key, status, id_key, i))
            .await
            .unwrap();
    }

    // Build the index synchronously and verify it is usable.
    tbl.create_index("status_idx", &["status"]).await.unwrap();
    let pre_drop = read_eq_status(&tbl, "active").await;
    assert_eq!(
        pre_drop.stats.as_ref().unwrap().index_used.as_deref(),
        Some("status_idx"),
        "sanity: the index must be usable before the drop"
    );

    // Install the DROP pause hook on the low-level IndexManager (the DROP
    // path lives in `shamir-index`, a lower crate).
    let hook = Arc::new(BackfillPauseHook::new());
    tbl.index_manager_ref()
        .set_drop_index_pause_hook(Some(Arc::clone(&hook)));

    // Spawn the DROP; it retires the definition FIRST (RCU swap), then parks
    // — postings not yet swept, definition already gone from the planner.
    let tbl_c = tbl.clone();
    let drop_task = tokio::spawn(async move { tbl_c.drop_index("status_idx").await });

    // Rendezvous: the DROP is parked — definition retired, postings intact.
    hook.wait_until_parked().await;

    // THE PROOF: a concurrent Eq read on "status" must fall back to a full
    // scan (index_used == None) and return the COMPLETE correct set — not a
    // truncated result from a registered-but-emptied index.
    let result = read_eq_status(&tbl, "active").await;
    assert_eq!(
        result.stats.as_ref().unwrap().index_used,
        None,
        "F-76: a dropped (definition retired) regular index must be \
         invisible to the planner — this read must fall back to a full scan, \
         not select the emptying index"
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
        "F-76: the full-scan fallback must return the COMPLETE correct row \
         set — an empty/truncated result here would mean the read was \
         (wrongly) planned against the emptying index"
    );

    // Release the DROP; it finishes the sweep and persists.
    hook.release();
    drop_task
        .await
        .unwrap()
        .expect("drop_index must complete once released");

    // Post-drop: the index no longer exists — the read still returns the
    // correct set via full scan.
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
    assert_eq!(ids_after, vec![1, 3]);
}

// ============================================================================
// 2. Index2 (functional) — DROP retires the backend BEFORE the posting sweep
// ============================================================================

/// F-76: while `drop_index2` is parked (backend already retired from the
/// registry via `remove_by_id`, postings not yet swept via `drop_all`),
/// `try_plan_index2` must return `None` — the planner cannot see the
/// backend, so reads fall back to a full scan. Before the drop, the same
/// planner probe returned `Some`. After the drop, it still returns `None`.
#[tokio::test]
async fn f76_index2_drop_invisible_during_sweep() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let name_field = key_id(&tbl, "name").await;

    // Insert rows whose lower-cased "name" the functional index will map.
    tbl.insert(&record_with_str(name_field, "Alice"))
        .await
        .unwrap();
    tbl.insert(&record_with_str(name_field, "Bob"))
        .await
        .unwrap();
    tbl.insert(&record_with_str(name_field, "alice"))
        .await
        .unwrap();

    // Build the functional lower(name) index synchronously.
    tbl.create_index_v2(&functional_lower_op("lower_name", "people", "name"))
        .await
        .unwrap();

    // The filter the planner would route to the functional backend.
    let filter = Filter::Computed {
        expr_op: "lower".into(),
        field: vec!["name".into()],
        expr_args: None,
        cmp: "eq".into(),
        value: FilterValue::String("alice".into()),
    };

    // Sanity: before the drop, the planner DOES find the backend.
    {
        let interner = tbl.interner().get().await.unwrap();
        let plan = tbl.try_plan_index2(&filter, interner).await;
        assert!(
            plan.is_some(),
            "sanity: the functional backend must be plannable before the drop"
        );
    }

    // Install the index2 DROP pause hook (fires between remove_by_id and
    // drop_all — the exact visibility window this task closes).
    let hook = Arc::new(crate::table::index2_backfill_hook::BackfillPauseHook::new());
    tbl.set_drop_index2_pause_hook(Some(Arc::clone(&hook)));

    // Spawn the DROP.
    let tbl_c = tbl.clone();
    let drop_task = tokio::spawn(async move { tbl_c.drop_index2("lower_name").await });

    // Rendezvous: the DROP is parked — backend retired from the registry,
    // postings not yet swept.
    hook.wait_until_parked().await;

    // THE PROOF: the planner must NOT find the backend (it was retired from
    // the registry before the sweep).
    let interner = tbl.interner().get().await.unwrap();
    let plan = tbl.try_plan_index2(&filter, interner).await;
    assert!(
        plan.is_none(),
        "F-76: a dropped (registry-retired) index2 backend must be invisible \
         to the planner — try_plan_index2 must return None so reads fall back \
         to a full scan instead of selecting the emptying backend"
    );

    // The backend must also be gone from the registry's name lookup.
    let name_id = key_id(&tbl, "lower_name").await;
    let still_registered = tbl.index2_registry().get_by_name(name_id).await;
    assert!(
        still_registered.is_none(),
        "F-76: the backend must be removed from the registry before the sweep"
    );

    // Release the DROP; it finishes the sweep and persists.
    hook.release();
    drop_task
        .await
        .unwrap()
        .expect("drop_index2 must complete once released");

    // Post-drop: the planner still returns None.
    let interner = tbl.interner().get().await.unwrap();
    let plan_after = tbl.try_plan_index2(&filter, interner).await;
    assert!(
        plan_after.is_none(),
        "after the drop completes, the backend must remain invisible"
    );
}

// ============================================================================
// 3. Unique hash index — DROP retires the definition + lowers the barrier
//    BEFORE the posting sweep
// ============================================================================

/// F-76: while `drop_unique_index` is parked (definition already retired via
/// RCU swap, write-barrier bit already cleared, postings not yet swept), the
/// unique index definition must already be gone from the planner-visible
/// registry and `has_unique_indexes()` must already be false. This proves
/// the definition retirement + barrier lowering happen BEFORE the sweep —
/// the mirror-image-of-F-72 ordering fix for the unique family.
///
/// (The read planner does not route queries through unique indexes — they
/// enforce uniqueness on the write path — so the proof here checks the
/// definition/visibility registry state directly rather than issuing a
/// planner-routed read.)
#[tokio::test]
async fn f76_unique_hash_drop_retires_definition_before_sweep() {
    use shamir_index::legacy::backfill_pause_hook::BackfillPauseHook;

    let repo = make_repo();
    repo.add_table(TableConfig::new("people"));
    let tbl = repo.get_table("people").await.unwrap();
    let email_field = key_id(&tbl, "email").await;

    // Insert a row so the unique index has at least one posting.
    tbl.insert(&record_with_str(email_field, "alice@example.com"))
        .await
        .unwrap();

    // Build the unique index synchronously.
    tbl.create_unique_index("email_uniq", &["email"])
        .await
        .unwrap();

    // Sanity: the unique index exists and the barrier is raised.
    assert!(
        tbl.index_manager_ref().has_unique_indexes(),
        "sanity: unique index barrier must be raised before the drop"
    );

    // Install the DROP pause hook (shared by regular + unique DROP paths).
    let hook = Arc::new(BackfillPauseHook::new());
    tbl.index_manager_ref()
        .set_drop_index_pause_hook(Some(Arc::clone(&hook)));

    // Spawn the DROP.
    let tbl_c = tbl.clone();
    let drop_task = tokio::spawn(async move { tbl_c.drop_unique_index("email_uniq").await });

    // Rendezvous: the DROP is parked — definition retired, postings not yet
    // swept.
    hook.wait_until_parked().await;

    // THE PROOF: the definition must already be gone AND the barrier must
    // already be lowered, BEFORE the posting sweep runs.
    assert!(
        !tbl.index_manager_ref().has_unique_indexes(),
        "F-76: the unique-index write-barrier bit must be cleared BEFORE \
         the posting sweep — a writer must not try to maintain an index \
         whose postings are mid-sweep"
    );
    let email_id = key_id(&tbl, "email_uniq").await;
    assert!(
        !tbl.index_manager_ref().unique_index_exists(email_id),
        "F-76: the unique index definition must be retired from the registry \
         BEFORE the posting sweep"
    );

    // Release the DROP; it finishes the sweep and persists.
    hook.release();
    drop_task
        .await
        .unwrap()
        .expect("drop_unique_index must complete once released");

    // Post-drop: still gone.
    assert!(
        !tbl.index_manager_ref().has_unique_indexes(),
        "after the drop completes, the barrier must remain lowered"
    );
}
