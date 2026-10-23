use crate::query::filter::eval_context::FilterContext;
use crate::table::TableConfig;
use shamir_query_builder::filter as qf;
use shamir_query_types::read::ReadQuery;
use shamir_tx::IsolationLevel;
use shamir_types::types::common::new_map;

use crate::table::tests::read::vector::filtered_ann::shared::filtered_ann_tests::*;

use super::prefilter_cofilter::{commit_cluster, path_label};
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
    repo.add_table_for_test(TableConfig::new("vecs"));
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

    // Snapshot transactions rank exactly over snapshot + staged overlay (B04b):
    // no ANN strategy label; the result semantics below are the contract.
    let label = path_label(&result);
    assert_eq!(
        label, "<none>",
        "snapshot vector read is exact, got {label:?}"
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
