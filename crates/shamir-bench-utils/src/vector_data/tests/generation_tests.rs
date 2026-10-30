use crate::vector_data::*;

/// Squared L2 distance — cheaper than the sqrt version and sufficient for
/// ordering comparisons in the clustering sanity checks.
fn sq_l2(a: &[f32], b: &[f32]) -> f64 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| {
            let d = (*x as f64) - (*y as f64);
            d * d
        })
        .sum()
}

/// Reproducibility artefacts must identify the generator revision (the old
/// code exposed no version at all; on unfixed code this cannot even compile).
#[test]
fn rv_bench_utils_126e5e6f_generator_revision_is_pinned() {
    assert_eq!(GENERATOR_REVISION, "clustered_vectors/2");
    assert!(!GENERATOR_REVISION.is_empty());
}

#[test]
fn same_seed_is_byte_identical() {
    let a = clustered_vectors(64, 8, 4, 0.1, 42);
    let b = clustered_vectors(64, 8, 4, 0.1, 42);
    assert_eq!(a.vectors, b.vectors, "same seed must reproduce exactly");
    assert_eq!(a.centroids, b.centroids, "centroids must reproduce too");
}

#[test]
fn different_seed_differs() {
    let a = clustered_vectors(64, 8, 4, 0.1, 42);
    let b = clustered_vectors(64, 8, 4, 0.1, 43);
    assert_ne!(a.vectors, b.vectors, "different seeds must differ");
    assert_ne!(a.centroids, b.centroids);
}

#[test]
fn dimensions_and_counts_are_correct() {
    let n = 50;
    let dim = 16;
    let k = 5;
    let ds = clustered_vectors(n, dim, k, 0.2, 7);
    assert_eq!(ds.n(), n);
    assert_eq!(ds.dim(), dim);
    assert_eq!(ds.k(), k);
    for v in &ds.vectors {
        assert_eq!(v.len(), dim);
    }
    for c in &ds.centroids {
        assert_eq!(c.len(), dim);
    }
}

#[test]
fn round_robin_balances_clusters() {
    // n divisible by k → perfectly balanced assignment via round-robin.
    let n = 40;
    let k = 4;
    let ds = clustered_vectors(n, 4, k, 0.05, 99);
    // Each point's nearest centroid (by construction it is its own cluster).
    let mut counts = vec![0usize; k];
    for v in &ds.vectors {
        let (idx, _) = ds
            .centroids
            .iter()
            .map(|c| sq_l2(v, c))
            .enumerate()
            .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .unwrap();
        counts[idx] += 1;
    }
    for &c in &counts {
        assert_eq!(c, n / k, "round-robin should balance clusters exactly");
    }
}

#[test]
fn points_are_clustered_not_scattered() {
    // Sanity: mean intra-cluster distance << mean inter-cluster distance.
    // σ small relative to the [-1,1] centroid box so clusters are tight.
    let n = 200;
    let dim = 8;
    let k = 5;
    let sigma = 0.05;
    let ds = clustered_vectors(n, dim, k, sigma, 1234);

    // Assign each point to its nearest centroid.
    let mut intra: Vec<f64> = Vec::new();
    for v in &ds.vectors {
        let (_, d) = ds
            .centroids
            .iter()
            .map(|c| sq_l2(v, c))
            .enumerate()
            .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .unwrap();
        intra.push(d.sqrt());
    }
    let mean_intra: f64 = intra.iter().sum::<f64>() / intra.len() as f64;

    // Mean pairwise inter-cluster centroid distance.
    let mut inter: Vec<f64> = Vec::new();
    for i in 0..k {
        for j in (i + 1)..k {
            inter.push(sq_l2(&ds.centroids[i], &ds.centroids[j]).sqrt());
        }
    }
    let mean_inter: f64 = inter.iter().sum::<f64>() / inter.len().max(1) as f64;

    // Sanity threshold (not a tight statistical bound): intra should be a
    // small fraction of inter. If the generator were uniform-by-mistake
    // this ratio would blow up towards 1.
    assert!(
        mean_intra < 0.25 * mean_inter,
        "intra-cluster dist {mean_intra:.4} should be << inter-cluster {mean_inter:.4}"
    );
}

#[test]
fn k_greater_than_n_clamps_silently() {
    // More clusters than points: must not panic, must not produce empty
    // centroid slots beyond those actually used.
    let ds = clustered_vectors(3, 4, 10, 0.1, 5);
    assert_eq!(ds.n(), 3);
    assert!(ds.k() <= 3, "k should clamp to n");
    assert_eq!(ds.dim(), 4);
}

#[test]
#[should_panic(expected = "k_clusters must be > 0")]
fn zero_clusters_panics() {
    let _ = clustered_vectors(10, 4, 0, 0.1, 1);
}

#[test]
fn n_zero_returns_centroids_only() {
    let ds = clustered_vectors(0, 4, 3, 0.1, 1);
    assert_eq!(ds.n(), 0);
    assert!(ds.k() > 0);
    assert_eq!(ds.dim(), 4, "dim recoverable from centroids alone");
}

#[test]
fn lcg_streams_differ_for_different_seeds() {
    let mut a = Lcg::new(1);
    let mut b = Lcg::new(2);
    let sa: u64 = (0..4).map(|_| a.next_u64()).fold(0u64, u64::wrapping_add);
    let sb: u64 = (0..4).map(|_| b.next_u64()).fold(0u64, u64::wrapping_add);
    assert_ne!(sa, sb);
}

#[test]
fn zero_dimension_is_rejected() {
    assert!(std::panic::catch_unwind(|| clustered_vectors(1, 0, 1, 0.1, 1)).is_err());
}

#[test]
fn rv_bench_utils_2caf36ea_negative_sigma_is_rejected() {
    assert!(std::panic::catch_unwind(|| clustered_vectors(1, 1, 1, -0.1, 1)).is_err());
}

#[test]
fn rv_bench_utils_2caf36ea_non_finite_sigma_is_rejected() {
    assert!(std::panic::catch_unwind(|| clustered_vectors(1, 1, 1, f32::NAN, 1)).is_err());
}

#[test]
fn rv_bench_utils_2f1d995c_rows_are_contiguous_and_keep_slice_semantics() {
    let ds = clustered_vectors(3, 2, 1, 0.0, 9);
    assert_eq!(ds.vectors.len(), 3);
    assert_eq!(ds.vectors[0].len(), 2);
    assert_eq!(
        ds.vectors.iter().map(<[f32]>::len).collect::<Vec<_>>(),
        [2, 2, 2]
    );
    assert_eq!(
        ds.vectors[0].as_ptr().wrapping_add(2),
        ds.vectors[1].as_ptr()
    );
    assert_eq!(
        ds.vectors[1].as_ptr().wrapping_add(2),
        ds.vectors[2].as_ptr()
    );
}
