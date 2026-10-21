use crate::index2::vector::hnsw_adapter::{CO_FILTER_MAX_SELECTIVITY, PRE_FILTER_MAX_CANDIDATES};
use crate::query::filter::eval_context::FilterContext;
use crate::table::TableConfig;
use shamir_query_builder::filter as qf;
use shamir_query_types::read::ReadQuery;
use shamir_tx::IsolationLevel;
use shamir_types::types::common::new_map;

use crate::table::tests::read::vector::filtered_ann::fixtures::filtered_ann_tests::*;

use super::prefilter_cofilter::{commit_cluster, path_label};
/// Contract pin: speeding up VR-5 must not change production planner defaults.
/// This guard builds no vector index and is independent of the test override.
#[test]
fn vr5_cofilter_production_thresholds_unchanged() {
    assert_eq!(PRE_FILTER_MAX_CANDIDATES, 4096);
    assert_eq!(CO_FILTER_MAX_SELECTIVITY, 0.20);
}

/// **CO-FILTER path**: lower this manager's planner thresholds, not the
/// production defaults, so 200 committed rows suffice. Pessimistic isolation
/// uses the live ANN planner; Snapshot/Serializable bypass it for exact MVCC
/// ranking. A staged "red" must appear and a staged "blue" must be filtered
/// out, regardless of approximate recall among tied committed vectors.
#[tokio::test]
#[serial_test::serial]
async fn vr5_cofilter_sees_staged_and_filters_residual() {
    let repo = make_repo();
    repo.add_table_for_test(TableConfig::new("vecs"));
    let mut tbl = repo.get_table("vecs").await.unwrap();
    assert_eq!(tbl.filtered_ann_thresholds, None);
    tbl.filtered_ann_thresholds = Some((16, 0.15));
    // A separately obtained manager keeps defaults: no async/global leakage.
    assert_eq!(
        repo.get_table("vecs")
            .await
            .unwrap()
            .filtered_ann_thresholds,
        None,
    );
    assert_eq!(tbl.clone().filtered_ann_thresholds, Some((16, 0.15)));
    tbl.create_index_v2(&vector_index_op(2)).await.unwrap();
    tbl.create_index("tag_idx", &["tag"]).await.unwrap();

    let emb_id = field_id(&tbl, "embedding").await;
    let tag_id = field_id(&tbl, "tag").await;

    // 24 > 16 candidates and 24/200 = 0.12 < 0.15 selectivity, with
    // headroom at both thresholds. Only the planner choice is overridden;
    // search_cofilter and the residual-aware staged merge run unchanged.
    // The oracle does not depend on HNSW recall/order among committed ties:
    // the unique staged red is strictly closer than every committed vector.
    commit_cluster(&tbl, emb_id, tag_id, "red", 24).await;
    commit_cluster(&tbl, emb_id, tag_id, "blue", 176).await;

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Pessimistic).await.unwrap();

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

    // This must reach the live co-filter branch, not exact snapshot ranking.
    let label = path_label(&result);
    assert_eq!(label, "co_filter", "expected co-filter, got {label:?}");

    let tags = tags_in_order(&result);
    assert_eq!(tags.len(), result.records.len(), "every result needs a tag");
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
