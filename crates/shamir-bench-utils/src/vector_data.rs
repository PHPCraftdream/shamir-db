//! Seeded clustered vector-dataset generator for vector benchmarks.
//!
//! Shared between the criterion bench (V0.3, `benches/vector_search.rs`) and
//! the recall/RSS report tool (V0.4, `examples/vector_report.rs`). Lives here
//! so both tools see **byte-identical** datasets for the same `(n, dim, k, σ,
//! seed)` tuple — recall numbers are only comparable when the underlying data
//! is reproducible.
//!
//! # Distribution
//!
//! **Clustered, not uniform.** A uniform `[0,1]^dim` cloud flatters ANN
//! recall: in high dimensions uniform points are nearly equidistant, so any
//! approximate neighbour looks correct. Real embedding spaces are clustered
//! around K semantic centroids, so we model that:
//!
//! 1. Draw `k_clusters` centroids uniformly in `[-1, 1]^dim`.
//! 2. For each of the `n` points, pick a centroid (round-robin so clusters are
//!    balanced — keeps recall ground-truth non-degenerate) and add per-dimension
//!    Gaussian noise `N(0, σ²)` via Box-Muller.
//!
//! # Determinism
//!
//! No global RNG, no `Math.random`/`Date::now`/system entropy. A single
//! [`Lcg`] seeded by `seed` drives everything: centroid coordinates, centroid
//! assignment, and the Box-Muller pairs. Generation is single-threaded with a
//! fixed iteration order, so the same `seed` yields byte-identical output
//! across runs on a given target. (Cross-target f32 identity additionally
//! relies on IEEE-754 `ln`/`sqrt`, which is stable in practice but not
//! promised here.) The reproducibility key is
//! `(GENERATOR_REVISION, n, dim, k_clusters, sigma, seed)` — surface it in
//! every report.
//!
//! # RNG choice
//!
//! [`Lcg`] uses the Numerical Recipes constant `6364136223846793005`
//! (matching `shamir-index::vector::tests::hnsw_rs_contract_tests::lcg_vec`)
//! so contract-test fixtures and bench data share a lineage. It is **not**
//! cryptographically secure — it does not need to be; it only needs to be
//! deterministic and fast.

/// Numerical Recipes LCG multiplier (matches the workspace contract-test
/// helper so bench data and fixtures stay in the same lineage).
pub const LCG_MULT: u64 = 6364136223846793005;
/// Numerical Recipes LCG increment.
pub const LCG_INC: u64 = 1442695040888963407;

/// Revision of the dataset generator (algorithm + constants) — surface
/// this in every published report so artefacts identify the code that
/// produced them. Bump whenever generation output could change.
pub const GENERATOR_REVISION: &str = "clustered_vectors/1";

/// A tiny deterministic linear-congruential generator.
///
/// `state ← state * MULT + INC (mod 2^64)`; the high 32 bits are returned as
/// the random payload (low bits of an LCG have poor entropy). No global state,
/// no locking — pure value type, cloneable, seedable.
#[derive(Clone, Debug)]
pub struct Lcg {
    state: u64,
}

impl Lcg {
    /// Create a new generator. `seed = 0` is allowed and behaves like any
    /// other seed (the first step multiplies before reading, so `0` does not
    /// produce an all-zero stream).
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Advance one step and return the next raw `u64` state.
    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_mul(LCG_MULT).wrapping_add(LCG_INC);
        self.state
    }

    /// Uniform `f32` in `[0, 1)`. Uses the high 32 bits.
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        let high = (self.next_u64() >> 32) as u32;
        (high as f32) / (1u64 << 32) as f32
    }

    /// Uniform `f32` in `[lo, hi)`.
    #[inline]
    pub fn next_range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }

    /// Standard-normal sample via the polar Box-Muller transform.
    ///
    /// Consumes two `u64`s from the stream per accepted draw (rejection
    /// sampling on the unit-disk check; on reject it redraws). Two LCG
    /// uniforms → one Gaussian, as specified in the bench brief.
    pub fn next_gaussian(&mut self) -> f32 {
        loop {
            // Map two independent u64s to [-1, 1].
            let u1 = self.next_f32() * 2.0 - 1.0;
            let u2 = self.next_f32() * 2.0 - 1.0;
            let s = u1 * u1 + u2 * u2;
            if s > 0.0 && s < 1.0 {
                let mul = ((-2.0 * s.ln()) / s).sqrt();
                // We only need one of the pair; the second (u2 * mul) is
                // discarded — determinism is preserved because we always
                // consume exactly the same stream prefix on accept.
                return u1 * mul;
            }
        }
    }
}

/// Result of a clustered dataset generation: contiguous point and centroid
/// coordinates in flat storage.
///
/// Centroids are returned so the report tool can derive a cheap ground-truth
/// proxy. Requested generation parameters are not retained in this artifact;
/// reports must record the complete reproducibility key separately.
#[derive(Debug, Clone)]
pub struct ClusteredDataset {
    /// `n` points of dimension `dim`, in insertion order.
    pub vectors: VectorRows,
    /// `k_clusters` centroids of dimension `dim`.
    pub centroids: VectorRows,
}

mod vector_rows;
pub use vector_rows::VectorRows;

impl ClusteredDataset {
    /// Dimensionality of every vector in the dataset.
    pub fn dim(&self) -> usize {
        self.vectors
            .iter()
            .next()
            .or_else(|| self.centroids.iter().next())
            .map(<[f32]>::len)
            .unwrap_or(0)
    }

    /// Number of points.
    #[inline]
    pub fn n(&self) -> usize {
        self.vectors.len()
    }

    /// Number of clusters.
    #[inline]
    pub fn k(&self) -> usize {
        self.centroids.len()
    }
}

/// Generate a clustered vector dataset deterministically from `seed`.
///
/// Produces `n` points of dimension `dim` distributed around `k_clusters`
/// centroids (each centroid in `[-1, 1]^dim`) with per-dimension Gaussian
/// noise of standard deviation `sigma`.
///
/// Centroid assignment is round-robin so cluster sizes are balanced (within
/// one) — this keeps brute-force recall ground-truth non-degenerate: every
/// cluster contributes roughly the same number of true neighbours.
/// `k_clusters == 0` panics; for positive `n`, `k_clusters > n` clamps to `n`.
/// `sigma` must be finite and nonnegative; zero disables Gaussian noise.
///
/// # Reproducibility
///
/// Same `(GENERATOR_REVISION, n, dim, k_clusters, sigma, seed)` produces
/// byte-identical output on a given target. Surface all six values in reports.
///
/// # Panics
///
/// Panics if `k_clusters == 0`, `dim == 0`, or `sigma` is negative or
/// non-finite. `sigma == 0` is valid and produces centroid copies.
pub fn clustered_vectors(
    n: usize,
    dim: usize,
    k_clusters: usize,
    sigma: f32,
    seed: u64,
) -> ClusteredDataset {
    assert!(k_clusters > 0, "k_clusters must be > 0");
    assert!(dim > 0, "dim must be > 0");
    assert!(
        sigma.is_finite() && sigma >= 0.0,
        "sigma must be finite and >= 0"
    );

    // Effective cluster count. For n > 0 we cap at n so round-robin assignment
    // never references an unused centroid (round-robin index < k_eff for every
    // point). For n == 0, retain requested k centroids; only dim and
    // requested k are recoverable from the artifact, not sigma.
    let k_eff = if n == 0 {
        k_clusters
    } else {
        k_clusters.min(n)
    };

    let mut rng = Lcg::new(seed);

    // 1. Centroids: uniform in [-1, 1]^dim, generated in a reusable point buffer.
    let mut centroids = VectorRows::with_capacity(k_eff, dim);
    let mut point = Vec::with_capacity(dim);
    for _ in 0..k_eff {
        point.clear();
        for _ in 0..dim {
            point.push(rng.next_range(-1.0, 1.0));
        }
        centroids.push(&point);
    }

    // Degenerate case: n == 0 → return just the requested centroids.
    if n == 0 {
        return ClusteredDataset {
            vectors: VectorRows::with_capacity(0, dim),
            centroids,
        };
    }

    // 2. Points: round-robin centroid + Gaussian noise.
    let mut vectors = VectorRows::with_capacity(n, dim);
    let mut point = Vec::with_capacity(dim);
    for i in 0..n {
        let c = &centroids[i % k_eff];
        point.clear();
        for &cv in c {
            point.push(cv + sigma * rng.next_gaussian());
        }
        vectors.push(&point);
    }

    ClusteredDataset { vectors, centroids }
}

#[cfg(test)]
mod tests;
