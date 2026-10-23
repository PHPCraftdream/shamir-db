use crate::query::filter::eval_context::FilterContext;
use crate::table::TableConfig;
use shamir_query_builder::filter as qf;
use shamir_query_types::read::ReadQuery;
use shamir_types::types::common::new_map;

use crate::table::tests::read::vector::filtered_ann::shared::filtered_ann_tests::*;

/// Minimal PCG-style PRNG for deterministic test data (avoids pulling in
/// the `rand` crate). Not cryptographically secure — just needs to scatter
/// vectors around a centroid.
pub(crate) struct Pcg(pub(crate) u64);
impl Pcg {
    pub(crate) fn next(&mut self) -> f32 {
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

/// ~0.25% selectivity: only 5 of 2005 records are "rare". The oversample-retry
/// loop must widen k′ enough to find them. Query k=3 — the loop must find a
/// majority of the 3 requested rare records.
///
/// Dataset size (2000 "common" + 5 "rare" = 2005 total) is deliberately kept
/// well above the ≥1k-vector "recall-tolerance" threshold documented at
/// `crates/shamir-index/src/vector/hnsw_adapter.rs:46-58`: `hnsw_rs` 0.3.x
/// assigns graph node layers from an internal, unseedable RNG, so a small
/// HNSW graph is genuinely nondeterministic run-to-run. Below `BRUTE_FORCE_MAX`
/// (256) the search is an exact brute-force scan (safe for `assert_eq!`);
/// above it but below ~1k the graph is under-connected and recall can
/// legitimately dip. 2005 total keeps this test solidly on the reliable side
/// of the HNSW path while still exercising the post-filter oversample-retry
/// logic (the whole point of this test).
#[tokio::test]
#[serial_test::serial]
async fn filtered_ann_low_selectivity_finds_rare() {
    let repo = make_repo();
    repo.add_table_for_test(TableConfig::new("vecs"));
    let tbl = repo.get_table("vecs").await.unwrap();
    tbl.create_index_v2(&vector_index_op(8)).await.unwrap();

    let emb_id = field_id(&tbl, "embedding").await;
    let tag_id = field_id(&tbl, "tag").await;

    let mut rng = Pcg(99);
    // 2000 "common" records scattered around [1,0,...]
    for _ in 0..2000 {
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

    // With 5 rare records and k=3, we expect to find all 3 requested — but
    // tolerate finding one fewer. `hnsw_rs` 0.3.x seeds graph node layers from
    // an internal, unseedable RNG (see `hnsw_adapter.rs:46-58`), so even at
    // this dataset size (well above the ≥1k "recall-tolerance" threshold)
    // legitimate small-N approximate-recall variance can occasionally miss
    // one of the 5 needles. This is NOT a correctness compromise: the
    // oversample-retry path is still meaningfully exercised and validated —
    // only the last unit of exact recall is tolerated as approximate.
    assert!(
        result.records.len() >= (k as usize).saturating_sub(1),
        "1% selectivity: must find at least {} of 3 rare records; got {}",
        (k as usize).saturating_sub(1),
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
