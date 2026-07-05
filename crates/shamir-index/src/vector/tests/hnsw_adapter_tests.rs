use crate::kind::VectorMetric;
use crate::vector::adapter::{SearchOpts, VectorAdapter, VectorError};
use crate::vector::hnsw_adapter::{HnswAdapter, HnswConfig, ShamirDist, MAX_EF_SEARCH};
use hnsw_rs::prelude::Distance;
use shamir_collections::TFxSet;
use shamir_types::types::record_id::RecordId;

fn rid(n: u8) -> RecordId {
    let mut a = [0u8; 16];
    a[15] = n;
    RecordId(a)
}

fn random_vec(dim: usize, seed: u64) -> Vec<f32> {
    let mut v = Vec::with_capacity(dim);
    let mut s = seed;
    for _ in 0..dim {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        v.push(((s >> 33) as f32) / (u32::MAX as f32) - 0.5);
    }
    v
}

#[tokio::test]
async fn basic_cosine_search() {
    let adapter = HnswAdapter::new(
        3,
        VectorMetric::Cosine,
        HnswConfig {
            max_elements: 100,
            ..Default::default()
        },
    );

    adapter.upsert(rid(1), &[1.0, 0.0, 0.0]).await.unwrap();
    adapter.upsert(rid(2), &[0.0, 1.0, 0.0]).await.unwrap();
    adapter.upsert(rid(3), &[0.9, 0.1, 0.0]).await.unwrap();

    let results = adapter
        .search(&[1.0, 0.0, 0.0], 2, SearchOpts::default(), None)
        .await
        .unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].0, rid(1));
    assert!(results[0].1 < 0.01);
}

#[tokio::test]
async fn delete_removes_from_results() {
    // Deletion correctness must not hinge on HNSW recall. On a tiny
    // 2-node graph, soft-deleting the entry point makes search
    // intermittently return 0 results (recall artifact on a
    // degenerate graph — not a soft-delete bug). To assert behaviour
    // deterministically we:
    //   1. build a non-degenerate graph (10 points) so recall over
    //      the survivors is reliable;
    //   2. assert the deleted rid is ABSENT (the actual contract of
    //      delete — never relies on recall reaching one survivor);
    //   3. assert the survivors ARE found (recall sanity on a graph
    //      large enough that it holds).
    // Mirrors the presence/absence pattern of
    // `apply_committed_vectors_inserts_all_into_graph` and
    // `recall_at_10_on_1k_vectors`.
    let adapter = HnswAdapter::new(
        2,
        VectorMetric::L2,
        HnswConfig {
            max_elements: 100,
            ..Default::default()
        },
    );

    // 10 points spread along x so neighbours are well separated.
    for i in 0..10u8 {
        adapter.upsert(rid(i), &[i as f32, 0.0]).await.unwrap();
    }

    adapter.delete(rid(0)).await.unwrap();

    let results = adapter
        .search(&[0.0, 0.0], 10, SearchOpts::default(), None)
        .await
        .unwrap();

    // Contract: the deleted rid must never surface.
    assert!(
        !results.iter().any(|(r, _)| *r == rid(0)),
        "deleted rid must be absent; got {results:?}"
    );
    // Recall sanity on a non-degenerate graph: the surviving nearest
    // neighbour (rid 1 at [1,0]) is found.
    assert!(
        results.iter().any(|(r, _)| *r == rid(1)),
        "surviving nearest neighbour must be found; got {results:?}"
    );
}

#[tokio::test]
async fn upsert_replaces() {
    let adapter = HnswAdapter::new(
        2,
        VectorMetric::L2,
        HnswConfig {
            max_elements: 100,
            ..Default::default()
        },
    );

    adapter.upsert(rid(1), &[0.0, 0.0]).await.unwrap();
    adapter.upsert(rid(1), &[10.0, 10.0]).await.unwrap();

    let results = adapter
        .search(&[10.0, 10.0], 1, SearchOpts::default(), None)
        .await
        .unwrap();
    assert_eq!(results[0].0, rid(1));
    assert!(results[0].1 < 0.01);
}

#[tokio::test]
async fn recall_at_10_on_1k_vectors() {
    // `hnsw_rs` 0.3.4 has no public seed API — `Hnsw::new` calls
    // `StdRng::from_os_rng()` internally, so the graph topology is
    // non-deterministic between runs. To keep this test stable while
    // still exercising HNSW under realistic load we:
    //   1. raise ef_search well above the dataset's natural variance
    //      (a search that visits ~half the graph hits recall 1.0
    //      almost always);
    //   2. require recall ≥ 0.5 — a soft floor that catches gross
    //      regressions (broken Distance impl, broken pruning) without
    //      flaking on the ~5% of runs where the random graph is
    //      adversarial.
    // Tighter recall validation belongs in a separate bench-only run.
    let dim = 32;
    let n = 1000;
    let adapter = HnswAdapter::new(
        dim as u32,
        VectorMetric::L2,
        HnswConfig {
            max_elements: n + 100,
            ef_construction: 400,
            ef_search: 400,
            ..Default::default()
        },
    );

    let mut vecs = Vec::with_capacity(n);
    for i in 0..n {
        let v = random_vec(dim, i as u64 + 42);
        adapter.upsert(rid(0), &v).await.unwrap();
        // Use unique rids:
        let mut a = [0u8; 16];
        a[14] = (i >> 8) as u8;
        a[15] = (i & 0xFF) as u8;
        let r = RecordId(a);
        adapter.upsert(r, &v).await.unwrap();
        vecs.push((r, v));
    }

    // Brute-force ground truth for query = vecs[0].
    let query = &vecs[0].1;
    let mut dists: Vec<(RecordId, f32)> = vecs
        .iter()
        .map(|(r, v)| {
            let d: f32 = query
                .iter()
                .zip(v.iter())
                .map(|(a, b)| (a - b) * (a - b))
                .sum::<f32>()
                .sqrt();
            (*r, d)
        })
        .collect();
    dists.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    let gt_top10: TFxSet<RecordId> = dists.iter().take(10).map(|(r, _)| *r).collect();

    let hnsw_results = adapter
        .search(query, 10, SearchOpts::default(), None)
        .await
        .unwrap();
    let hnsw_top10: TFxSet<RecordId> = hnsw_results.iter().map(|(r, _)| *r).collect();

    let recall = gt_top10.intersection(&hnsw_top10).count() as f64 / 10.0;
    assert!(recall >= 0.5, "recall@10 = {recall:.2} — expected >= 0.50");
}

#[tokio::test]
async fn dim_mismatch_rejected() {
    let adapter = HnswAdapter::new(3, VectorMetric::L2, HnswConfig::default());
    let err = adapter.upsert(rid(1), &[1.0, 2.0]).await.unwrap_err();
    assert!(matches!(
        err,
        VectorError::DimMismatch {
            expected: 3,
            got: 2
        }
    ));
}

#[tokio::test]
async fn search_dim_mismatch_rejected() {
    let adapter = HnswAdapter::new(3, VectorMetric::L2, HnswConfig::default());
    adapter.upsert(rid(1), &[1.0, 2.0, 3.0]).await.unwrap();
    let err = adapter
        .search(&[1.0, 2.0], 1, SearchOpts::default(), None)
        .await
        .unwrap_err();
    assert!(matches!(err, VectorError::DimMismatch { .. }));
}

#[tokio::test]
async fn empty_index_returns_empty() {
    let adapter = HnswAdapter::new(2, VectorMetric::L2, HnswConfig::default());
    let results = adapter
        .search(&[0.0, 0.0], 5, SearchOpts::default(), None)
        .await
        .unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn dot_product_metric_normalized() {
    // Direct test of `ShamirDist` evaluator for the Dot metric — three
    // hand-picked normalized vectors, exact distance values, exact
    // ordering. We sidestep HNSW here because the graph is non-
    // deterministic (no seed API in hnsw_rs 0.3.4) and unstable
    // ordering would force soft assertions even at n=3. The HNSW
    // integration is covered by `basic_cosine_search` and `recall_at_10`.
    let dist = ShamirDist {
        metric: VectorMetric::Dot,
    };
    let s = 2.0_f32.sqrt().recip();
    let q = [1.0_f32, 0.0];

    // dot(q, [1,0]) = 1.0  → dist 0.0
    // dot(q, [s,s]) ≈ 0.707 → dist 0.293
    // dot(q, [0,1]) = 0.0  → dist 1.0
    let d_self = dist.eval(&q, &[1.0, 0.0]);
    let d_diag = dist.eval(&q, &[s, s]);
    let d_orth = dist.eval(&q, &[0.0, 1.0]);

    assert!(d_self < 0.01, "self-similarity should be ~0, got {d_self}");
    assert!(
        (d_diag - (1.0 - s)).abs() < 0.01,
        "diag dist should be ~{}, got {d_diag}",
        1.0 - s
    );
    assert!(
        (d_orth - 1.0).abs() < 0.01,
        "orthogonal dist should be ~1.0, got {d_orth}"
    );
    // Ordering invariant: nearer < farther.
    assert!(d_self < d_diag && d_diag < d_orth);
}

#[tokio::test]
async fn k_larger_than_dataset() {
    let adapter = HnswAdapter::new(2, VectorMetric::L2, HnswConfig::default());
    adapter.upsert(rid(1), &[0.0, 0.0]).await.unwrap();
    adapter.upsert(rid(2), &[1.0, 0.0]).await.unwrap();

    // k > dataset size returns every vector. The index is tiny, so search
    // runs the exact brute-force path (see `BRUTE_FORCE_MAX`) — fully
    // deterministic, no polling needed.
    let results = adapter
        .search(&[0.0, 0.0], 100, SearchOpts::default(), None)
        .await
        .unwrap();
    assert_eq!(results.len(), 2, "search must return both inserted vectors");
}

#[tokio::test]
async fn huge_k_clamped_no_panic() {
    let adapter = HnswAdapter::new(2, VectorMetric::L2, HnswConfig::default());
    adapter.upsert(rid(1), &[0.0, 0.0]).await.unwrap();
    adapter.upsert(rid(2), &[1.0, 0.0]).await.unwrap();
    // k = u32::MAX would previously cause huge allocation
    let results = adapter
        .search(&[0.0, 0.0], u32::MAX, SearchOpts::default(), None)
        .await
        .unwrap();
    assert_eq!(results.len(), 2);
}

#[tokio::test]
async fn k_zero_returns_empty() {
    let adapter = HnswAdapter::new(2, VectorMetric::L2, HnswConfig::default());
    adapter.upsert(rid(1), &[0.0, 0.0]).await.unwrap();
    let results = adapter
        .search(&[0.0, 0.0], 0, SearchOpts::default(), None)
        .await
        .unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn concurrent_searches_lock_free() {
    let dim = 16;
    let adapter = std::sync::Arc::new(HnswAdapter::new(
        dim,
        VectorMetric::Cosine,
        HnswConfig {
            max_elements: 1000,
            ..Default::default()
        },
    ));
    // Populate
    for i in 0..100 {
        let mut a = [0u8; 16];
        a[15] = i as u8;
        adapter
            .upsert(RecordId(a), &random_vec(dim as usize, i as u64))
            .await
            .unwrap();
    }

    // 8 concurrent searches.
    let mut handles = Vec::new();
    for s in 0..8u64 {
        let a = std::sync::Arc::clone(&adapter);
        handles.push(tokio::spawn(async move {
            let q = random_vec(dim as usize, s + 100);
            a.search(&q, 10, SearchOpts::default(), None).await.unwrap()
        }));
    }
    for h in handles {
        let r = h.await.unwrap();
        assert!(!r.is_empty());
    }
}

#[tokio::test]
async fn delete_nonexistent_no_error() {
    let adapter = HnswAdapter::new(2, VectorMetric::L2, HnswConfig::default());
    adapter.delete(rid(99)).await.unwrap();
}

#[tokio::test]
async fn search_merges_staged_slice() {
    // The committed graph holds rid(1); the caller passes its own
    // un-committed vector (rid(2), very close to the query) as a
    // staged slice. `search` must brute-force-merge it so an in-tx
    // query sees both. This is the path the executor drives from
    // `TxContext::staged_vectors_for(token)`.
    let adapter = HnswAdapter::new(
        3,
        VectorMetric::L2,
        HnswConfig {
            max_elements: 100,
            ..Default::default()
        },
    );
    adapter.upsert(rid(1), &[1.0, 0.0, 0.0]).await.unwrap();

    let staged = vec![(rid(2), vec![0.9, 0.1, 0.0])];

    // Without the staged slice: only the committed vector.
    let committed_only = adapter
        .search(&[1.0, 0.0, 0.0], 10, SearchOpts::default(), None)
        .await
        .unwrap();
    let committed_rids: TFxSet<_> = committed_only.iter().map(|(r, _)| *r).collect();
    assert!(committed_rids.contains(&rid(1)));
    assert!(
        !committed_rids.contains(&rid(2)),
        "staged vector must be invisible without the slice"
    );

    // With the staged slice: both surface.
    let merged = adapter
        .search(&[1.0, 0.0, 0.0], 10, SearchOpts::default(), Some(&staged))
        .await
        .unwrap();
    let merged_rids: TFxSet<_> = merged.iter().map(|(r, _)| *r).collect();
    assert!(
        merged_rids.contains(&rid(1)),
        "committed vector still found"
    );
    assert!(merged_rids.contains(&rid(2)), "staged vector merged in");
}

#[tokio::test]
async fn apply_committed_vectors_inserts_all_into_graph() {
    let adapter = HnswAdapter::new(
        3,
        VectorMetric::L2,
        HnswConfig {
            max_elements: 100,
            ..Default::default()
        },
    );

    // Before apply — search finds nothing.
    let before = adapter
        .search(&[1.0, 0.0, 0.0], 10, SearchOpts::default(), None)
        .await
        .unwrap();
    assert_eq!(before.len(), 0, "graph empty before apply");

    // Apply a committed batch (what commit Phase 5d feeds in).
    let batch = vec![
        (rid(1), vec![1.0, 0.0, 0.0]),
        (rid(2), vec![0.0, 1.0, 0.0]),
        (rid(3), vec![0.0, 0.0, 1.0]),
    ];
    adapter.apply_committed_vectors(&batch).await.unwrap();

    // After apply — the closest vector (rid 1 = the query) is findable.
    let after = adapter
        .search(&[1.0, 0.0, 0.0], 10, SearchOpts::default(), None)
        .await
        .unwrap();
    assert!(
        !after.is_empty(),
        "graph must hold committed vectors after apply"
    );
    let labels: TFxSet<_> = after.iter().map(|(r, _)| *r).collect();
    assert!(
        labels.contains(&rid(1)),
        "closest applied vector (rid 1) must be findable; got {after:?}"
    );
}

#[tokio::test]
async fn apply_committed_vectors_empty_is_noop() {
    let adapter = HnswAdapter::new(2, VectorMetric::L2, HnswConfig::default());
    adapter.apply_committed_vectors(&[]).await.unwrap(); // must not panic
}

#[tokio::test]
async fn apply_committed_vectors_handles_replace() {
    let adapter = HnswAdapter::new(
        3,
        VectorMetric::L2,
        HnswConfig {
            max_elements: 100,
            ..Default::default()
        },
    );

    // A committed vector at [1,0,0].
    adapter.upsert(rid(1), &[1.0, 0.0, 0.0]).await.unwrap();
    let before = adapter
        .search(&[1.0, 0.0, 0.0], 1, SearchOpts::default(), None)
        .await
        .unwrap();
    assert_eq!(before[0].0, rid(1));

    // Apply a committed batch that replaces rid(1) with a new vector.
    adapter
        .apply_committed_vectors(&[(rid(1), vec![0.0, 1.0, 0.0])])
        .await
        .unwrap();

    // Search for [0,1,0] -> should find rid(1) (updated position).
    let after = adapter
        .search(&[0.0, 1.0, 0.0], 1, SearchOpts::default(), None)
        .await
        .unwrap();
    assert_eq!(after[0].0, rid(1));
    assert!(after[0].1 < 0.01, "should be very close to [0,1,0]");
}

#[tokio::test]
async fn many_upserts_same_rid() {
    let adapter = HnswAdapter::new(2, VectorMetric::L2, HnswConfig::default());
    for i in 0..10 {
        adapter.upsert(rid(1), &[i as f32, 0.0]).await.unwrap();
    }
    // Only latest visible
    let results = adapter
        .search(&[9.0, 0.0], 10, SearchOpts::default(), None)
        .await
        .unwrap();
    let matching: Vec<_> = results.iter().filter(|(r, _)| *r == rid(1)).collect();
    assert_eq!(
        matching.len(),
        1,
        "rid(1) should appear once after 10 upserts"
    );
    assert!(matching[0].1 < 0.5);
}

/// D12: two concurrent `upsert(SAME rid, different vec)` must leave the
/// rid mapped to EXACTLY ONE live internal — never two. This is the race
/// reachable since III.5 moved HNSW promote outside `commit_lock` (two
/// committers promoting the same record concurrently).
///
/// Against the OLD non-atomic read-tombstone-reassign, both upserts read
/// "no old internal", allocate i1/i2, insert both into the graph, and the
/// loser's internal stays un-tombstoned — so the rid surfaces twice in
/// search and `len()` over-counts. The `entry_async` slot-claim fixes it:
/// the second upsert serialises on the bucket entry, sees the first's
/// internal as "old", and tombstones it.
///
/// We drive many barrier-synced iterations to surface the interleaving,
/// and assert the invariant two ways: (a) directly on adapter state — the
/// count of live (non-tombstoned) internals mapped to the rid is exactly
/// 1, which is recall-independent and the decisive old-vs-new
/// discriminator; and (b) end-to-end via search on a non-degenerate graph
/// — the rid appears at most once.
#[tokio::test]
#[serial_test::serial]
async fn upsert_same_rid_concurrent_no_duplicate() {
    use std::sync::Arc as StdArc;
    use tokio::sync::Barrier;

    let dim = 4usize;
    // Each iteration is a fresh adapter so the race window is clean and
    // any single failure is decisive. Enough iterations to reliably
    // surface the interleaving on the racy code.
    for iter in 0..40u64 {
        let adapter = StdArc::new(HnswAdapter::new(
            dim as u32,
            VectorMetric::L2,
            HnswConfig {
                max_elements: 1000,
                ..Default::default()
            },
        ));

        // Populate a non-degenerate graph with unrelated rids so search
        // recall over survivors is reliable (the same rationale as
        // `delete_removes_from_results`). rid bytes start at 10 so they
        // never collide with the contended rid (1).
        for j in 0..12u8 {
            adapter
                .upsert(rid(10 + j), &random_vec(dim, iter * 100 + j as u64))
                .await
                .unwrap();
        }

        let target = rid(1);
        let vec_a = vec![1.0f32, 0.0, 0.0, 0.0];
        let vec_b = vec![0.0f32, 1.0, 0.0, 0.0];

        // Two tasks race an upsert of the SAME rid, synced at the barrier
        // so both enter the critical section as close together as the
        // runtime allows.
        let barrier = StdArc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for v in [vec_a.clone(), vec_b.clone()] {
            let a = StdArc::clone(&adapter);
            let b = StdArc::clone(&barrier);
            handles.push(tokio::spawn(async move {
                b.wait().await;
                a.upsert(target, &v).await.unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        // (a) Direct state invariant (recall-independent): exactly one
        //     internal mapped to `target` is live (present in rid_map and
        //     NOT tombstoned). The old racy path leaves two.
        let mut live_internals = 0usize;
        adapter
            .rid_map
            .scan_async(|internal, mapped_rid| {
                if *mapped_rid == target && !adapter.deleted.contains(internal) {
                    live_internals += 1;
                }
            })
            .await;
        assert_eq!(
            live_internals, 1,
            "iter {iter}: rid must map to exactly ONE live internal after \
             two concurrent upserts; found {live_internals} (D12 duplicate-rid race)"
        );

        // (b) End-to-end: the rid appears AT MOST once in a top-k search
        //     near either staged vector. (Recall may legitimately miss it
        //     on a tiny random graph, but it must never appear twice.)
        for q in [&vec_a, &vec_b] {
            let results = adapter
                .search(q, 16, SearchOpts::default(), None)
                .await
                .unwrap();
            let occurrences = results.iter().filter(|(r, _)| *r == target).count();
            assert!(
                occurrences <= 1,
                "iter {iter}: rid surfaced {occurrences} times in search — \
                 duplicate live graph node (D12); results={results:?}"
            );
        }
    }
}

#[tokio::test]
async fn non_tx_search_unchanged_after_refactor() {
    // Regression guard: existing non-tx path works exactly as before.
    let adapter = HnswAdapter::new(
        3,
        VectorMetric::Cosine,
        HnswConfig {
            max_elements: 100,
            ..Default::default()
        },
    );
    adapter.upsert(rid(1), &[1.0, 0.0, 0.0]).await.unwrap();
    adapter.upsert(rid(2), &[0.0, 1.0, 0.0]).await.unwrap();
    adapter.upsert(rid(3), &[0.9, 0.1, 0.0]).await.unwrap();

    let results = adapter
        .search(&[1.0, 0.0, 0.0], 2, SearchOpts::default(), None)
        .await
        .unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].0, rid(1));
}

// ============================================================================
// V0.2 — upsert_batch (parallel_insert, batched rebuild)
// ============================================================================

/// Batch of N → `len() == N` AND search surfaces the inserted points.
/// On a small graph (≤ BRUTE_FORCE_MAX) search is exact brute-force so we
/// can assert recall deterministically; a larger batch (300 > 256) forces
/// the HNSW path and we assert recall ≥ a soft floor vs brute-force, mirroring
/// `recall_at_10_on_1k_vectors`.
#[tokio::test]
async fn upsert_batch_inserts_all_and_search_finds_them() {
    let dim = 4usize;

    // (a) Small batch (50 ≤ 256): exact brute-force search path.
    {
        let n = 50usize;
        let adapter = HnswAdapter::new(
            dim as u32,
            VectorMetric::L2,
            HnswConfig {
                max_elements: n + 10,
                ..Default::default()
            },
        );
        let mut batch: Vec<(RecordId, Vec<f32>)> = Vec::with_capacity(n);
        for i in 0..n {
            let mut a = [0u8; 16];
            a[15] = i as u8;
            batch.push((RecordId(a), random_vec(dim, i as u64 + 1)));
        }
        adapter.upsert_batch(&batch).await.unwrap();
        assert_eq!(adapter.len(), n, "len must equal batch size after insert");

        // Exact path: querying the first inserted vector returns it as top-1.
        let q = batch[0].1.clone();
        let results = adapter
            .search(&q, 1, SearchOpts::default(), None)
            .await
            .unwrap();
        assert_eq!(
            results[0].0, batch[0].0,
            "brute-force path must find the exact nearest"
        );
    }

    // (b) Larger batch (300 > 256): HNSW path, recall vs brute-force ≥ 0.5.
    {
        let n = 300usize;
        let adapter = HnswAdapter::new(
            dim as u32,
            VectorMetric::L2,
            HnswConfig {
                max_elements: n + 100,
                ef_construction: 400,
                ef_search: 400,
                ..Default::default()
            },
        );
        let mut batch: Vec<(RecordId, Vec<f32>)> = Vec::with_capacity(n);
        for i in 0..n {
            let mut a = [0u8; 16];
            a[14] = (i >> 8) as u8;
            a[15] = (i & 0xFF) as u8;
            batch.push((RecordId(a), random_vec(dim, i as u64 * 7 + 3)));
        }
        adapter.upsert_batch(&batch).await.unwrap();
        assert_eq!(adapter.len(), n, "len must equal batch size after insert");

        // Recall vs brute-force ground-truth top-10 for a held-out query.
        let query = random_vec(dim, 9999);
        let mut dists: Vec<(RecordId, f32)> = batch
            .iter()
            .map(|(r, v)| {
                let d: f32 = query
                    .iter()
                    .zip(v.iter())
                    .map(|(a, b)| (a - b) * (a - b))
                    .sum::<f32>()
                    .sqrt();
                (*r, d)
            })
            .collect();
        dists.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let gt_top10: TFxSet<RecordId> = dists.iter().take(10).map(|(r, _)| *r).collect();

        let hnsw_results = adapter
            .search(&query, 10, SearchOpts::default(), None)
            .await
            .unwrap();
        let hnsw_top10: TFxSet<RecordId> = hnsw_results.iter().map(|(r, _)| *r).collect();
        let recall = gt_top10.intersection(&hnsw_top10).count() as f64 / 10.0;
        assert!(
            recall >= 0.5,
            "batch-inserted HNSW recall@10 = {recall:.2} — expected >= 0.50"
        );
    }
}

/// Dim mismatch in the middle of a batch → Err AND nothing applied
/// (atomicity of validation). We assert `len()` is unchanged and no rid
/// is findable.
#[tokio::test]
async fn upsert_batch_dim_mismatch_is_atomic() {
    let dim = 3u32;
    let adapter = HnswAdapter::new(dim, VectorMetric::L2, HnswConfig::default());

    // Batch where row index 2 has the wrong dim.
    let mut batch: Vec<(RecordId, Vec<f32>)> = Vec::new();
    let mut a = [0u8; 16];
    a[15] = 1;
    batch.push((RecordId(a), vec![1.0, 0.0, 0.0]));
    let mut b = [0u8; 16];
    b[15] = 2;
    batch.push((RecordId(b), vec![0.0, 1.0, 0.0]));
    let mut c = [0u8; 16];
    c[15] = 3;
    // WRONG dim (2 instead of 3) — should abort the whole batch.
    batch.push((RecordId(c), vec![1.0, 1.0]));

    let len_before = adapter.len();
    let err = adapter.upsert_batch(&batch).await.unwrap_err();
    assert!(
        matches!(
            err,
            VectorError::DimMismatch {
                expected: 3,
                got: 2
            }
        ),
        "expected DimMismatch, got {err:?}"
    );
    assert_eq!(
        adapter.len(),
        len_before,
        "len must be unchanged after atomic-abort batch"
    );
    // None of the batch rids should be findable.
    let results = adapter
        .search(&[1.0, 0.0, 0.0], 10, SearchOpts::default(), None)
        .await
        .unwrap();
    assert!(
        results.is_empty(),
        "no row from the aborted batch must be in the graph; got {results:?}"
    );
}

/// D12 regression for the batch path: a concurrent `upsert_batch` and
/// `upsert` of the SAME rid must leave the rid mapped to exactly ONE live
/// internal — never two. Mirrors `upsert_same_rid_concurrent_no_duplicate`
/// but exercises the batch ↔ single interleaving.
#[tokio::test]
#[serial_test::serial]
async fn upsert_batch_concurrent_with_upsert_no_duplicate() {
    use std::sync::Arc as StdArc;
    use tokio::sync::Barrier;

    let dim = 4usize;
    for iter in 0..40u64 {
        let adapter = StdArc::new(HnswAdapter::new(
            dim as u32,
            VectorMetric::L2,
            HnswConfig {
                max_elements: 1000,
                ..Default::default()
            },
        ));

        // Non-degenerate filler graph so search recall over survivors is
        // reliable (same rationale as the single-upsert D12 test).
        for j in 0..12u8 {
            adapter
                .upsert(rid(10 + j), &random_vec(dim, iter * 100 + j as u64))
                .await
                .unwrap();
        }

        let target = rid(1);
        // Batch side: target plus a few unrelated rids.
        let batch_vec_a = vec![1.0f32, 0.0, 0.0, 0.0];
        let batch: Vec<(RecordId, Vec<f32>)> = vec![
            (rid(2), random_vec(dim, iter * 7 + 1)),
            (rid(3), random_vec(dim, iter * 7 + 2)),
            (target, batch_vec_a.clone()),
        ];
        // Single-upsert side: same target, different vec.
        let single_vec_b = vec![0.0f32, 1.0, 0.0, 0.0];

        let barrier = StdArc::new(Barrier::new(2));
        let mut handles = Vec::new();

        // Task 1: batch upsert.
        {
            let a = StdArc::clone(&adapter);
            let b = StdArc::clone(&barrier);
            let batch = batch.clone();
            handles.push(tokio::spawn(async move {
                b.wait().await;
                a.upsert_batch(&batch).await.unwrap();
            }));
        }
        // Task 2: single upsert of the contended rid.
        {
            let a = StdArc::clone(&adapter);
            let b = StdArc::clone(&barrier);
            let v = single_vec_b.clone();
            handles.push(tokio::spawn(async move {
                b.wait().await;
                a.upsert(target, &v).await.unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        // (a) Direct state invariant: exactly one live internal for target.
        let mut live_internals = 0usize;
        adapter
            .rid_map
            .scan_async(|internal, mapped_rid| {
                if *mapped_rid == target && !adapter.deleted.contains(internal) {
                    live_internals += 1;
                }
            })
            .await;
        assert_eq!(
            live_internals, 1,
            "iter {iter}: rid must map to exactly ONE live internal after \
             batch↔single concurrent upsert; found {live_internals} (D12 batch race)"
        );

        // (b) End-to-end: target appears at most once in search.
        for q in [&batch_vec_a, &single_vec_b] {
            let results = adapter
                .search(q, 16, SearchOpts::default(), None)
                .await
                .unwrap();
            let occurrences = results.iter().filter(|(r, _)| *r == target).count();
            assert!(
                occurrences <= 1,
                "iter {iter}: rid surfaced {occurrences} times in search — \
                 duplicate live graph node (D12 batch); results={results:?}"
            );
        }
    }
}

/// D12 WITHIN a single batch: the SAME rid appearing twice in one
/// `upsert_batch` must resolve to the LAST occurrence (last-write-wins),
/// leave exactly one live internal, and surface only the later vector. This
/// is the intra-batch duplicate sub-case the batch↔single test does not cover.
#[tokio::test]
#[serial_test::serial]
async fn upsert_batch_duplicate_rid_within_batch_last_wins() {
    let dim = 4usize;
    let adapter = HnswAdapter::new(
        dim as u32,
        VectorMetric::L2,
        HnswConfig {
            max_elements: 100,
            ..Default::default()
        },
    );

    let target = rid(1);
    let vec_first = vec![1.0f32, 0.0, 0.0, 0.0];
    let vec_last = vec![0.0f32, 1.0, 0.0, 0.0];
    // Same rid twice (first, then last) plus one unrelated row.
    let batch: Vec<(RecordId, Vec<f32>)> = vec![
        (target, vec_first.clone()),
        (rid(2), random_vec(dim, 42)),
        (target, vec_last.clone()),
    ];
    adapter.upsert_batch(&batch).await.unwrap();

    // Two DISTINCT rids → len 2: the duplicate collapses to one live node
    // (the first internal is tombstoned).
    assert_eq!(
        adapter.len(),
        2,
        "duplicate rid within a batch must collapse to one live node"
    );

    // Exactly one live internal maps to target.
    let mut live = 0usize;
    adapter
        .rid_map
        .scan_async(|internal, mapped| {
            if *mapped == target && !adapter.deleted.contains(internal) {
                live += 1;
            }
        })
        .await;
    assert_eq!(
        live, 1,
        "duplicate rid within a batch must leave exactly one live internal"
    );

    // Last write wins: querying the LATER vector returns target as top-1, and
    // the earlier vector is tombstoned (removed from the brute-force scan).
    let results = adapter
        .search(&vec_last, 1, SearchOpts::default(), None)
        .await
        .unwrap();
    assert_eq!(
        results[0].0, target,
        "the later vector of a duplicated rid must be the surviving one"
    );
}

// ============================================================================
// V1.1 — per-query ef_search (SearchOpts)
// ============================================================================

/// Per-query `ef_search` raises recall monotonically (statistical, not strict):
/// on a 1k-vector HNSW graph, `ef_search = 400` yields recall@10 ≥
/// `ef_search = 16`. We assert the non-strict inequality `recall_high ≥
/// recall_low` — a strict `>` would be flaky given the unseedable layer RNG.
///
/// Ground truth is brute-force exact top-10 over the inserted set.
#[tokio::test]
async fn ef_search_higher_yields_at_least_as_good_recall() {
    let dim = 8usize;
    let n = 1000usize; // > BRUTE_FORCE_MAX → HNSW path

    // Build a shared graph ONCE with a generous build-time ef so the graph
    // is well-connected; recall differences come from the per-query ef only.
    let adapter = HnswAdapter::new(
        dim as u32,
        VectorMetric::L2,
        HnswConfig {
            max_elements: n + 100,
            ef_construction: 400,
            ef_search: 400, // build-time default (overridden per-query below)
            ..Default::default()
        },
    );
    let mut vecs: Vec<(RecordId, Vec<f32>)> = Vec::with_capacity(n);
    for i in 0..n {
        let mut a = [0u8; 16];
        a[12] = (i >> 24) as u8;
        a[13] = (i >> 16) as u8;
        a[14] = (i >> 8) as u8;
        a[15] = (i & 0xFF) as u8;
        let v = random_vec(dim, i as u64 * 31 + 7);
        vecs.push((RecordId(a), v));
    }
    adapter.upsert_batch(&vecs).await.unwrap();
    assert_eq!(adapter.len(), n);

    // Held-out query (NOT one of the inserted vectors) for ground truth.
    let query = random_vec(dim, 55555);

    // Brute-force ground-truth top-10.
    let mut dists: Vec<(RecordId, f32)> = vecs
        .iter()
        .map(|(r, v)| {
            let d: f32 = query
                .iter()
                .zip(v.iter())
                .map(|(a, b)| (a - b) * (a - b))
                .sum::<f32>()
                .sqrt();
            (*r, d)
        })
        .collect();
    dists.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    let gt: TFxSet<RecordId> = dists.iter().take(10).map(|(r, _)| *r).collect();

    // Low ef → lower recall.
    let r_low = adapter
        .search(&query, 10, SearchOpts::with_ef_search(16), None)
        .await
        .unwrap();
    let recall_low = gt
        .iter()
        .filter(|r| r_low.iter().any(|(rr, _)| rr == *r))
        .count() as f64
        / 10.0;

    // High ef → higher recall (non-strict).
    let r_high = adapter
        .search(&query, 10, SearchOpts::with_ef_search(400), None)
        .await
        .unwrap();
    let recall_high = gt
        .iter()
        .filter(|r| r_high.iter().any(|(rr, _)| rr == *r))
        .count() as f64
        / 10.0;

    // Non-strict monotonicity: higher ef ≥ lower ef. A strict > would be flaky.
    assert!(
        recall_high >= recall_low,
        "ef=400 recall={recall_high:.2} should be >= ef=16 recall={recall_low:.2}"
    );
    // Sanity: ef=400 should give reasonable recall on a 1k graph.
    assert!(recall_high >= 0.5, "ef=400 recall={recall_high:.2} < 0.50");
}

/// `ef_search = None` (default SearchOpts) uses the adapter's build-time
/// `HnswConfig::ef_search` (=50 default), NOT a zero. We verify by checking
/// that the default SearchOpts search returns the same result count as an
/// explicit ef=50 search.
#[tokio::test]
async fn ef_search_none_uses_build_time_default() {
    let adapter = HnswAdapter::new(
        2,
        VectorMetric::L2,
        HnswConfig {
            max_elements: 10,
            ef_search: 50,
            ..Default::default()
        },
    );
    adapter.upsert(rid(1), &[1.0, 0.0]).await.unwrap();
    adapter.upsert(rid(2), &[0.0, 1.0]).await.unwrap();

    let r_default = adapter
        .search(&[0.9, 0.1], 2, SearchOpts::default(), None)
        .await
        .unwrap();
    let r_explicit = adapter
        .search(&[0.9, 0.1], 2, SearchOpts::with_ef_search(50), None)
        .await
        .unwrap();
    assert_eq!(r_default.len(), r_explicit.len());
}

/// `ef_search = u32::MAX` must NOT panic — it is clamped to `MAX_EF_SEARCH`
/// (10_000) before being passed to `hnsw.search`. A huge ef behaves
/// identically to `MAX_EF_SEARCH` for recall but cannot starve the worker.
#[tokio::test]
async fn ef_search_u32_max_clamped_not_panicked() {
    let adapter = HnswAdapter::new(
        2,
        VectorMetric::L2,
        HnswConfig {
            max_elements: 10,
            ..Default::default()
        },
    );
    adapter.upsert(rid(1), &[1.0, 0.0]).await.unwrap();
    adapter.upsert(rid(2), &[0.0, 1.0]).await.unwrap();

    // u32::MAX would overflow the overscan/ef arithmetic if not clamped.
    let results = adapter
        .search(&[1.0, 0.0], 1, SearchOpts::with_ef_search(u32::MAX), None)
        .await
        .expect("ef_search = u32::MAX must be clamped, not panic");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, rid(1));
}

/// `ef_search` clamped above `MAX_EF_SEARCH` behaves identically to
/// `MAX_EF_SEARCH`. The adapter's internal clamp is
/// `ef.min(MAX_EF_SEARCH).max(k)`.
#[tokio::test]
async fn ef_search_above_cap_clamps_to_cap() {
    // On a graph large enough to hit the HNSW path (> BRUTE_FORCE_MAX), a
    // per-query ef of MAX_EF_SEARCH+1 must not panic and must return results.
    let dim = 4usize;
    let n = 300usize; // > BRUTE_FORCE_MAX
    let adapter = HnswAdapter::new(
        dim as u32,
        VectorMetric::L2,
        HnswConfig {
            max_elements: n + 100,
            ef_construction: 400,
            ..Default::default()
        },
    );
    let mut batch: Vec<(RecordId, Vec<f32>)> = Vec::with_capacity(n);
    for i in 0..n {
        let mut a = [0u8; 16];
        a[14] = (i >> 8) as u8;
        a[15] = (i & 0xFF) as u8;
        batch.push((RecordId(a), random_vec(dim, i as u64 * 7 + 3)));
    }
    adapter.upsert_batch(&batch).await.unwrap();

    let q = random_vec(dim, 12345);
    let above_cap = adapter
        .search(&q, 10, SearchOpts::with_ef_search(MAX_EF_SEARCH + 1), None)
        .await
        .expect("ef above MAX_EF_SEARCH must clamp, not panic");
    let at_cap = adapter
        .search(&q, 10, SearchOpts::with_ef_search(MAX_EF_SEARCH), None)
        .await
        .unwrap();
    // Same recall (both clamp to MAX_EF_SEARCH internally).
    assert_eq!(above_cap.len(), at_cap.len());
}

/// `oversample` is accepted in SearchOpts but NOT consumed by the adapter
/// directly (P3 / V3.1: the ENGINE consumes it — it requests `k′` candidates
/// and applies post-filtering). Passing it to the adapter must NOT error and
/// must behave identically to `None` at the adapter level.
#[tokio::test]
async fn oversample_accepted_but_ignored_at_adapter_level() {
    let adapter = HnswAdapter::new(2, VectorMetric::L2, HnswConfig::default());
    adapter.upsert(rid(1), &[1.0, 0.0]).await.unwrap();
    adapter.upsert(rid(2), &[0.0, 1.0]).await.unwrap();

    let with_oversample = adapter
        .search(
            &[0.7, 0.3],
            2,
            SearchOpts {
                ef_search: None,
                oversample: Some(3.0),
            },
            None,
        )
        .await
        .unwrap();
    let without = adapter
        .search(&[0.7, 0.3], 2, SearchOpts::default(), None)
        .await
        .unwrap();
    assert_eq!(with_oversample.len(), without.len());
    assert_eq!(with_oversample[0].0, without[0].0);
}

// ---------------------------------------------------------------------------
// V4.1 — deleted_count mirror tests
// ---------------------------------------------------------------------------

fn make_adapter(max: usize) -> HnswAdapter {
    HnswAdapter::new(
        4,
        VectorMetric::L2,
        HnswConfig {
            max_elements: max,
            ..Default::default()
        },
    )
}

#[tokio::test]
async fn len_is_o1_correct_after_churn() {
    let adapter = make_adapter(200);
    let dim = 4;

    // Insert 10 fresh rids via single upsert
    for i in 0u8..10 {
        adapter
            .upsert(rid(i), &random_vec(dim, i as u64))
            .await
            .unwrap();
    }
    assert_eq!(adapter.len(), 10);
    assert_eq!(adapter.deleted_count(), 0);

    // Replace 5 of them (single upsert path) — live count unchanged
    for i in 0u8..5 {
        adapter
            .upsert(rid(i), &random_vec(dim, 100 + i as u64))
            .await
            .unwrap();
    }
    assert_eq!(adapter.len(), 10);
    assert_eq!(adapter.deleted_count(), 5);

    // Delete 3
    for i in 5u8..8 {
        adapter.delete(rid(i)).await.unwrap();
    }
    assert_eq!(adapter.len(), 7);
    assert_eq!(adapter.deleted_count(), 8);

    // Now test upsert_batch path: insert 4 new + replace 2 existing
    let batch: Vec<(RecordId, Vec<f32>)> = (0u8..6)
        .map(|i| (rid(i), random_vec(dim, 200 + i as u64)))
        .collect();
    adapter.upsert_batch(&batch).await.unwrap();
    // 2 of those rids (0..5 minus deleted 5,6,7 = 0,1,2,3,4 live; batch has 0..6)
    // rids 0..5 are replacements (5 tombstones), rid 5 was deleted so insert_async
    // won't find it in rid_to_internal -> fresh insert.
    // Wait — rid 5 was deleted above, so rid_to_internal no longer has it.
    // Existing live before batch: 0,1,2,3,4,8,9 (7 live)
    // Batch rids: 0,1,2,3,4,5 — 5 are replacements (0-4), 1 is new insert (5)
    // After: live = 7 - 0 + 1 = 8, deleted_count = 8 + 5 = 13
    assert_eq!(adapter.len(), 8);
    assert_eq!(adapter.deleted_count(), 13);
}

#[tokio::test]
async fn deleted_ratio_tracks_tombstones() {
    let adapter = make_adapter(100);
    assert_eq!(adapter.deleted_ratio(), 0.0);

    // Insert 4 vectors
    for i in 0u8..4 {
        adapter
            .upsert(rid(i), &random_vec(4, i as u64))
            .await
            .unwrap();
    }
    // next_id=4, deleted=0 -> ratio=0.0
    assert_eq!(adapter.deleted_ratio(), 0.0);

    // Replace 2 -> next_id=6, deleted=2 -> ratio=2/6
    for i in 0u8..2 {
        adapter
            .upsert(rid(i), &random_vec(4, 50 + i as u64))
            .await
            .unwrap();
    }
    let expected = 2.0 / 6.0;
    assert!((adapter.deleted_ratio() - expected).abs() < 1e-10);
}

#[tokio::test]
async fn deleted_count_mirror_matches_map_len() {
    let adapter = make_adapter(100);

    for i in 0u8..8 {
        adapter
            .upsert(rid(i), &random_vec(4, i as u64))
            .await
            .unwrap();
    }
    // Replace some
    for i in 0u8..3 {
        adapter
            .upsert(rid(i), &random_vec(4, 80 + i as u64))
            .await
            .unwrap();
    }
    // Delete some
    for i in 5u8..7 {
        adapter.delete(rid(i)).await.unwrap();
    }

    #[allow(clippy::disallowed_methods)] // O(N) ack: test-only mirror invariant check
    let map_len = adapter.deleted.len();
    assert_eq!(adapter.deleted_count(), map_len);
}

#[tokio::test]
async fn from_parts_seeds_deleted_count() {
    use shamir_types::types::common::THasher;
    use std::sync::Arc;

    let dim = 4u32;
    let metric = VectorMetric::L2;
    let config = HnswConfig {
        max_elements: 100,
        ..Default::default()
    };
    let dist = ShamirDist { metric };
    let hnsw = Arc::new(hnsw_rs::hnsw::Hnsw::new(
        config.m,
        config.max_elements,
        config.max_layer,
        config.ef_construction,
        dist,
    ));

    let rid_map: scc::HashMap<usize, RecordId, THasher> =
        scc::HashMap::with_hasher(THasher::default());
    let rid_to_internal: scc::HashMap<RecordId, usize, THasher> =
        scc::HashMap::with_hasher(THasher::default());
    let vectors: scc::HashMap<usize, Vec<f32>, THasher> =
        scc::HashMap::with_hasher(THasher::default());
    let deleted: scc::HashMap<usize, (), THasher> = scc::HashMap::with_hasher(THasher::default());

    // Seed 5 tombstones
    for i in 0..5usize {
        let _ = deleted.insert(i, ());
    }

    let adapter = HnswAdapter::from_parts(
        dim,
        metric,
        config.ef_search,
        hnsw,
        rid_map,
        rid_to_internal,
        vectors,
        deleted,
        10, // next_id
    );

    assert_eq!(adapter.deleted_count(), 5);
    assert_eq!(adapter.live_count(), 5); // 10 - 5
    assert_eq!(adapter.len(), 5);
}
