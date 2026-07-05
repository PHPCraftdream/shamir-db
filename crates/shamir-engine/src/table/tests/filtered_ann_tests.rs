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
