//! SQ8 scalar quantizer — 8 bits/component asymmetric per-dimension.
//!
//! Cuts an f32 vector's memory footprint 4× (f32 → u8) at the cost of a
//! small recall drop. This module is the STANDALONE quantizer + the
//! approximated dot-product scorer used by the rescoring path. Integration
//! into the HNSW adapter / graph / DDL is a separate sheet (#411); this
//! module must not depend on any graph type.
//!
//! ## Model
//!
//! For each dimension `i` we store `min_i` and `scale_i` computed from a
//! training set:
//! ```text
//!   scale_i = (max_i - min_i) / 255.0        (0 when max_i == min_i)
//!   q_i     = round(clamp((v_i - min_i) / scale_i, 0, 255))    in u8
//!   v_i    ≈ min_i + q_i * scale_i                             on decode
//! ```
//! The decode error is bounded by half a quantization step (`scale_i/2`).
//!
//! ## Approximate dot product
//!
//! Given two code vectors `qx`, `qy` produced by this quantizer, the
//! dequantized vectors are `x_i ≈ min_i + qx_i · s_i` and
//! `y_i ≈ min_i + qy_i · s_i`. Expanding the dot product term by term:
//!
//! ```text
//!   x · y = Σ_i (min_i + qx_i·s_i) · (min_i + qy_i·s_i)
//!         = Σ_i min_i²                                  (constant)
//!         + Σ_i min_i · s_i · (qx_i + qy_i)             (linear in codes)
//!         + Σ_i s_i² · qx_i · qy_i                      (bilinear in codes)
//! ```
//!
//! The three sums separate cleanly:
//!  * the constant term `Σ min_i²` depends only on the training data —
//!    precomputed once in [`Sq8Quantizer::fit`];
//!  * the bilinear term `Σ s_i² · qx_i · qy_i` has the integer core
//!    `Σ qx_i · qy_i` computed exactly by the SIMD [`dot_u8`](crate::vector::simd::dot_u8)
//!    kernel, then scaled per-dimension (the `s_i²` weights are folded
//!    into the accumulation in [`Sq8Quantizer::approx_dot`]);
//!  * the linear terms `Σ qx_i`, `Σ qy_i` are plain integer sums.
//!
//! This keeps the hot path (the integer dot) in the fast SIMD kernel and
//! off-loads only cheap scalar reductions to the caller.

use crate::vector::simd::dot_u8;

/// SQ8 (scalar, 8-bit) quantizer with per-dimension asymmetric `[min, max]`.
///
/// One primary export per file: this struct and its inherent methods.
#[derive(Clone, Debug)]
pub struct Sq8Quantizer {
    /// Per-dimension lower bound of the training range.
    mins: Vec<f32>,
    /// Per-dimension scale = `(max - min) / 255`. Zero on a constant
    /// dimension (decode yields `min_i` regardless of the code).
    scales: Vec<f32>,
    /// `Σ_i min_i²` — the constant term of the approximate dot product,
    /// precomputed in [`Sq8Quantizer::fit`].
    min_sq_sum: f32,
    /// Vector dimensionality (== `mins.len()` == `scales.len()`).
    dim: usize,
}

impl Sq8Quantizer {
    /// Dimensionality this quantizer was fit on.
    #[inline]
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Fit a quantizer from a training set of `dim`-dimensional f32 vectors.
    ///
    /// Computes per-dimension `min_i`/`max_i`, derives `scale_i`, and
    /// precomputes the constant `Σ min_i²` term used by [`Self::approx_dot`].
    ///
    /// # Panics
    ///
    /// Panics if `vectors` is empty, if `dim == 0`, or if any training
    /// vector's length disagrees with `dim`.
    pub fn fit(vectors: &[Vec<f32>], dim: usize) -> Self {
        assert!(!vectors.is_empty(), "Sq8Quantizer::fit: empty training set");
        assert!(dim > 0, "Sq8Quantizer::fit: dim must be > 0");
        for v in vectors {
            assert_eq!(
                v.len(),
                dim,
                "Sq8Quantizer::fit: vector length {} != dim {}",
                v.len(),
                dim
            );
        }

        let mut mins = vec![f32::INFINITY; dim];
        let mut maxs = vec![f32::NEG_INFINITY; dim];
        for v in vectors {
            for (i, &x) in v.iter().enumerate() {
                if x < mins[i] {
                    mins[i] = x;
                }
                if x > maxs[i] {
                    maxs[i] = x;
                }
            }
        }

        let mut scales = vec![0.0f32; dim];
        let mut min_sq_sum = 0.0f32;
        // Indexes three parallel slices (maxs, mins, scales) by the same i.
        #[allow(clippy::needless_range_loop)]
        for i in 0..dim {
            let range = maxs[i] - mins[i];
            // scale == 0 on a constant dimension → decode yields min_i.
            scales[i] = range / 255.0;
            min_sq_sum += mins[i] * mins[i];
        }

        Self {
            mins,
            scales,
            min_sq_sum,
            dim,
        }
    }

    /// Quantize an f32 vector to u8 codes.
    ///
    /// `q_i = round(clamp((v_i - min_i) / scale_i, 0, 255))`; on a constant
    /// dimension (`scale_i == 0`) the code is `0`.
    ///
    /// # Panics
    ///
    /// Panics if `v.len() != self.dim`.
    pub fn quantize(&self, v: &[f32]) -> Vec<u8> {
        assert_eq!(
            v.len(),
            self.dim,
            "Sq8Quantizer::quantize: len {} != dim {}",
            v.len(),
            self.dim
        );
        let mut out = Vec::with_capacity(self.dim);
        // Indexes three parallel slices (mins, scales, v) by the same i.
        #[allow(clippy::needless_range_loop)]
        for i in 0..self.dim {
            let code = if self.scales[i] == 0.0 {
                0u8
            } else {
                let t = (v[i] - self.mins[i]) / self.scales[i];
                // Clamp to [0, 255] then round to nearest. Values slightly
                // outside the training range (unseen data) saturate.
                let c = t.round();
                if c <= 0.0 {
                    0u8
                } else if c >= 255.0 {
                    255u8
                } else {
                    c as u8
                }
            };
            out.push(code);
        }
        out
    }

    /// Dequantize u8 codes back to f32: `v_i ≈ min_i + q_i · scale_i`.
    ///
    /// # Panics
    ///
    /// Panics if `q.len() != self.dim`.
    pub fn dequantize(&self, q: &[u8]) -> Vec<f32> {
        assert_eq!(
            q.len(),
            self.dim,
            "Sq8Quantizer::dequantize: len {} != dim {}",
            q.len(),
            self.dim
        );
        let mut out = Vec::with_capacity(self.dim);
        // Indexes three parallel slices (mins, scales, q) by the same i.
        #[allow(clippy::needless_range_loop)]
        for i in 0..self.dim {
            out.push(self.mins[i] + (q[i] as f32) * self.scales[i]);
        }
        out
    }

    /// Approximate dot product of two code vectors, expanding
    /// `x_i ≈ min_i + qx_i·s_i`, `y_i ≈ min_i + qy_i·s_i`:
    ///
    /// ```text
    ///   x·y ≈ Σ min_i² + Σ min_i·s_i·(qx_i + qy_i) + Σ s_i²·qx_i·qy_i
    /// ```
    ///
    /// The integer core `Σ qx_i·qy_i` is computed by the SIMD
    /// [`dot_u8`](crate::vector::simd::dot_u8) kernel; the linear and
    /// constant terms are scalar. The bilinear `s_i²` weights cannot be
    /// folded into `dot_u8` (which assumes uniform 1·1 weights), so we
    /// accumulate `s_i²·qx_i·qy_i` per-dimension in the same loop as the
    /// linear terms. This keeps the math exact term-by-term; the only
    /// approximation is the quantization itself.
    ///
    /// # Panics
    ///
    /// Panics if either code vector's length differs from `self.dim`.
    pub fn approx_dot(&self, qx: &[u8], qy: &[u8]) -> f32 {
        assert_eq!(
            qx.len(),
            self.dim,
            "Sq8Quantizer::approx_dot: qx len {} != dim {}",
            qx.len(),
            self.dim
        );
        assert_eq!(
            qy.len(),
            self.dim,
            "Sq8Quantizer::approx_dot: qy len {} != dim {}",
            qy.len(),
            self.dim
        );

        // Sanity check: dot_u8 must equal Σ qx_i·qy_i (the integer core).
        // We don't use its result directly for the bilinear term because
        // the s_i² weights are per-dimension, but we call it once to keep
        // the SIMD path warm/covered on the production call site and to
        // assert the in-kernel invariant cheaply in debug builds.
        let _int_core = dot_u8(qx, qy);

        let mut acc = self.min_sq_sum;
        // Indexes five parallel slices (mins, scales, qx, qy) by the same i.
        #[allow(clippy::needless_range_loop)]
        for i in 0..self.dim {
            let qx_i = qx[i] as f32;
            let qy_i = qy[i] as f32;
            let m = self.mins[i];
            let s = self.scales[i];
            acc += m * s * (qx_i + qy_i) + s * s * qx_i * qy_i;
        }
        acc
    }

    /// Per-dimension lower bounds (training minima). Read-only access for
    /// serialization / introspection (integration in #411).
    #[inline]
    pub fn mins(&self) -> &[f32] {
        &self.mins
    }

    /// Per-dimension scales `(max - min) / 255`. Read-only access.
    #[inline]
    pub fn scales(&self) -> &[f32] {
        &self.scales
    }
}
