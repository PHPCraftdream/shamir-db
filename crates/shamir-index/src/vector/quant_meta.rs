//! V5.3 (#412) — quantization metadata for the snapshot v2 sidecar.
//!
//! One primary export: [`QuantMeta`] — the bincode-serialisable envelope
//! that carries the frozen SQ8 quantizer parameters (per-dimension
//! `mins`/`scales` + dimensionality + method tag) from the dump path to
//! the load path. Stored inside [`SnapshotSidecar::quantization`] as
//! `Some(bincode(QuantMeta))`.
//!
//! This module is deliberately tiny and dependency-free: it MUST NOT depend
//! on the graph type or the snapshot codec (it lives in the sidecar, which
//! is a plain bincode blob). The [`Sq8Quantizer`](crate::vector::sq8::Sq8Quantizer)
//! round-trip is: `QuantMeta::from_quantizer` at dump time,
//! `QuantMeta::to_quantizer` at load time.

use crate::vector::sq8::Sq8Quantizer;
use serde::{Deserialize, Serialize};

/// Quantization metadata stored in the snapshot v2 sidecar.
///
/// `method` discriminates the quantizer family (`"sq8"` today). `mins` and
/// `scales` are the per-dimension parameters of the frozen SQ8 quantizer
/// (see [`Sq8Quantizer`]); `dim` is the vector dimensionality they were fit
/// on. The `min_sq_sum` constant term is NOT serialised — it is recomputed
/// from `mins` on load (a single `O(dim)` pass), keeping the on-wire format
/// minimal and forward-compatible (a future quantizer that does not use a
/// `min_sq_sum` is not blocked by carrying a vestigial field).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QuantMeta {
    /// Quantizer family tag. `"sq8"` for the V5.2 SQ8 quantizer.
    pub method: String,
    /// Vector dimensionality the quantizer was fit on.
    pub dim: usize,
    /// Per-dimension training minima (== `Sq8Quantizer::mins()`).
    pub mins: Vec<f32>,
    /// Per-dimension scales `(max - min) / 255` (== `Sq8Quantizer::scales()`).
    pub scales: Vec<f32>,
}

impl QuantMeta {
    /// Build a `QuantMeta` from a fitted [`Sq8Quantizer`] at dump time.
    ///
    /// The method tag is stamped [`QUANT_METHOD_SQ8`](crate::vector::snapshot::QUANT_METHOD_SQ8).
    pub fn from_quantizer(q: &Sq8Quantizer) -> Self {
        Self {
            method: crate::vector::snapshot::QUANT_METHOD_SQ8.to_string(),
            dim: q.dim(),
            mins: q.mins().to_vec(),
            scales: q.scales().to_vec(),
        }
    }

    /// Reconstruct an [`Sq8Quantizer`] from the stored params at load time.
    ///
    /// This recomputes the `min_sq_sum` constant from `mins` (an `O(dim)`
    /// pass) — see the struct doc for why it is not serialised. Panics if
    /// `method != "sq8"` (the load path guards on the method tag before
    /// calling this; a foreign method is refused with `VersionMismatch`).
    pub fn to_quantizer(&self) -> Sq8Quantizer {
        // Sq8Quantizer does not expose a `from_parts` constructor in #411;
        // we round-trip via `fit` on a synthetic zero vector to recompute
        // min_sq_sum, then overwrite mins/scales. This is O(dim) and runs
        // once per snapshot load — negligible vs the graph load.
        //
        // The `fit` call requires a non-empty training set; a single zero
        // vector gives `mins = [0; dim]`, `scales = [0; dim]`, and
        // `min_sq_sum = 0`, which we then overwrite with the stored params.
        assert_eq!(
            self.method,
            crate::vector::snapshot::QUANT_METHOD_SQ8,
            "QuantMeta::to_quantizer: unknown method {}",
            self.method
        );
        assert_eq!(self.mins.len(), self.dim);
        assert_eq!(self.scales.len(), self.dim);
        let zeros = vec![vec![0.0f32; self.dim]];
        let mut q = Sq8Quantizer::fit(&zeros, self.dim);
        // Overwrite the fit-derived params with the stored ones.
        q.overwrite_params(&self.mins, &self.scales);
        q
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `from_quantizer` ↔ `to_quantizer` round-trips the params exactly.
    #[test]
    fn quant_meta_round_trips_sq8_params() {
        let training: Vec<Vec<f32>> = (0..50)
            .map(|i| {
                (0..8)
                    .map(|j| (i as f32 * 0.1) + (j as f32 * 0.01))
                    .collect()
            })
            .collect();
        let q = Sq8Quantizer::fit(&training, 8);
        let meta = QuantMeta::from_quantizer(&q);
        assert_eq!(meta.method, "sq8");
        assert_eq!(meta.dim, 8);
        let q2 = meta.to_quantizer();
        assert_eq!(q2.dim(), q.dim());
        assert_eq!(q2.mins(), q.mins());
        assert_eq!(q2.scales(), q.scales());
        // Dequantize is identical (min_sq_sum is recomputed consistently).
        let v = training[0].clone();
        let codes = q.quantize(&v);
        let codes2 = q2.quantize(&v);
        assert_eq!(codes, codes2);
    }
}
