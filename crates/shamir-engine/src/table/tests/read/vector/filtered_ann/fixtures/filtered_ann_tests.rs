//! V3.1 / P3 leaf 3.1 — filtered ANN: post-filter with adaptive oversample.
//!
//! Tests cover:
//! - **correctness** — filtered top-k matches brute-force ground truth at
//!   multiple selectivities (50%, ~1%) via an auxiliary `tag` field.
//! - **empty-without-infinite-retry** — a predicate that matches nothing must
//!   terminate (retry loop widens k′ to MAX_TOPK then returns empty).
//! - **oversample-monotonicity** — a larger oversample yields ≥ as many
//!   valid candidates (statistical; tested on a dataset large enough for
//!   the HNSW path).
//! - **tx-staged + filter** — an in-tx query sees its own staged vectors AND
//!   applies the residual predicate.
//! - **back-compat** — a bare `VectorSimilarity` (no `And`) still works
//!   through the pre-existing index2 fast path.
//!
//! Queries are built through the query builder (`shamir_query_builder::filter`)
//! per CLAUDE.md, except for serde round-trip edge cases (not needed here).

use std::sync::Arc;

use shamir_query_builder::filter as qf;
use shamir_query_types::admin::CreateIndexOp;
use shamir_query_types::read::ReadQuery;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::{new_map, new_map_wc};
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::filter::eval_context::FilterContext;
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;
use crate::table::TableManager;
use super::super::cases::selectivity::Pcg;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

pub(crate) fn make_repo() -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    RepoInstance::new("test".into(), BoxRepo::InMemory(repo), Vec::new())
}

pub(crate) fn vector_index_op(dim: u32) -> CreateIndexOp {
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

pub(crate) async fn field_id(tbl: &TableManager, name: &str) -> u64 {
    let interner = tbl.interner().get().await.unwrap();
    match interner.touch_ind(name).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

/// Build a record `{embedding: [f32...], tag: &str}`.
pub(crate) fn vec_record(emb_key: u64, vec: &[f32], tag_key: u64, tag: &str) -> InnerValue {
    let mut m = new_map_wc(2);
    m.insert(
        InternerKey::new(emb_key),
        InnerValue::List(vec.iter().map(|f| InnerValue::F64(*f as f64)).collect()),
    );
    m.insert(InternerKey::new(tag_key), InnerValue::Str(tag.into()));
    InnerValue::Map(m)
}

/// Collect the `tag` field from each returned record, in result order.
pub(crate) fn tags_in_order(result: &crate::query::read::QueryResult) -> Vec<String> {
    result
        .records
        .iter()
        .filter_map(|r| r.get_value_str("tag").map(str::to_owned))
        .collect()
}

/// VR-5 test helper: extract the `embedding` field of each result record as
/// `Vec<f32>`. Used to prove a SPECIFIC staged vector (identified by its
/// exact, distinctive coordinates) is present in the result — `tags_in_order`
/// alone cannot distinguish a staged row from an indistinguishable committed
/// row with the same tag, which would make a "some red row is present"
/// assertion pass even if the staged merge were a no-op.
pub(crate) fn embeddings_in_order(result: &crate::query::read::QueryResult) -> Vec<Vec<f32>> {
    result
        .records
        .iter()
        .filter_map(|r| match r.get_value("embedding") {
            Some(QueryValue::List(items)) => Some(
                items
                    .iter()
                    .map(|v| match v {
                        QueryValue::F64(f) => *f as f32,
                        QueryValue::Int(i) => *i as f32,
                        other => panic!("embedding component must be numeric; got {other:?}"),
                    })
                    .collect(),
            ),
            _ => None,
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// Back-compat: bare VectorSimilarity (no And) still works
// ─────────────────────────────────────────────────────────────────────────────

/// A bare `VectorSimilarity` (NOT wrapped in `And`) must still take the
/// pre-existing index2 fast path and return ranked results.
#[tokio::test]
#[serial_test::serial]
pub(crate) async fn bare_vector_similarity_still_works() {
    let repo = make_repo();
    repo.add_table_for_test(TableConfig::new("vecs"));
    let tbl = repo.get_table("vecs").await.unwrap();
    tbl.create_index_v2(&vector_index_op(2)).await.unwrap();

    let emb_id = field_id(&tbl, "embedding").await;
    let tag_id = field_id(&tbl, "tag").await;

    // Insert 3 records. [1,0] and [0.9,0.1] are near query [1,0]; [0,1] is far.
    tbl.insert(&vec_record(emb_id, &[1.0, 0.0], tag_id, "a"))
        .await
        .unwrap();
    tbl.insert(&vec_record(emb_id, &[0.0, 1.0], tag_id, "b"))
        .await
        .unwrap();
    tbl.insert(&vec_record(emb_id, &[0.9, 0.1], tag_id, "c"))
        .await
        .unwrap();

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query =
        ReadQuery::new("vecs").filter(qf::vector_similarity("embedding", vec![1.0, 0.0], 2));

    let result = tbl.read(&query, &ctx).await.unwrap();
    assert_eq!(result.records.len(), 2, "must return top-2");
    let tags = tags_in_order(&result);
    assert!(
        tags.contains(&"a".to_string()) && tags.contains(&"c".to_string()),
        "top-2 must be the two nearest ([1,0] and [0.9,0.1]); got {tags:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Setup helper: two clusters
// ─────────────────────────────────────────────────────────────────────────────

/// Build N records per cluster with dim=8 vectors: "red" near [1,0,...] and
/// "blue" near [0,1,...]. The `tag` field distinguishes them. A filtered
/// query can select one cluster. Uses a simple PRNG to scatter each vector
/// around its centroid so the HNSW graph is well-connected (identical
/// vectors degenerate the graph).
pub(crate) async fn setup_two_cluster(n_per_cluster: usize) -> (RepoInstance, TableManager) {
    let repo = make_repo();
    repo.add_table_for_test(TableConfig::new("vecs"));
    let tbl = repo.get_table("vecs").await.unwrap();
    tbl.create_index_v2(&vector_index_op(8)).await.unwrap();

    let emb_id = field_id(&tbl, "embedding").await;
    let tag_id = field_id(&tbl, "tag").await;

    let mut rng = Pcg(42);
    for _ in 0..n_per_cluster {
        // Red cluster: [1, noise...] with gaussian-ish jitter.
        let mut v = [0.0f32; 8];
        v[0] = 1.0;
        for slot in &mut v[1..] {
            *slot = (rng.next() - 0.5) * 0.05;
        }
        tbl.insert(&vec_record(emb_id, &v, tag_id, "red"))
            .await
            .unwrap();
    }
    for _ in 0..n_per_cluster {
        // Blue cluster: [0, 1, noise...]
        let mut v = [0.0f32; 8];
        v[1] = 1.0;
        for (j, slot) in v.iter_mut().enumerate() {
            if j != 1 {
                *slot = (rng.next() - 0.5) * 0.05;
            }
        }
        tbl.insert(&vec_record(emb_id, &v, tag_id, "blue"))
            .await
            .unwrap();
    }
    (repo, tbl)
}
