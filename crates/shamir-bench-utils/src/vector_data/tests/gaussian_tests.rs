//! Statistical (distribution-shape) tests for [`Lcg::next_gaussian`] and the
//! σ scaling of [`clustered_vectors`].

use crate::vector_data::*;

/// Pins the Box-Muller scale — the old suite only had byte-identity
/// (same-impl) and an upper clustering bound, so a transcription bug in
/// `next_gaussian` (e.g. dropping the `-2.0`) passed everything. Production
/// code unchanged — the fix IS the discriminating test.
#[test]
fn rv_bench_utils_325f3ac6_next_gaussian_moments_match_unit_normal() {
    let mut rng = Lcg::new(7);
    let n = 100_000;
    let mut sum = 0.0f64;
    let mut sum_sq = 0.0f64;
    for _ in 0..n {
        let g = rng.next_gaussian() as f64;
        sum += g;
        sum_sq += g * g;
    }
    let mean = sum / n as f64;
    let variance = sum_sq / n as f64 - mean * mean;
    assert!(mean.abs() < 0.05, "mean {mean} too far from 0");
    assert!(
        (0.85..=1.15).contains(&variance),
        "variance {variance} outside unit-normal range"
    );
}

/// σ enters the noise linearly per dimension, so the variance of
/// `(point - centroid)` must scale quadratically in σ (≈4× when σ doubles),
/// and a small σ must still produce measurable spread (the old suite had
/// only an upper bound).
#[test]
fn rv_bench_utils_325f3ac6_sigma_scales_point_spread_quadratically() {
    let per_dim_variance = |sigma: f32| -> f64 {
        let ds = clustered_vectors(2000, 4, 2, sigma, 11);
        let mut acc = 0.0f64;
        let mut count = 0usize;
        for (i, point) in ds.vectors.iter().enumerate() {
            let c = &ds.centroids[i % ds.k()];
            for j in 0..ds.dim() {
                let d = (point[j] - c[j]) as f64;
                acc += d * d;
                count += 1;
            }
        }
        acc / count as f64
    };

    let v01 = per_dim_variance(0.1);
    let v02 = per_dim_variance(0.2);
    assert!(v01 > 0.004, "sigma=0.1 variance {v01} too small");
    let ratio = v02 / v01;
    assert!(
        (3.0..=5.0).contains(&ratio),
        "variance ratio {ratio} not ~quadratic (expected ≈4)"
    );
}
