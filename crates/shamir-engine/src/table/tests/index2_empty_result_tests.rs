//! CRIT-7 regression: an empty index2 result (FTS / functional / vector) is
//! the authoritative "zero rows match" answer and must short-circuit the
//! query — it must NOT fall through to the legacy btree / full-scan paths.
//!
//! The dangerous case is `Filter::VectorSimilarity`: it compiles to
//! `FilterNode::True` (`query/filter/compile.rs`), so a fall-through to the
//! full-scan path evaluates `true` for EVERY row and returns the entire table
//! instead of zero rows. The FTS / functional case is "only" a perf regression
//! (the full scan re-derives the same empty answer at O(N) cost) but is the
//! same root bug.
//!
//! These tests assert the index2 fast-path returns an empty `QueryResult`
//! (with `records_returned == 0`) instead of falling through.

use std::sync::Arc;

use shamir_query_builder::filter as qf;
use shamir_query_types::admin::CreateIndexOp;
use shamir_query_types::read::ReadQuery;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::{new_map, new_map_wc};
use shamir_types::types::value::InnerValue;

use crate::query::filter::eval_context::FilterContext;
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;
use crate::table::TableManager;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers (mirrors filtered_ann_tests.rs)
// ─────────────────────────────────────────────────────────────────────────────

fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

fn vector_index_op(dim: u32) -> CreateIndexOp {
    CreateIndexOp {
        create_index: "vec_idx".into(),
        table: "vecs".into(),
        fields: vec![vec!["embedding".into()]],
        unique: false,
        sorted: false,
        repo: "main".into(),
        index_type: Some("vector".into()),
        fts_tokenizer: None,
        fts_language: None,
        functional_op: None,
        functional_args: None,
        vector_dim: Some(dim),
        vector_metric: Some("cosine".into()),
        vector_quantization: None,
        include: Vec::new(),
        if_not_exists: false,
    }
}

async fn field_id(tbl: &TableManager, name: &str) -> u64 {
    let interner = tbl.interner().get().await.unwrap();
    match interner.touch_ind(name).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

/// Build a record `{embedding: [f32...], tag: &str}`.
fn vec_record(emb_key: u64, vec: &[f32], tag_key: u64, tag: &str) -> InnerValue {
    let mut m = new_map_wc(2);
    m.insert(
        InternerKey::new(emb_key),
        InnerValue::List(vec.iter().map(|f| InnerValue::F64(*f as f64)).collect()),
    );
    m.insert(InternerKey::new(tag_key), InnerValue::Str(tag.into()));
    InnerValue::Map(m)
}

// ─────────────────────────────────────────────────────────────────────────────
// CRIT-7: empty VectorSimilarity via index2 must return zero rows
// ─────────────────────────────────────────────────────────────────────────────

/// A bare `VectorSimilarity` query against a populated vector index2, where
/// the index lookup matches ZERO rows (querying an EMPTY table — no vectors
/// indexed — so `try_plan_index2` returns `Some(IndexResult::Ranked(vec![]))`).
///
/// Before the fix: the empty `rids_vec` caused fall-through past the index2
/// block; the bare `VectorSimilarity` compiles to `FilterNode::True`, so the
/// full-scan path returned EVERY row in the table — but here there are no
/// rows either, so we additionally seed the table with one row and assert
/// the empty result is returned (not the seeded row).
///
/// Concretely: seed 1 row, query with `k=0` which the index resolves to an
/// empty ranked set. The query MUST return 0 records, not the 1 seeded row.
#[tokio::test]
#[serial_test::serial]
async fn empty_vector_similarity_returns_zero_rows() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("vecs"));
    let tbl = repo.get_table("vecs").await.unwrap();
    tbl.create_index_v2(&vector_index_op(2)).await.unwrap();

    let emb_id = field_id(&tbl, "embedding").await;
    let tag_id = field_id(&tbl, "tag").await;

    // Seed one row. The full-scan fall-through bug would return THIS row.
    tbl.insert(&vec_record(emb_id, &[1.0, 0.0], tag_id, "a"))
        .await
        .unwrap();

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // k=0 → index2 vector lookup returns an empty Ranked set (top-0 = none).
    // try_plan_index2 yields Some(IndexResult::Ranked(vec![])).
    let query =
        ReadQuery::new("vecs").filter(qf::vector_similarity("embedding", vec![1.0, 0.0], 0));

    let result = tbl.read(&query, &ctx).await.unwrap();

    // CRIT-7: must be zero rows, NOT the seeded row (which the full-scan
    // fall-through would return because VectorSimilarity compiles to True).
    assert_eq!(
        result.records.len(),
        0,
        "CRIT-7: empty index2 result must short-circuit; got {} records (full-scan fall-through bug)",
        result.records.len(),
    );
    let stats = result
        .stats
        .as_ref()
        .expect("stats must be present on the index2 path");
    assert_eq!(
        stats.records_returned, 0,
        "CRIT-7: records_returned must be 0 for an empty index2 result",
    );
    assert!(
        stats
            .index_used
            .as_deref()
            .is_some_and(|t| t.starts_with("index2")),
        "CRIT-7: empty result must still be attributed to the index2 path (got {:?})",
        stats.index_used,
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// CRIT-7 (FTS side): empty FTS match via index2 must return zero rows
// ─────────────────────────────────────────────────────────────────────────────

/// A `Filter::Fts` query against a populated FTS index2 where the lookup
/// matches ZERO rows (the query term does not appear in any indexed document).
///
/// Before the fix: the empty `rids_vec` fell through to a full scan that
/// re-evaluated `FilterNode::FtsMatch` against every row. The result was
/// still empty (FTS re-derives the same answer) but at O(N) needless cost
/// and with the wrong `index_used` attribution. This test pins the perf +
/// attribution side of the same root bug.
#[tokio::test]
#[serial_test::serial]
async fn empty_fts_match_returns_zero_rows() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("docs"));
    let tbl = repo.get_table("docs").await.unwrap();
    tbl.create_index_v2(&CreateIndexOp {
        create_index: "body_fts".into(),
        table: "docs".into(),
        fields: vec![vec!["body".into()]],
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
    })
    .await
    .unwrap();

    let body_id = field_id(&tbl, "body").await;

    // Seed two documents whose tokens will not match the query term.
    let mut m = new_map_wc(1);
    m.insert(
        InternerKey::new(body_id),
        InnerValue::Str("hello world".into()),
    );
    tbl.insert(&InnerValue::Map(m)).await.unwrap();
    let mut m = new_map_wc(1);
    m.insert(
        InternerKey::new(body_id),
        InnerValue::Str("foo bar baz".into()),
    );
    tbl.insert(&InnerValue::Map(m)).await.unwrap();

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Query for a term that does not appear in any document.
    let query = ReadQuery::new("docs").filter(qf::fts("body", "missingterm", "and"));

    let result = tbl.read(&query, &ctx).await.unwrap();

    assert_eq!(
        result.records.len(),
        0,
        "CRIT-7: empty FTS index2 result must short-circuit; got {} records",
        result.records.len(),
    );
    let stats = result
        .stats
        .as_ref()
        .expect("stats must be present on the index2 path");
    assert_eq!(
        stats.records_returned, 0,
        "CRIT-7: records_returned must be 0 for an empty FTS index2 result",
    );
    assert!(
        stats
            .index_used
            .as_deref()
            .is_some_and(|t| t.starts_with("index2")),
        "CRIT-7: empty FTS result must still be attributed to the index2 path (got {:?})",
        stats.index_used,
    );
}
