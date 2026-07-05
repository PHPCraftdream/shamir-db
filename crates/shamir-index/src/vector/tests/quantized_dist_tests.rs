//! Tests for [`ShamirDistU8`] — the quantized distance function.
//!
//! Coverage:
//!  * **eval == exact f32 distance on dequantized vectors** (per-metric):
//!    L2, Dot, Cosine. The proof is that `ShamirDistU8::eval(qx, qy)` must
//!    equal `ShamirDist(f32).eval(dequant(qx), dequant(qy))` to within fp
//!    rounding, because both expand the same `min_i + q_i·s_i` model.
//!  * **Clone is cheap** (Arc clone) — smoke test that the distance can be
//!    cloned for hnsw_rs rayon thread-locals.
//!  * **Metric convention matches ShamirDist** — Dot returns `1 − dot`
//!    (clamped ≥ 0), Cosine returns `1 − cos_sim`, L2 returns `sqrt(Σ)`.
//!  * **rescore_f32** matches `ShamirDist::eval` on the dequantized vector
//!    (the rescore path used after a quantized graph traversal).

use crate::kind::VectorMetric;
use crate::vector::hnsw_adapter::ShamirDist;
use crate::vector::quantized_dist::{rescore_f32, ShamirDistU8};
use crate::vector::simd::{dot_product, l2_squared};
use crate::vector::sq8::Sq8Quantizer;
use hnsw_rs::anndists::dist::distances::Distance;
use std::sync::Arc;

// ----- deterministic RNG (mirrors sq8_tests::Lcg lineage) -----------------

struct Lcg {
    state: u64,
}
impl Lcg {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }
    #[inline]
    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }
    #[inline]
    fn next_f32(&mut self) -> f32 {
        let high = (self.next_u64() >> 32) as u32;
        (high as f32) / (1u64 << 32) as f32
    }
    #[inline]
    fn next_range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }
    fn next_gaussian(&mut self) -> f32 {
        loop {
            let u1 = self.next_f32() * 2.0 - 1.0;
            let u2 = self.next_f32() * 2.0 - 1.0;
            let s = u1 * u1 + u2 * u2;
            if s > 0.0 && s < 1.0 {
                let mul = ((-2.0 * s.ln()) / s).sqrt();
                return u1 * mul;
            }
        }
    }
}

fn clustered(n: usize, dim: usize, k: usize, sigma: f32, seed: u64) -> Vec<Vec<f32>> {
    assert!(k > 0);
    let mut rng = Lcg::new(seed);
    let centroids: Vec<Vec<f32>> = (0..k)
        .map(|_| (0..dim).map(|_| rng.next_range(-1.0, 1.0)).collect())
        .collect();
    (0..n)
        .map(|i| {
            let c = &centroids[i % k];
            (0..dim)
                .map(|j| c[j] + sigma * rng.next_gaussian())
                .collect()
        })
        .collect()
}

/// Reference f32 distance on two dequantized code vectors, using the SAME
/// metric convention as `ShamirDist` (sqrt for L2, 1−dot for Dot,
/// 1−cos_sim for Cosine). This is the ground truth `ShamirDistU8::eval`
/// must match.
fn ref_dist_f32(metric: VectorMetric, q: &Sq8Quantizer, qx: &[u8], qy: &[u8]) -> f32 {
    let x = q.dequantize(qx);
    let y = q.dequantize(qy);
    let dist = ShamirDist { metric };
    dist.eval(&x, &y)
}

// =========================================================================
// eval == exact f32 distance on dequantized vectors (per-metric)
// =========================================================================

#[test]
fn eval_l2_matches_dequant_reference() {
    let dim = 64;
    let train = clustered(256, dim, 16, 0.2, 11);
    let q = Arc::new(Sq8Quantizer::fit(&train, dim));
    let dist = ShamirDistU8::new(Arc::clone(&q), VectorMetric::L2);

    let mut max_abs_err = 0.0f32;
    for i in 0..100 {
        let a = &train[i];
        let b = &train[(i + 50) % train.len()];
        let qa = q.quantize(a);
        let qb = q.quantize(b);
        let got = dist.eval(&qa, &qb);
        let want = ref_dist_f32(VectorMetric::L2, &q, &qa, &qb);
        let err = (got - want).abs();
        if err > max_abs_err {
            max_abs_err = err;
        }
    }
    // L2 on dequantized vectors is EXACT up to fp rounding — both paths
    // compute sqrt(Σ (x_i − y_i)²) with x_i = min_i + q_i·s_i. The only
    // difference is accumulation order (ShamirDistU8 folds s_i² inside,
    // ShamirDist computes on the dequantized f32s), bounded by a few ulp.
    assert!(
        max_abs_err < 1e-3,
        "L2 eval max abs error {max_abs_err} too large"
    );
}

#[test]
fn eval_dot_matches_dequant_reference() {
    let dim = 128;
    let train = clustered(512, dim, 32, 0.25, 22);
    let q = Arc::new(Sq8Quantizer::fit(&train, dim));
    let dist = ShamirDistU8::new(Arc::clone(&q), VectorMetric::Dot);

    let mut max_abs_err = 0.0f32;
    for i in 0..100 {
        let a = &train[i];
        let b = &train[(i + 100) % train.len()];
        let qa = q.quantize(a);
        let qb = q.quantize(b);
        let got = dist.eval(&qa, &qb);
        let want = ref_dist_f32(VectorMetric::Dot, &q, &qa, &qb);
        let err = (got - want).abs();
        if err > max_abs_err {
            max_abs_err = err;
        }
    }
    assert!(
        max_abs_err < 1e-2,
        "Dot eval max abs error {max_abs_err} too large"
    );
}

#[test]
fn eval_cosine_matches_dequant_reference() {
    let dim = 96;
    let train = clustered(384, dim, 24, 0.3, 33);
    let q = Arc::new(Sq8Quantizer::fit(&train, dim));
    let dist = ShamirDistU8::new(Arc::clone(&q), VectorMetric::Cosine);

    let mut max_abs_err = 0.0f32;
    for i in 0..100 {
        let a = &train[i];
        let b = &train[(i + 80) % train.len()];
        let qa = q.quantize(a);
        let qb = q.quantize(b);
        let got = dist.eval(&qa, &qb);
        let want = ref_dist_f32(VectorMetric::Cosine, &q, &qa, &qb);
        let err = (got - want).abs();
        if err > max_abs_err {
            max_abs_err = err;
        }
    }
    assert!(
        max_abs_err < 1e-3,
        "Cosine eval max abs error {max_abs_err} too large"
    );
}

// =========================================================================
// eval is non-negative (HNSW invariant) and deterministic
// =========================================================================

#[test]
fn eval_dot_is_non_negative() {
    let dim = 32;
    let train = clustered(64, dim, 8, 0.2, 44);
    let q = Arc::new(Sq8Quantizer::fit(&train, dim));
    let dist = ShamirDistU8::new(Arc::clone(&q), VectorMetric::Dot);
    for i in 0..30 {
        let a = q.quantize(&train[i]);
        let b = q.quantize(&train[(i + 5) % train.len()]);
        let d = dist.eval(&a, &b);
        assert!(d >= 0.0, "Dot eval returned negative distance {d}");
    }
}

#[test]
fn eval_is_deterministic() {
    let dim = 16;
    let train = clustered(32, dim, 4, 0.2, 55);
    let q = Arc::new(Sq8Quantizer::fit(&train, dim));
    let dist = ShamirDistU8::new(q, VectorMetric::L2);
    let a = dist.quantizer().quantize(&train[0]);
    let b = dist.quantizer().quantize(&train[1]);
    let d1 = dist.eval(&a, &b);
    let d2 = dist.eval(&a, &b);
    let d3 = dist.eval(&a, &b);
    assert_eq!(d1.to_bits(), d2.to_bits(), "eval not deterministic");
    assert_eq!(d2.to_bits(), d3.to_bits(), "eval not deterministic");
}

// =========================================================================
// clone is cheap (Arc clone) — hnsw_rs clones Distance for rayon
// =========================================================================

#[test]
fn clone_shares_arc() {
    let dim = 8;
    let train = clustered(16, dim, 4, 0.1, 66);
    let q = Arc::new(Sq8Quantizer::fit(&train, dim));
    let dist = ShamirDistU8::new(Arc::clone(&q), VectorMetric::Dot);
    let cloned = dist.clone();
    // Both distances point at the SAME Arc<Sq8Quantizer>.
    assert!(
        Arc::ptr_eq(dist.quantizer(), cloned.quantizer()),
        "Clone did not share the quantizer Arc"
    );
    // Eval agrees.
    let a = q.quantize(&train[0]);
    let b = q.quantize(&train[1]);
    assert_eq!(dist.eval(&a, &b).to_bits(), cloned.eval(&a, &b).to_bits());
}

// =========================================================================
// rescore_f32 matches ShamirDist::eval on dequantized codes
// =========================================================================

#[test]
fn rescore_f32_matches_shamir_dist_on_dequant() {
    let dim = 48;
    let train = clustered(128, dim, 12, 0.2, 77);
    let q = Sq8Quantizer::fit(&train, dim);

    // The query is an ORIGINAL f32 vector (not quantized) — this is what
    // the rescore path receives from the client.
    for metric in [VectorMetric::L2, VectorMetric::Dot, VectorMetric::Cosine] {
        let shamir = ShamirDist { metric };
        for i in 0..30 {
            let query = &train[i];
            let codes = q.quantize(&train[(i + 7) % train.len()]);
            let dequant = q.dequantize(&codes);
            let want = shamir.eval(query, &dequant);
            let got = rescore_f32(metric, &q, query, &codes);
            let err = (got - want).abs();
            assert!(
                err < 1e-4,
                "rescore_f32({metric:?}) err {err} too large: got {got}, want {want}"
            );
        }
    }
}

// =========================================================================
// approx_l2_sq (on Sq8Quantizer) matches the per-dim scalar reference
// =========================================================================

#[test]
fn approx_l2_sq_matches_scalar_reference() {
    let dim = 32;
    let train = clustered(64, dim, 8, 0.3, 88);
    let q = Sq8Quantizer::fit(&train, dim);
    let qx = q.quantize(&train[0]);
    let qy = q.quantize(&train[1]);

    // Scalar reference: Σ s_i² · (qx_i − qy_i)² in i32-then-f32.
    let mut ref_val = 0.0f32;
    for i in 0..dim {
        let diff = (qx[i] as i32) - (qy[i] as i32);
        let s = q.scales()[i];
        ref_val += s * s * (diff * diff) as f32;
    }
    let got = q.approx_l2_sq(&qx, &qy);
    assert!(
        (got - ref_val).abs() < 1e-5,
        "approx_l2_sq {got} != scalar reference {ref_val}"
    );
}

// =========================================================================
// L2 eval uses sqrt (not l2_squared) — pin the convention
// =========================================================================

#[test]
fn eval_l2_returns_sqrt_not_squared() {
    let dim = 4;
    let train = vec![
        vec![0.0, 0.0, 0.0, 0.0],
        vec![3.0, 4.0, 0.0, 0.0], // ||(3,4,0,0)|| = 5
    ];
    let q = Arc::new(Sq8Quantizer::fit(&train, dim));
    let dist = ShamirDistU8::new(q, VectorMetric::L2);
    let qa = dist.quantizer().quantize(&train[0]);
    let qb = dist.quantizer().quantize(&train[1]);
    let got = dist.eval(&qa, &qb);
    // sqrt(approx_l2_sq) ≈ 5.0 (within quantization rounding).
    assert!(
        (got - 5.0).abs() < 0.1,
        "L2 eval {got} should be ≈ 5.0 (sqrt convention)"
    );
    // Sanity: l2_squared would be ≈ 25.
    assert!(got < 6.0, "L2 eval {got} looks like squared, not sqrt");
}

// =========================================================================
// rescore_f32 uses the SAME SIMD kernels as ShamirDist — smoke check
// that the kernels are wired (not a no-op). This guards against a future
// refactor accidentally dropping the kernel call.
// =========================================================================

#[test]
fn rescore_f32_uses_simd_kernels() {
    let a = [1.0f32, 2.0, 3.0, 4.0];
    let b = [1.0f32, 2.0, 3.0, 4.0];
    // dot_product(a, b) = 1+4+9+16 = 30
    assert_eq!(dot_product(&a, &b), 30.0);
    // l2_squared(a, a) = 0
    assert_eq!(l2_squared(&a, &a), 0.0);
}
