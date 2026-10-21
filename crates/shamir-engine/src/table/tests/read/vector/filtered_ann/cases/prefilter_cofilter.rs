use shamir_query_builder::filter as qf;
use shamir_query_types::read::ReadQuery;
use shamir_tx::IsolationLevel;
use shamir_types::core::interner::{InternerKey};
use shamir_types::types::common::{new_map, new_map_wc};
use shamir_types::types::value::{InnerValue};
use crate::query::filter::eval_context::FilterContext;
use crate::table::TableConfig;
use crate::table::TableManager;


use crate::table::tests::read::vector::filtered_ann::fixtures::filtered_ann_tests::*;

use super::selectivity::Pcg;
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
    repo.add_table_for_test(TableConfig::new("vecs"));
    let tbl = repo.get_table("vecs").await.unwrap();
    tbl.create_index_v2(&vector_index_op(4)).await.unwrap();
    // base_index btree index on "tag" only — "price" is NOT indexed.
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
// adding a base_index btree index on the residual field (`tag`), then assert the
// in-tx staged vectors are visible AND the residual still filters them.
// ─────────────────────────────────────────────────────────────────────────────

/// Assert that a filtered-ANN query took the given path by inspecting
/// `stats.index_used`. Returns the label for chaining.
pub(crate) fn path_label(result: &crate::query::read::QueryResult) -> &str {
    result
        .stats
        .as_ref()
        .and_then(|s| s.index_used.as_deref())
        .unwrap_or("<none>")
}

/// Build `count` committed records with the given `tag`, each near the query
/// point `[1.0, 0.0]` (dim=2) so they all rank near the top of ANN search.
///
/// Uses the bulk `insert_many` path (one store transaction + batch index
/// hooks) instead of `count` sequential `insert` round-trips. The co-filter
/// test lowers only its manager's planner thresholds to keep this fixture
/// small; index2 still inserts each vector through the real backend.
pub(crate) async fn commit_cluster(tbl: &TableManager, emb_id: u64, tag_id: u64, tag: &str, count: usize) {
    // VR-5 tests query [1.0, 0.0] and stage a vector at [0.99, 0.01] to prove
    // read-your-own-writes. Committed rows here MUST be farther from the
    // query than the staged vector, or `count` distance-0 exact-match ties
    // would fill every top-k slot and the staged (non-zero-distance) vector
    // could never surface — even with a fully correct merge implementation.
    // [0.0, 1.0] (orthogonal, L2 distance ≈1.414) guarantees the staged
    // [0.99, 0.01] (distance ≈0.014) always ranks strictly closer.
    let values: Vec<InnerValue> = (0..count)
        .map(|_| vec_record(emb_id, &[0.0, 1.0], tag_id, tag))
        .collect();
    tbl.insert_many(&values).await.unwrap();
}

/// **PRE-FILTER path** (candidate set ≤ `PRE_FILTER_MAX_CANDIDATES`): a base_index
/// btree index on `tag` resolves `eq(tag,"red")` to a small committed RID set
/// (under 4096), so the planner selects `search_prefilter`. A staged "red"
/// record must appear, and a staged "blue" record must be filtered out —
/// matching the post-filter path's read-your-own-writes contract.
#[tokio::test]
#[serial_test::serial]
async fn vr5_prefilter_sees_staged_and_filters_residual() {
    let repo = make_repo();
    repo.add_table_for_test(TableConfig::new("vecs"));
    let tbl = repo.get_table("vecs").await.unwrap();
    tbl.create_index_v2(&vector_index_op(2)).await.unwrap();
    // base_index btree on `tag` so `try_plan_index_scan` resolves `eq(tag, ...)`.
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

    // Snapshot transactions rank exactly over snapshot + staged overlay (B04b):
    // no ANN strategy label; the result semantics below are the contract.
    let label = path_label(&result);
    assert_eq!(
        label, "<none>",
        "snapshot vector read is exact, got {label:?}"
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
