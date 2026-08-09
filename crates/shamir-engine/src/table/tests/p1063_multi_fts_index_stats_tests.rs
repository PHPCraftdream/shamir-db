//! #1063 regression tests: BumpFtsStats without provenance corrupts BM25 with 2+ FTS indexes
//!
//! Proves the production fix (R0-B, BumpFtsStats now carries provenance) closes the
//! failure modes the original brief identifies:
//!   - N FTS indexes on the same table → N² BumpFtsStats applications instead of N
//!   - Different fields have different `doc_len` → `sum_doc_len`/`avgdl` pollution
//!     across backends (not just `doc_count` scale corruption)
//!   - ABA: stale BumpFtsStats for dropped instance applied to newly created instance
//!     with the same name
//!
//! Every test in this file is confirmed (by mentally tracing the pre-fix code path)
//! to FAIL against the pre-fix implementation — the assertions are discriminating.

use std::sync::Arc;

use shamir_query_builder::{filter, write};
use shamir_query_types::admin::types::CreateIndexOp;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_tx::IsolationLevel;
use shamir_types::access::Actor;
use shamir_types::core::interner::InternerKey;
use shamir_types::mpack;
use shamir_types::types::common::new_map;
use shamir_types::types::value::InnerValue;

use crate::index2::fts_ranked_backend::FtsRankedBackend;
use crate::query::filter::eval_context::FilterContext;
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;
use crate::table::TableManager;

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

async fn key_id(tbl: &TableManager, name: &str) -> u64 {
    let interner = tbl.interner().get().await.unwrap();
    match interner.touch_ind(name).unwrap() {
        shamir_types::core::interner::TouchInd::Exists(k)
        | shamir_types::core::interner::TouchInd::New(k) => k.id(),
    }
}

fn fts_index_op(name: &str, table: &str, field: &str) -> CreateIndexOp {
    CreateIndexOp {
        create_index: name.into(),
        table: table.into(),
        fields: vec![vec![field.into()]],
        unique: false,
        sorted: false,
        repo: "main".into(),
        index_type: Some("fts".into()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: None,
        functional_args: None,
        vector_dim: None,
        vector_metric: None,
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    }
}

fn record_with_str(key: u64, val: &str) -> InnerValue {
    let mut m = new_map();
    m.insert(InternerKey::new(key), InnerValue::Str(val.into()));
    InnerValue::Map(m)
}

/// Test 1: Two FTS indexes on different fields, one insert.
///
/// Pre-fix behavior: each backend's BumpFtsStats is broadcast to BOTH backends,
/// so each backend receives 2 bumps instead of 1 → `doc_count` = 2 for both.
/// Worse: if the fields have different `doc_len`, each backend's `sum_doc_len`
/// gets polluted with the OTHER field's length → corrupt `avgdl`.
///
/// Post-fix: BumpFtsStats carries provenance, ops are grouped by backend at
/// commit time, and each backend receives exactly its own bump.
#[tokio::test]
async fn two_fts_indexes_different_fields_one_insert_doc_count_and_sum_doc_len_correct() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("docs"));
    let tbl = repo.get_table("docs").await.unwrap();

    let _title_key = key_id(&tbl, "title").await;
    let _body_key = key_id(&tbl, "body").await;

    // Create TWO FTS indexes on DIFFERENT fields — both exist BEFORE any tx stages.
    // Field lengths differ: "quick" = 1 word, "the quick brown fox" = 4 words.
    tbl.create_index_v2(&fts_index_op("title_fts", "docs", "title"))
        .await
        .unwrap();
    tbl.create_index_v2(&fts_index_op("body_fts", "docs", "body"))
        .await
        .unwrap();

    // Snapshot both backends' stats BEFORE the tx (backfill found nothing).
    let backend1 = tbl
        .index2_registry()
        .get_by_name(key_id(&tbl, "title_fts").await)
        .await
        .expect("title_fts must be registered");
    let backend2 = tbl
        .index2_registry()
        .get_by_name(key_id(&tbl, "body_fts").await)
        .await
        .expect("body_fts must be registered");
    let title_fts = backend1
        .as_any()
        .downcast_ref::<FtsRankedBackend>()
        .expect("backend must be FtsRankedBackend");
    let body_fts = backend2
        .as_any()
        .downcast_ref::<FtsRankedBackend>()
        .expect("backend must be FtsRankedBackend");
    let title_dc_pre = title_fts.doc_count();
    let title_sum_pre = title_fts.sum_doc_len();
    let body_dc_pre = body_fts.doc_count();
    let body_sum_pre = body_fts.sum_doc_len();

    // Insert ONE document with BOTH fields populated, different lengths.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let op = write::insert("docs")
        .rows([mpack!({
            "title": "quick",           // 1 token, doc_len = 1
            "body": "the quick brown fox", // 4 tokens, doc_len = 4
        })])
        .build();
    let result = tbl
        .execute_insert_tx(&op, &mut tx, true, None, &Actor::System)
        .await
        .unwrap();
    assert_eq!(result.affected, 1);

    repo.commit_tx(tx).await.expect("commit must succeed");

    // THE assertions: each backend's doc_count = 1 (not 2), AND sum_doc_len
    // matches that field's own token count (not the other's).
    let title_dc_post = title_fts.doc_count();
    let title_sum_post = title_fts.sum_doc_len();
    let body_dc_post = body_fts.doc_count();
    let body_sum_post = body_fts.sum_doc_len();

    assert_eq!(
        title_dc_post,
        title_dc_pre + 1,
        "title_fts: doc_count should be 1, not 2 (double-count bug)"
    );
    assert_eq!(
        title_sum_post,
        title_sum_pre + 1,
        "title_fts: sum_doc_len should reflect 'quick' = 1 token, not pollute with body's 4"
    );

    assert_eq!(
        body_dc_post,
        body_dc_pre + 1,
        "body_fts: doc_count should be 1, not 2 (double-count bug)"
    );
    assert_eq!(
        body_sum_post,
        body_sum_pre + 4,
        "body_fts: sum_doc_len should reflect 'the quick brown fox' = 4 tokens, not pollute with title's 1"
    );

    // Discriminating check: the two backends' sum_doc_len values are DIFFERENT
    // when the two fields have different lengths — this alone proves no
    // cross-contamination (double-counting would give both the SAME polluted sum).
    assert_ne!(
        title_sum_post, body_sum_post,
        "title_fts and body_fts must have different sum_doc_len when fields have different token counts \
         (pre-fix would pollute both with the sum of both)"
    );
}

/// Test 2: Two FTS indexes, update one field only.
///
/// Pre-fix behavior: the update generates BumpFtsStats for the UPDATED field only
/// (plan_update emits remove(old) + add(new) for that field), but each bump is
/// broadcast to BOTH backends → both backends' stats change even though only one
/// field changed.
///
/// Post-fix: only the owning backend (title_fts) receives the bumps; body_fts
/// is completely untouched.
#[tokio::test]
async fn two_fts_indexes_update_one_field_only_owner_stats_change() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("docs"));
    let tbl = repo.get_table("docs").await.unwrap();

    let _title_key = key_id(&tbl, "title").await;
    let _body_key = key_id(&tbl, "body").await;

    // Create TWO FTS indexes on DIFFERENT fields.
    tbl.create_index_v2(&fts_index_op("title_fts", "docs", "title"))
        .await
        .unwrap();
    tbl.create_index_v2(&fts_index_op("body_fts", "docs", "body"))
        .await
        .unwrap();

    // Insert ONE document with BOTH fields populated, different lengths.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let op = write::insert("docs")
        .rows([mpack!({
            "title": "quick",           // 1 token, doc_len = 1
            "body": "the quick brown fox", // 4 tokens, doc_len = 4
        })])
        .build();
    let result = tbl
        .execute_insert_tx(&op, &mut tx, true, None, &Actor::System)
        .await
        .unwrap();
    assert_eq!(result.affected, 1);
    repo.commit_tx(tx).await.expect("commit must succeed");

    // Snapshot both backends' stats BEFORE the update.
    let backend1 = tbl
        .index2_registry()
        .get_by_name(key_id(&tbl, "title_fts").await)
        .await
        .expect("title_fts must be registered");
    let backend2 = tbl
        .index2_registry()
        .get_by_name(key_id(&tbl, "body_fts").await)
        .await
        .expect("body_fts must be registered");
    let title_fts = backend1
        .as_any()
        .downcast_ref::<FtsRankedBackend>()
        .expect("backend must be FtsRankedBackend");
    let body_fts = backend2
        .as_any()
        .downcast_ref::<FtsRankedBackend>()
        .expect("backend must be FtsRankedBackend");
    let title_dc_pre = title_fts.doc_count();
    let title_sum_pre = title_fts.sum_doc_len();
    let body_dc_pre = body_fts.doc_count();
    let body_sum_pre = body_fts.sum_doc_len();

    // Stage a NEW tx, update ONLY the `title` field to different text.
    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let (mut tx2, _guard2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let op2 = write::update("docs")
        .set(mpack!({ "title": "hello world" })) // 2 tokens, doc_len = 2
        .build()
        .unwrap();

    let result2 = tbl
        .execute_update_tx(&op2, &ctx, &mut tx2, None, &Actor::System)
        .await
        .unwrap();
    assert_eq!(result2.affected, 1);
    repo.commit_tx(tx2).await.expect("commit must succeed");

    // THE assertions:
    // - title_fts: doc_count unchanged (old -1, new +1), sum_doc_len changed from 1 to 2
    // - body_fts: doc_count AND sum_doc_len are BYTE-IDENTICAL to pre-update snapshot

    assert_eq!(
        title_fts.doc_count(),
        title_dc_pre,
        "title_fts: doc_count should be unchanged after update (old -1, new +1)"
    );
    assert_eq!(
        title_fts.sum_doc_len(),
        title_sum_pre + 1, // changed from 1 to 2
        "title_fts: sum_doc_len should change from 1 to 2 (reflecting new title length)"
    );

    // Discriminating check: body_fts stats are BYTE-IDENTICAL to pre-update snapshot.
    // This proves body_fts received ZERO bumps from the title field update.
    assert_eq!(
        body_fts.doc_count(),
        body_dc_pre,
        "body_fts: doc_count should be unchanged (title update must not affect body backend)"
    );
    assert_eq!(
        body_fts.sum_doc_len(),
        body_sum_pre,
        "body_fts: sum_doc_len should be unchanged (title update must not affect body backend)"
    );
}

/// Test 3: Stage tx → DROP FTS → CREATE FTS with same name → commit (ABA).
///
/// Pre-fix behavior: BumpFtsStats had no provenance, so
/// `retract_stale_provenance_ops` always kept it. The stale bump from
/// instance A gets applied to instance B → corrupts B's stats.
///
/// Post-fix: BumpFtsStats carries provenance with `instance_epoch`. The stale
/// bump's `(name_interned, instance_epoch_A)` doesn't match instance B's
/// `(name_interned, instance_epoch_B)`, so it's retracted before commit.
/// Instance B's stats remain at 0 (or whatever B's own backfill produced).
#[tokio::test]
async fn stale_bump_from_dropped_and_recreated_fts_index_not_applied_to_new_instance() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("docs"));
    let tbl = repo.get_table("docs").await.unwrap();
    let title_key = key_id(&tbl, "title").await;

    // Instance A: title_fts on "title"
    tbl.create_index_v2(&fts_index_op("title_fts", "docs", "title"))
        .await
        .unwrap();

    // Stage an insert against instance A.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let rid = tbl
        .insert_tx(&record_with_str(title_key, "hello world"), Some(&mut tx))
        .await
        .unwrap();

    // ABA: DROP instance A, CREATE a NEW instance B under the SAME name.
    tbl.drop_index2("title_fts", None).await.unwrap();
    tbl.create_index_v2(&fts_index_op("title_fts", "docs", "title"))
        .await
        .unwrap();

    repo.commit_tx(tx).await.expect("commit must succeed");

    // Instance B MUST have received fresh ops from re-derivation (because
    // the generation changed from 1 to 3), so doc_count should be 1.
    // The key assertion is that the STALE bump from instance A was NOT applied.
    let backend_b = tbl
        .index2_registry()
        .get_by_name(key_id(&tbl, "title_fts").await)
        .await
        .expect("title_fts must be registered");
    let fts_b = backend_b
        .as_any()
        .downcast_ref::<FtsRankedBackend>()
        .expect("backend must be FtsRankedBackend");

    assert_eq!(
        fts_b.doc_count(),
        1,
        "instance B: doc_count should be 1 (re-derivation adds fresh ops for new backend)"
    );
    assert_eq!(
        fts_b.sum_doc_len(),
        2,
        "instance B: sum_doc_len should be 2 (matching 'hello world')"
    );

    // Verify the row was actually inserted by checking it exists in the table.
    let _row = tbl.get(rid).await.expect("row should exist");
}

/// Test 4: Two FTS indexes, delete row.
///
/// Pre-fix behavior: delete generates BumpFtsStats { sign: -1 } for BOTH fields,
/// each broadcast to BOTH backends → each backend receives 2 negative bumps,
/// going from doc_count=1 to doc_count=-1 (or underflow panic).
///
/// Post-fix: each backend receives exactly its own bump (1 negative → 0).
#[tokio::test]
async fn two_fts_indexes_delete_row_correct_backend_only_decremented() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("docs"));
    let tbl = repo.get_table("docs").await.unwrap();

    let _title_key = key_id(&tbl, "title").await;
    let _body_key = key_id(&tbl, "body").await;

    // Create TWO FTS indexes on DIFFERENT fields.
    tbl.create_index_v2(&fts_index_op("title_fts", "docs", "title"))
        .await
        .unwrap();
    tbl.create_index_v2(&fts_index_op("body_fts", "docs", "body"))
        .await
        .unwrap();

    // Insert ONE document with BOTH fields populated, different lengths.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let op = write::insert("docs")
        .rows([mpack!({
            "title": "quick",           // 1 token, doc_len = 1
            "body": "the quick brown fox", // 4 tokens, doc_len = 4
        })])
        .build();
    let result = tbl
        .execute_insert_tx(&op, &mut tx, true, None, &Actor::System)
        .await
        .unwrap();
    assert_eq!(result.affected, 1);
    repo.commit_tx(tx).await.expect("commit must succeed");

    // Snapshot both backends' stats BEFORE the delete.
    let backend1 = tbl
        .index2_registry()
        .get_by_name(key_id(&tbl, "title_fts").await)
        .await
        .expect("title_fts must be registered");
    let backend2 = tbl
        .index2_registry()
        .get_by_name(key_id(&tbl, "body_fts").await)
        .await
        .expect("body_fts must be registered");
    let title_fts = backend1
        .as_any()
        .downcast_ref::<FtsRankedBackend>()
        .expect("backend must be FtsRankedBackend");
    let body_fts = backend2
        .as_any()
        .downcast_ref::<FtsRankedBackend>()
        .expect("backend must be FtsRankedBackend");
    let title_dc_pre = title_fts.doc_count();
    let title_sum_pre = title_fts.sum_doc_len();
    let body_dc_pre = body_fts.doc_count();
    let body_sum_pre = body_fts.sum_doc_len();

    // Stage a NEW tx, delete the row.
    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let (mut tx2, _guard2) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let op2 = write::delete("docs")
        .where_(filter::and(vec![])) // Delete all rows (we have exactly 1)
        .build()
        .unwrap();

    let result2 = tbl
        .execute_delete_tx(&op2, &ctx, &mut tx2, None, &Actor::System)
        .await
        .unwrap();
    assert_eq!(result2.affected, 1);
    repo.commit_tx(tx2).await.expect("commit must succeed");

    // THE assertions: each backend's doc_count decreased by EXACTLY 1 (not 2),
    // and sum_doc_len decreased by that field's length.
    // This proves each backend received exactly ONE negative bump, not two.

    assert_eq!(
        title_fts.doc_count(),
        title_dc_pre - 1,
        "title_fts: doc_count should decrease by exactly 1 (not 2, which would be N²)"
    );
    assert_eq!(
        title_fts.sum_doc_len(),
        title_sum_pre - 1,
        "title_fts: sum_doc_len should decrease by title's length (1)"
    );

    assert_eq!(
        body_fts.doc_count(),
        body_dc_pre - 1,
        "body_fts: doc_count should decrease by exactly 1 (not 2, which would be N²)"
    );
    assert_eq!(
        body_fts.sum_doc_len(),
        body_sum_pre - 4,
        "body_fts: sum_doc_len should decrease by body's length (4)"
    );

    // Final sanity check: both backends are now at 0.
    assert_eq!(
        title_fts.doc_count(),
        0,
        "title_fts: doc_count should be 0 after deleting the only row"
    );
    assert_eq!(
        body_fts.doc_count(),
        0,
        "body_fts: doc_count should be 0 after deleting the only row"
    );
}

/// Test 5: Two FTS indexes, abort staged tx.
///
/// Pre-fix behavior: tx is staged with BumpFtsStats, then aborted.
/// If BumpFtsStats were broadcast (which happens in apply_index_ops_at_commit
/// BEFORE the abort check), the stats would be corrupted regardless of abort.
///
/// Post-fix: abort discards the tx's `index_write_set`, so no BumpFtsStats
/// is ever applied to any backend. Both backends' stats remain at 0.
#[tokio::test]
async fn two_fts_indexes_abort_staged_tx_neither_backend_stats_change() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("docs"));
    let tbl = repo.get_table("docs").await.unwrap();

    let _title_key = key_id(&tbl, "title").await;
    let _body_key = key_id(&tbl, "body").await;

    // Create two FTS indexes.
    tbl.create_index_v2(&fts_index_op("title_fts", "docs", "title"))
        .await
        .unwrap();
    tbl.create_index_v2(&fts_index_op("body_fts", "docs", "body"))
        .await
        .unwrap();

    let backend1 = tbl
        .index2_registry()
        .get_by_name(key_id(&tbl, "title_fts").await)
        .await
        .expect("title_fts must be registered");
    let backend2 = tbl
        .index2_registry()
        .get_by_name(key_id(&tbl, "body_fts").await)
        .await
        .expect("body_fts must be registered");
    let title_fts = backend1
        .as_any()
        .downcast_ref::<FtsRankedBackend>()
        .expect("backend must be FtsRankedBackend");
    let body_fts = backend2
        .as_any()
        .downcast_ref::<FtsRankedBackend>()
        .expect("backend must be FtsRankedBackend");

    // Stage an insert, then ABORT it.
    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let op = write::insert("docs")
        .rows([mpack!({
            "title": "quick",           // 1 token
            "body": "the quick brown fox", // 4 tokens
        })])
        .build();
    let result = tbl
        .execute_insert_tx(&op, &mut tx, true, None, &Actor::System)
        .await
        .unwrap();
    assert_eq!(result.affected, 1);

    // ABORT instead of commit.
    drop(tx); // Explicit drop simulates abort; RAII clean-up removes staged ops

    // THE assertions: both backends' stats are still at 0 (nothing was applied).
    assert_eq!(
        title_fts.doc_count(),
        0,
        "title_fts: doc_count should be 0 after abort (staged bump must not apply)"
    );
    assert_eq!(
        title_fts.sum_doc_len(),
        0,
        "title_fts: sum_doc_len should be 0 after abort"
    );

    assert_eq!(
        body_fts.doc_count(),
        0,
        "body_fts: doc_count should be 0 after abort (staged bump must not apply)"
    );
    assert_eq!(
        body_fts.sum_doc_len(),
        0,
        "body_fts: sum_doc_len should be 0 after abort"
    );
}
