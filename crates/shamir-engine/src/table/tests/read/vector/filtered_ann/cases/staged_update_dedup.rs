use crate::query::filter::eval_context::FilterContext;
use crate::table::TableConfig;
use shamir_query_builder::filter as qf;
use shamir_query_types::read::ReadQuery;
use shamir_tx::IsolationLevel;
use shamir_types::types::common::new_map;

use crate::table::tests::read::vector::filtered_ann::fixtures::filtered_ann_tests::*;

use super::prefilter_cofilter::{commit_cluster, path_label};
/// VR-5 (@sh adversarial-review finding, second round) — UPDATE-in-tx of an
/// ALREADY-COMMITTED row (same rid) must not produce a DUPLICATE entry in the
/// pre-filter path's result: the row's OLD version is still in the committed
/// HNSW graph (an embedding-value UPDATE does not stage a vector delete —
/// `stage_vector_deletes_on_update` only stages one when the new record has
/// NO embedding at all), while the NEW version is in `tx.staged_vectors`. The
/// merge must deduplicate by RecordId, with the staged (tx-visible) version
/// winning — not double-count the rid.
#[tokio::test]
async fn vr5_prefilter_update_in_tx_no_duplicate_rid() {
    let repo = make_repo();
    repo.add_table_for_test(TableConfig::new("vecs"));
    let tbl = repo.get_table("vecs").await.unwrap();
    tbl.create_index_v2(&vector_index_op(2)).await.unwrap();
    tbl.create_index("tag_idx", &["tag"]).await.unwrap();

    let emb_id = field_id(&tbl, "embedding").await;
    let tag_id = field_id(&tbl, "tag").await;

    // One committed "red" row, far from the query — this is the row we will
    // UPDATE in-tx (same rid, new embedding, same tag).
    let target_rid = tbl
        .insert(&vec_record(emb_id, &[0.0, 1.0], tag_id, "red"))
        .await
        .unwrap();
    // Padding so pre-filter's candidate count stays comfortably under
    // PRE_FILTER_MAX_CANDIDATES.
    commit_cluster(&tbl, emb_id, tag_id, "red", 50).await;

    let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();

    // UPDATE the target row's embedding to a NEW value very close to the
    // query, keeping tag="red" (so it still passes the residual and does
    // NOT stage a vector delete — the committed version stays in the graph).
    tbl.update_tx(
        target_rid,
        &vec_record(emb_id, &[0.99, 0.01], tag_id, "red"),
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
    assert_eq!(
        path_label(&result),
        "<none>",
        "snapshot vector read is exact"
    );

    // The updated row's NEW embedding [0.99, 0.01] is unique in this dataset
    // (every other committed row is at [0.0, 1.0]) — it must appear EXACTLY
    // ONCE. Before the dedup fix, the row's stale committed version (also
    // resolvable via the graph) plus its staged version would both enter
    // `ranked`, though only the staged bytes would ever be projected (
    // `get_many_bytes_tx` always returns the staged value for a staged rid)
    // — so the observable regression is TWO entries with the identical new
    // embedding, not one stale + one fresh. Either way, exactly one entry
    // must appear.
    let embeddings = embeddings_in_order(&result);
    let new_embedding_count = embeddings
        .iter()
        .filter(|e| (e[0] - 0.99).abs() < 1e-4 && (e[1] - 0.01).abs() < 1e-4)
        .count();
    assert_eq!(
        new_embedding_count, 1,
        "the updated row (new embedding [0.99, 0.01]) must appear EXACTLY \
         ONCE in pre_filter results — a duplicate means the stale committed \
         graph entry and the staged entry were BOTH merged for the same \
         rid; got {new_embedding_count} occurrences in {embeddings:?}"
    );
}
