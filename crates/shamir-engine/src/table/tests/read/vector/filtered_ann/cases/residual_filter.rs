use shamir_query_builder::filter as qf;
use shamir_query_types::read::ReadQuery;
use shamir_tx::IsolationLevel;
use shamir_types::types::common::{new_map};
use crate::query::filter::eval_context::FilterContext;
use crate::table::TableConfig;


use crate::table::tests::read::vector::filtered_ann::fixtures::filtered_ann_tests::*;

use super::selectivity::Pcg;
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
///
/// # Determinism (audit G4 / #528)
///
/// The subject under test is the ENGINE-level oversample-retry loop
/// (`read_filtered_vector_scan`: request `k′ = k × oversample` candidates,
/// post-filter by the residual, widen `k′` and retry when short). That loop
/// is orthogonal to whether the backend answers via the exact brute-force
/// scan or the approximate HNSW graph.
///
/// An earlier version scattered 306 vectors — above `BRUTE_FORCE_MAX` (256),
/// so the query ran on `hnsw_rs`'s approximate graph. `hnsw_rs` 0.3.x assigns
/// node layers from an **unseedable** internal RNG (see the `BRUTE_FORCE_MAX`
/// doc-comment in `hnsw_adapter.rs`), so recall — and thus which of the 6
/// rare records surface, and in what order — varied build-to-build regardless
/// of this test's `Pcg` data seed. That made both the `high >= low` inequality
/// (two independent approximate runs could legitimately tie or invert) and the
/// `high == k` fill (a rare record could be graph-unreachable) flaky under
/// load. The test's own comment ("may miss rare records initially") was in
/// fact describing that inherent ANN-approximation noise.
///
/// The fix keeps the dataset at or below `BRUTE_FORCE_MAX` (206 ≤ 256) so the
/// backend answers via the EXACT deterministic brute-force scan. The
/// oversample-retry loop is still fully exercised (low oversample starts with
/// a tight `k′` and must widen to reach the rare records; high oversample
/// starts wide), but its result no longer rides on an unseedable RNG — so
/// `high >= low` and `high == k` hold deterministically. This removes the
/// source of non-determinism rather than retrying-until-lucky or loosening the
/// assertion to hide it.
#[tokio::test]
#[serial_test::serial]
async fn oversample_higher_yields_at_least_as_many() {
    let repo = make_repo();
    repo.add_table_for_test(TableConfig::new("vecs"));
    let tbl = repo.get_table("vecs").await.unwrap();
    tbl.create_index_v2(&vector_index_op(8)).await.unwrap();

    let emb_id = field_id(&tbl, "embedding").await;
    let tag_id = field_id(&tbl, "tag").await;

    let mut rng = Pcg(7);
    // 200 common + 6 rare = 206 total (≤ BRUTE_FORCE_MAX = 256), all scattered
    // around the same query point so distance ranking interleaves them and the
    // oversample-retry loop must widen k′ to surface the rare records. Staying
    // on the exact brute-force path keeps the ranking deterministic (no
    // unseedable HNSW layer RNG).
    for _ in 0..200 {
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
    repo.add_table_for_test(TableConfig::new("vecs"));
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
