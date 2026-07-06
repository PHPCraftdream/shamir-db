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
use shamir_tx::IsolationLevel;
use shamir_types::core::interner::{InternerKey, TouchInd};
use shamir_types::types::common::{new_map, new_map_wc};
use shamir_types::types::value::InnerValue;

use crate::query::filter::eval_context::FilterContext;
use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::TableConfig;
use crate::table::TableManager;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
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

/// Collect the `tag` field from each returned record, in result order.
fn tags_in_order(result: &crate::query::read::QueryResult) -> Vec<String> {
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
fn embeddings_in_order(result: &crate::query::read::QueryResult) -> Vec<Vec<f32>> {
    use shamir_types::types::value::QueryValue;
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
async fn bare_vector_similarity_still_works() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("vecs"));
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
async fn setup_two_cluster(n_per_cluster: usize) -> (RepoInstance, TableManager) {
    let repo = make_repo();
    repo.add_table(TableConfig::new("vecs"));
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

/// Minimal PCG-style PRNG for deterministic test data (avoids pulling in
/// the `rand` crate). Not cryptographically secure — just needs to scatter
/// vectors around a centroid.
struct Pcg(u64);
impl Pcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f32) / (u32::MAX as f32)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Correctness: 50% selectivity
// ─────────────────────────────────────────────────────────────────────────────

/// 50% selectivity: tag="red" filters out half the records. The filtered
/// ANN must return only red records, ranked by proximity to the query.
#[tokio::test]
#[serial_test::serial]
async fn filtered_ann_50pct_selectivity_correct() {
    let (_repo, tbl) = setup_two_cluster(200).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query_vec = vec![1.0, 0.01, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let k = 10u32;

    let result = tbl
        .read(
            &ReadQuery::new("vecs").filter(qf::and(vec![
                qf::vector_similarity("embedding", query_vec.clone(), k),
                qf::eq("tag", "red"),
            ])),
            &ctx,
        )
        .await
        .unwrap();

    // Must return exactly k records (200 reds >> k).
    assert_eq!(
        result.records.len(),
        k as usize,
        "50% selectivity: must fill k={k}"
    );
    // All must be "red".
    let tags = tags_in_order(&result);
    assert!(
        tags.iter().all(|t| t == "red"),
        "filtered ANN must return only red records; got {tags:?}"
    );
    // Stats must show the filtered-vector path.
    let label = result
        .stats
        .as_ref()
        .and_then(|s| s.index_used.as_deref())
        .unwrap_or("<none>");
    assert!(
        label.contains("filtered_vector"),
        "expected filtered_vector_scan path, got {label:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Correctness: ~1% selectivity (rare tag)
// ─────────────────────────────────────────────────────────────────────────────

/// ~1% selectivity: only 5 of 505 records are "rare". The oversample-retry
/// loop must widen k′ enough to find them. Query k=3 — the loop must find
/// at least 3 rare records.
#[tokio::test]
#[serial_test::serial]
async fn filtered_ann_low_selectivity_finds_rare() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("vecs"));
    let tbl = repo.get_table("vecs").await.unwrap();
    tbl.create_index_v2(&vector_index_op(8)).await.unwrap();

    let emb_id = field_id(&tbl, "embedding").await;
    let tag_id = field_id(&tbl, "tag").await;

    let mut rng = Pcg(99);
    // 500 "common" records scattered around [1,0,...]
    for _ in 0..500 {
        let mut v = [0.0f32; 8];
        v[0] = 1.0;
        for slot in &mut v[1..] {
            *slot = (rng.next() - 0.5) * 0.05;
        }
        tbl.insert(&vec_record(emb_id, &v, tag_id, "common"))
            .await
            .unwrap();
    }
    // 5 "rare" records scattered around [1,0,...] but tagged "rare"
    for _ in 0..5 {
        let mut v = [0.0f32; 8];
        v[0] = 1.0;
        for slot in &mut v[1..] {
            *slot = (rng.next() - 0.5) * 0.05;
        }
        tbl.insert(&vec_record(emb_id, &v, tag_id, "rare"))
            .await
            .unwrap();
    }

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query_vec = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let k = 3u32;

    let result = tbl
        .read(
            &ReadQuery::new("vecs").filter(qf::and(vec![
                qf::vector_similarity("embedding", query_vec.clone(), k),
                qf::eq("tag", "rare"),
            ])),
            &ctx,
        )
        .await
        .unwrap();

    // With 5 rare records and k=3, we must get 3.
    assert_eq!(
        result.records.len(),
        k as usize,
        "1% selectivity: must find 3 of 5 rare records; got {}",
        result.records.len()
    );
    let tags = tags_in_order(&result);
    assert!(
        tags.iter().all(|t| t == "rare"),
        "must return only rare records; got {tags:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Empty result without infinite retry
// ─────────────────────────────────────────────────────────────────────────────

/// A predicate matching NOTHING must terminate and return empty. The retry
/// loop must NOT spin forever — it widens k′ to MAX_TOPK, gets no survivors,
/// and returns empty.
#[tokio::test]
#[serial_test::serial]
async fn filtered_ann_no_match_terminates_with_empty() {
    let (_repo, tbl) = setup_two_cluster(50).await;

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query_vec = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let k = 5u32;

    // tag="nonexistent" matches nothing.
    let result = tbl
        .read(
            &ReadQuery::new("vecs").filter(qf::and(vec![
                qf::vector_similarity("embedding", query_vec.clone(), k),
                qf::eq("tag", "nonexistent"),
            ])),
            &ctx,
        )
        .await
        .unwrap();

    assert_eq!(
        result.records.len(),
        0,
        "no-match predicate must return empty, not hang"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Oversample monotonicity (statistical)
// ─────────────────────────────────────────────────────────────────────────────

/// A larger oversample must yield ≥ as many valid candidates. With a
/// selective predicate (rare tag), oversample=1× might miss some rare
/// records on the first ANN pass, while oversample=10× widens the net
/// enough to catch them in one shot. We assert the high-oversample query
/// returns ≥ the low-oversample count, and fills k.
#[tokio::test]
#[serial_test::serial]
async fn oversample_higher_yields_at_least_as_many() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("vecs"));
    let tbl = repo.get_table("vecs").await.unwrap();
    tbl.create_index_v2(&vector_index_op(8)).await.unwrap();

    let emb_id = field_id(&tbl, "embedding").await;
    let tag_id = field_id(&tbl, "tag").await;

    let mut rng = Pcg(7);
    // 300 common + 6 rare, all scattered around the same query point so ANN
    // ordering interleaves them.
    for _ in 0..300 {
        let mut v = [0.0f32; 8];
        v[0] = 1.0;
        for slot in &mut v[1..] {
            *slot = (rng.next() - 0.5) * 0.05;
        }
        tbl.insert(&vec_record(emb_id, &v, tag_id, "common"))
            .await
            .unwrap();
    }
    for _ in 0..6 {
        let mut v = [0.0f32; 8];
        v[0] = 1.0;
        for slot in &mut v[1..] {
            *slot = (rng.next() - 0.5) * 0.05;
        }
        tbl.insert(&vec_record(emb_id, &v, tag_id, "rare"))
            .await
            .unwrap();
    }

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query_vec = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let k = 4u32;

    // Low oversample (1×) — tight net, may miss rare records initially.
    let result_low = tbl
        .read(
            &ReadQuery::new("vecs").filter(qf::and(vec![
                qf::vector_similarity_opts("embedding", query_vec.clone(), k, None, Some(1.0)),
                qf::eq("tag", "rare"),
            ])),
            &ctx,
        )
        .await
        .unwrap();

    // High oversample — wide net, should catch more.
    let result_high = tbl
        .read(
            &ReadQuery::new("vecs").filter(qf::and(vec![
                qf::vector_similarity_opts("embedding", query_vec.clone(), k, None, Some(10.0)),
                qf::eq("tag", "rare"),
            ])),
            &ctx,
        )
        .await
        .unwrap();

    // Both capped at k, but high-oversample must fill k if there are ≥k rare.
    assert!(
        result_high.records.len() >= result_low.records.len(),
        "higher oversample must yield ≥ candidates: low={}, high={}",
        result_low.records.len(),
        result_high.records.len()
    );
    // With 6 rare and k=4, high oversample must fill k.
    assert_eq!(
        result_high.records.len(),
        k as usize,
        "high oversample must fill k={k} from 6 rare records"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// tx-staged + filter
// ─────────────────────────────────────────────────────────────────────────────

/// An in-tx query with a filter must see its own staged vectors AND apply
/// the residual predicate. Stage a "red" vector in-tx, query for
/// `And([VectorSimilarity, Eq(tag,"red")])` — the staged record must
/// appear. Also stage a "blue" vector and confirm the red filter excludes it.
#[tokio::test]
#[serial_test::serial]
async fn tx_staged_vector_visible_and_filtered() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("vecs"));
    let tbl = repo.get_table("vecs").await.unwrap();
    tbl.create_index_v2(&vector_index_op(2)).await.unwrap();

    let emb_id = field_id(&tbl, "embedding").await;
    let tag_id = field_id(&tbl, "tag").await;

    // Commit one "red" baseline record.
    tbl.insert(&vec_record(emb_id, &[1.0, 0.0], tag_id, "red"))
        .await
        .unwrap();

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    // Stage a "red" vector (near query) in-tx.
    let _red_rid = tbl
        .insert_tx(
            &vec_record(emb_id, &[0.95, 0.05], tag_id, "red"),
            Some(&mut tx),
        )
        .await
        .unwrap();

    // Stage a "blue" vector (near query too) in-tx — must be filtered out.
    let _blue_rid = tbl
        .insert_tx(
            &vec_record(emb_id, &[0.94, 0.06], tag_id, "blue"),
            Some(&mut tx),
        )
        .await
        .unwrap();

    // Query in-tx: filtered ANN for red.
    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query = ReadQuery::new("vecs").filter(qf::and(vec![
        qf::vector_similarity("embedding", vec![1.0, 0.0], 10),
        qf::eq("tag", "red"),
    ]));

    let result = tbl.read_tx(&query, &ctx, Some(&tx)).await.unwrap();
    let tags = tags_in_order(&result);

    // Must see the committed red AND the staged red, but NOT the staged blue.
    assert!(
        tags.iter().all(|t| t == "red"),
        "filtered ANN in-tx must return only red; got {tags:?}"
    );
    // 1 committed + 1 staged red = 2.
    assert!(
        tags.len() >= 2,
        "must see committed + staged red records; got {} tags: {tags:?}",
        tags.len()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// C1 regression: partial index coverage must NOT skip unindexed predicates
// ─────────────────────────────────────────────────────────────────────────────

/// Build a record `{embedding: [f32...], tag: &str, price: f64}`.
fn vec_record_with_price(
    emb_key: u64,
    vec: &[f32],
    tag_key: u64,
    tag: &str,
    price_key: u64,
    price: f64,
) -> InnerValue {
    let mut m = new_map_wc(3);
    m.insert(
        InternerKey::new(emb_key),
        InnerValue::List(vec.iter().map(|f| InnerValue::F64(*f as f64)).collect()),
    );
    m.insert(InternerKey::new(tag_key), InnerValue::Str(tag.into()));
    m.insert(InternerKey::new(price_key), InnerValue::F64(price));
    InnerValue::Map(m)
}

/// C1 regression: a filtered-ANN query with `And(VectorSimilarity, Eq(tag),
/// Gt(price, 100))` where only `tag` is indexed must NOT return records with
/// price <= 100. Before the fix, the fast-path (pre-filter/co-filter) would
/// use the btree index on `tag` to get candidate RIDs but silently skip the
/// unindexed `Gt(price, 100)` predicate, returning wrong results.
#[tokio::test]
#[serial_test::serial]
async fn c1_partial_index_coverage_enforces_unindexed_predicate() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("vecs"));
    let tbl = repo.get_table("vecs").await.unwrap();
    tbl.create_index_v2(&vector_index_op(4)).await.unwrap();
    // Legacy btree index on "tag" only — "price" is NOT indexed.
    tbl.create_index("tag_idx", &["tag"]).await.unwrap();

    let emb_id = field_id(&tbl, "embedding").await;
    let tag_id = field_id(&tbl, "tag").await;
    let price_id = field_id(&tbl, "price").await;

    // Insert records: all tag="electronics", varying prices.
    // Vectors are all near [1,0,0,0] so they all rank high.
    let base_vec = [1.0f32, 0.0, 0.0, 0.0];
    let mut rng = Pcg(7777);
    for i in 0..20u32 {
        let mut v = base_vec;
        for slot in &mut v[1..] {
            *slot = (rng.next() - 0.5) * 0.02;
        }
        // 10 records with price=50 (should be EXCLUDED by price>100)
        // 10 records with price=200 (should PASS)
        let price = if i < 10 { 50.0 } else { 200.0 };
        tbl.insert(&vec_record_with_price(
            emb_id,
            &v,
            tag_id,
            "electronics",
            price_id,
            price,
        ))
        .await
        .unwrap();
    }

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    // Query: top-5 closest AND tag="electronics" AND price > 100.
    // The btree on "tag" covers the Eq but NOT the Gt(price, 100).
    let k = 5u32;
    let result = tbl
        .read(
            &ReadQuery::new("vecs").filter(qf::and(vec![
                qf::vector_similarity("embedding", vec![1.0, 0.0, 0.0, 0.0], k),
                qf::eq("tag", "electronics"),
                qf::gt("price", 100.0f64),
            ])),
            &ctx,
        )
        .await
        .unwrap();

    // All returned records MUST have price > 100.
    use shamir_types::types::value::QueryValue;
    for rec in &result.records {
        let price_val = match rec.get_value("price") {
            Some(QueryValue::F64(v)) => *v,
            Some(QueryValue::Int(v)) => *v as f64,
            other => panic!("price field must be numeric; got {other:?}"),
        };
        assert!(
            price_val > 100.0,
            "C1 regression: returned record with price={price_val} which violates \
             the unindexed predicate price>100"
        );
    }
    // Should return 5 results (there are 10 records with price=200).
    assert_eq!(
        result.records.len(),
        k as usize,
        "should find k results matching all predicates"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// VR-5 (Б-5) — read-your-own-writes on pre/co-filter paths
//
// `tx_staged_vector_visible_and_filtered` above only exercises the POST-FILTER
// path (no secondary index on the residual field, so the planner cannot
// populate `candidate_rids` and falls through to the V3.1 oversample-retry
// loop). The three tests below force the PRE-FILTER and CO-FILTER paths by
// adding a legacy btree index on the residual field (`tag`), then assert the
// in-tx staged vectors are visible AND the residual still filters them.
// ─────────────────────────────────────────────────────────────────────────────

/// Assert that a filtered-ANN query took the given path by inspecting
/// `stats.index_used`. Returns the label for chaining.
fn path_label(result: &crate::query::read::QueryResult) -> &str {
    result
        .stats
        .as_ref()
        .and_then(|s| s.index_used.as_deref())
        .unwrap_or("<none>")
}

/// Build `count` committed records with the given `tag`, each near the query
/// point `[1.0, 0.0]` (dim=2) so they all rank near the top of ANN search.
async fn commit_cluster(tbl: &TableManager, emb_id: u64, tag_id: u64, tag: &str, count: usize) {
    // VR-5 tests query [1.0, 0.0] and stage a vector at [0.99, 0.01] to prove
    // read-your-own-writes. Committed rows here MUST be farther from the
    // query than the staged vector, or `count` distance-0 exact-match ties
    // would fill every top-k slot and the staged (non-zero-distance) vector
    // could never surface — even with a fully correct merge implementation.
    // [0.0, 1.0] (orthogonal, L2 distance ≈1.414) guarantees the staged
    // [0.99, 0.01] (distance ≈0.014) always ranks strictly closer.
    for _ in 0..count {
        tbl.insert(&vec_record(emb_id, &[0.0, 1.0], tag_id, tag))
            .await
            .unwrap();
    }
}

/// **PRE-FILTER path** (candidate set ≤ `PRE_FILTER_MAX_CANDIDATES`): a legacy
/// btree index on `tag` resolves `eq(tag,"red")` to a small committed RID set
/// (under 4096), so the planner selects `search_prefilter`. A staged "red"
/// record must appear, and a staged "blue" record must be filtered out —
/// matching the post-filter path's read-your-own-writes contract.
#[tokio::test]
#[serial_test::serial]
async fn vr5_prefilter_sees_staged_and_filters_residual() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("vecs"));
    let tbl = repo.get_table("vecs").await.unwrap();
    tbl.create_index_v2(&vector_index_op(2)).await.unwrap();
    // Legacy btree on `tag` so `try_plan_index_scan` resolves `eq(tag, ...)`.
    tbl.create_index("tag_idx", &["tag"]).await.unwrap();

    let emb_id = field_id(&tbl, "embedding").await;
    let tag_id = field_id(&tbl, "tag").await;

    // 100 committed "red" records (well under PRE_FILTER_MAX_CANDIDATES=4096).
    commit_cluster(&tbl, emb_id, tag_id, "red", 100).await;

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    // Stage a "red" record very close to the query (should rank high & pass
    // the residual).
    let _staged_red = tbl
        .insert_tx(
            &vec_record(emb_id, &[0.99, 0.01], tag_id, "red"),
            Some(&mut tx),
        )
        .await
        .unwrap();

    // Stage a "blue" record also very close to the query — must be filtered
    // OUT by the residual `eq(tag,"red")`.
    let _staged_blue = tbl
        .insert_tx(
            &vec_record(emb_id, &[0.985, 0.015], tag_id, "blue"),
            Some(&mut tx),
        )
        .await
        .unwrap();

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query = ReadQuery::new("vecs").filter(qf::and(vec![
        qf::vector_similarity("embedding", vec![1.0, 0.0], 5),
        qf::eq("tag", "red"),
    ]));

    let result = tbl.read_tx(&query, &ctx, Some(&tx)).await.unwrap();

    // Confirm the pre-filter path was taken.
    let label = path_label(&result);
    assert_eq!(
        label, "pre_filter",
        "expected pre_filter path, got {label:?}"
    );

    // All returned records must be "red" (residual enforced on committed AND
    // staged). The staged blue must NOT appear.
    let tags = tags_in_order(&result);
    assert!(
        tags.iter().all(|t| t == "red"),
        "pre_filter must return only red (staged blue filtered); got {tags:?}"
    );
    // VR-5 — the SPECIFIC staged vector [0.99, 0.01] must be present: every
    // committed row is at [1.0, 0.0] (from `commit_cluster`), so this exact
    // coordinate can ONLY have come from the staged merge. A "some red row is
    // present" assertion alone would pass even with a no-op merge (the 100
    // committed reds already satisfy it) — this is the actual regression
    // guard for read-your-own-writes on the pre-filter path.
    let embeddings = embeddings_in_order(&result);
    assert!(
        embeddings
            .iter()
            .any(|e| (e[0] - 0.99).abs() < 1e-4 && (e[1] - 0.01).abs() < 1e-4),
        "pre_filter must include the staged red vector [0.99, 0.01] \
         (read-your-own-writes); got embeddings {embeddings:?}"
    );
    // The staged blue's distinctive coordinates must NOT appear (residual
    // correctly excludes it, not merely "committed blues are absent").
    assert!(
        !embeddings
            .iter()
            .any(|e| (e[0] - 0.985).abs() < 1e-4 && (e[1] - 0.015).abs() < 1e-4),
        "pre_filter must NOT include the staged blue vector [0.985, 0.015] \
         (residual must filter staged rows, not just tag-label them); got \
         embeddings {embeddings:?}"
    );
}

/// **CO-FILTER path** (`n_candidates > PRE_FILTER_MAX_CANDIDATES` and
/// `selectivity ≤ CO_FILTER_MAX_SELECTIVITY`): build a table large enough that
/// `eq(tag,"red")` yields > 4096 candidates but the red fraction is ≤ 20% of
/// the live set, so the planner selects `search_cofilter`. A staged "red"
/// record must appear, a staged "blue" must be filtered out.
#[tokio::test]
#[serial_test::serial]
async fn vr5_cofilter_sees_staged_and_filters_residual() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("vecs"));
    let tbl = repo.get_table("vecs").await.unwrap();
    tbl.create_index_v2(&vector_index_op(2)).await.unwrap();
    tbl.create_index("tag_idx", &["tag"]).await.unwrap();

    let emb_id = field_id(&tbl, "embedding").await;
    let tag_id = field_id(&tbl, "tag").await;

    // 5000 committed "red" records — just above PRE_FILTER_MAX_CANDIDATES
    // (4096) so pre-filter is skipped. dim=2 keeps insertion fast.
    commit_cluster(&tbl, emb_id, tag_id, "red", 5000).await;
    // 16000 committed "blue" records so selectivity = 5000/21000 ≈ 0.238...
    // — wait, that's > 0.20. We need ≤ 0.20, so push blue up to ≥ 20000
    // (total_live ≥ 25000 → selectivity ≤ 0.20).
    commit_cluster(&tbl, emb_id, tag_id, "blue", 20_000).await;

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    // Stage a "red" record near the query.
    let _staged_red = tbl
        .insert_tx(
            &vec_record(emb_id, &[0.99, 0.01], tag_id, "red"),
            Some(&mut tx),
        )
        .await
        .unwrap();
    // Stage a "blue" record near the query — must be filtered out.
    let _staged_blue = tbl
        .insert_tx(
            &vec_record(emb_id, &[0.985, 0.015], tag_id, "blue"),
            Some(&mut tx),
        )
        .await
        .unwrap();

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query = ReadQuery::new("vecs").filter(qf::and(vec![
        qf::vector_similarity("embedding", vec![1.0, 0.0], 5),
        qf::eq("tag", "red"),
    ]));

    let result = tbl.read_tx(&query, &ctx, Some(&tx)).await.unwrap();

    let label = path_label(&result);
    assert_eq!(label, "co_filter", "expected co_filter path, got {label:?}");

    let tags = tags_in_order(&result);
    assert!(
        tags.iter().all(|t| t == "red"),
        "co_filter must return only red (staged blue filtered); got {tags:?}"
    );
    // VR-5 — same specific-coordinate check as the pre-filter test: every
    // committed row is at [0.0, 1.0] (distance ≈1.414 from the query), so
    // only the staged merge can produce [0.99, 0.01] (distance ≈0.014).
    let embeddings = embeddings_in_order(&result);
    assert!(
        embeddings
            .iter()
            .any(|e| (e[0] - 0.99).abs() < 1e-4 && (e[1] - 0.01).abs() < 1e-4),
        "co_filter must include the staged red vector [0.99, 0.01] \
         (read-your-own-writes); got embeddings {embeddings:?}"
    );
    assert!(
        !embeddings
            .iter()
            .any(|e| (e[0] - 0.985).abs() < 1e-4 && (e[1] - 0.015).abs() < 1e-4),
        "co_filter must NOT include the staged blue vector [0.985, 0.015] \
         (residual must filter staged rows); got embeddings {embeddings:?}"
    );
}

/// **POST-FILTER path** regression: same shape as the pre/co tests but WITHOUT
/// a secondary index on `tag`, so the planner cannot populate `candidate_rids`
/// and falls through to the V3.1 oversample-retry post-filter loop. The staged
/// "red" must appear, the staged "blue" must be filtered out. This documents
/// the path the bug-report flagged as already-correct and guards it against
/// regressions while the pre/co paths are fixed.
#[tokio::test]
#[serial_test::serial]
async fn vr5_postfilter_sees_staged_and_filters_residual() {
    let repo = make_repo();
    repo.add_table(TableConfig::new("vecs"));
    let tbl = repo.get_table("vecs").await.unwrap();
    // Vector index only — NO btree on `tag`, so no candidate_rids → post-filter.
    tbl.create_index_v2(&vector_index_op(2)).await.unwrap();

    let emb_id = field_id(&tbl, "embedding").await;
    let tag_id = field_id(&tbl, "tag").await;

    commit_cluster(&tbl, emb_id, tag_id, "red", 50).await;

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    let _staged_red = tbl
        .insert_tx(
            &vec_record(emb_id, &[0.99, 0.01], tag_id, "red"),
            Some(&mut tx),
        )
        .await
        .unwrap();
    let _staged_blue = tbl
        .insert_tx(
            &vec_record(emb_id, &[0.985, 0.015], tag_id, "blue"),
            Some(&mut tx),
        )
        .await
        .unwrap();

    let interner = tbl.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);

    let query = ReadQuery::new("vecs").filter(qf::and(vec![
        qf::vector_similarity("embedding", vec![1.0, 0.0], 5),
        qf::eq("tag", "red"),
    ]));

    let result = tbl.read_tx(&query, &ctx, Some(&tx)).await.unwrap();

    let label = path_label(&result);
    assert!(
        label.contains("filtered_vector"),
        "expected filtered_vector_scan (post-filter) path, got {label:?}"
    );

    let tags = tags_in_order(&result);
    assert!(
        tags.iter().all(|t| t == "red"),
        "post_filter must return only red (staged blue filtered); got {tags:?}"
    );
    // VR-5 — same specific-coordinate check as pre/co-filter above.
    let embeddings = embeddings_in_order(&result);
    assert!(
        embeddings
            .iter()
            .any(|e| (e[0] - 0.99).abs() < 1e-4 && (e[1] - 0.01).abs() < 1e-4),
        "post_filter must include the staged red vector [0.99, 0.01] \
         (read-your-own-writes); got embeddings {embeddings:?}"
    );
    assert!(
        !embeddings
            .iter()
            .any(|e| (e[0] - 0.985).abs() < 1e-4 && (e[1] - 0.015).abs() < 1e-4),
        "post_filter must NOT include the staged blue vector [0.985, 0.015] \
         (residual must filter staged rows); got embeddings {embeddings:?}"
    );
}
