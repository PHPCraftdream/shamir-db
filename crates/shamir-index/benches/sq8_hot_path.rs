//! SQ8 quantized hot-path bench — `Sq8Quantizer::approx_dot` (called by
//! `ShamirDistU8::eval` for Dot/Cosine on every HNSW graph hop & insert) and
//! `ShamirDistU8::eval` itself for the three metrics.
//!
//! VR-7 (#429): the baseline `approx_dot` contained a dead `dot_u8(qx, qy)`
//! call whose result was computed and discarded — an extra O(dim) SIMD pass
//! on every edge evaluation. This bench pins baseline vs post-fix throughput.
//!
//! Inputs: a clustered dim-128 dataset (matches typical embedding shape)
//! quantized through a fit-on-training `Sq8Quantizer`. Each iter scores a
//! query code against a fixed candidate pool, simulating one HNSW hop batch.
//!
//! Migrated to the fixed-iteration harness (`bench_scale_tool`): setup
//! (quantizer, pool codes, query code) is built ONCE outside the timed
//! closure, exactly as under Criterion's `b.iter` — plan 1 (shared setup).

use std::hint::black_box;
use std::sync::Arc;

use bench_scale_tool::Harness;
use hnsw_rs::anndists::dist::distances::Distance;
use shamir_bench_utils::vector_data::clustered_vectors;
use shamir_index::kind::VectorMetric;
use shamir_index::vector::quantized_dist::ShamirDistU8;
use shamir_index::vector::sq8::Sq8Quantizer;

/// Embedding dimensionality for the bench (typical small-model shape).
const DIM: usize = 128;
/// Candidate pool size per query — approximates one HNSW layer-0 hop batch
/// (M=16, ef ~ a few hundred). 256 keeps the cell wall-clock modest.
const POOL: usize = 256;
/// Training-set size for the quantizer fit (>= FIT_THRESHOLD so the params
/// are non-degenerate on a realistic spread).
const N_TRAIN: usize = 1024;

fn main() {
    let mut h = Harness::new("sq8_hot_path", env!("CARGO_MANIFEST_DIR"));

    // --- sq8_approx_dot/dim_128 ---------------------------------------------
    {
        let ds = clustered_vectors(N_TRAIN, DIM, 32, 0.15, 4242);
        let training: Vec<Vec<f32>> = ds.vectors.clone();
        let q = Sq8Quantizer::fit(&training, DIM);
        let pool_codes: Vec<Vec<u8>> = ds
            .vectors
            .iter()
            .take(POOL)
            .map(|v| q.quantize(v))
            .collect();
        let query_code = q.quantize(&ds.centroids[0]);

        h.bench("sq8_approx_dot/dim_128", move || {
            let mut acc = 0.0f32;
            for cand in &pool_codes {
                acc += black_box(q.approx_dot(black_box(&query_code), black_box(cand)));
            }
            black_box(acc);
        });
    }

    // --- shamir_dist_u8_eval/{L2,Dot,Cosine}/dim_128 ------------------------
    {
        let ds = clustered_vectors(N_TRAIN, DIM, 32, 0.15, 4242);
        let training: Vec<Vec<f32>> = ds.vectors.clone();
        let q = Arc::new(Sq8Quantizer::fit(&training, DIM));
        let pool_codes: Vec<Vec<u8>> = ds
            .vectors
            .iter()
            .take(POOL)
            .map(|v| q.quantize(v))
            .collect();
        let query_code = q.quantize(&ds.centroids[0]);

        for metric in [VectorMetric::L2, VectorMetric::Dot, VectorMetric::Cosine] {
            let dist = ShamirDistU8::new(Arc::clone(&q), metric);
            let pool_codes = pool_codes.clone();
            let query_code = query_code.clone();
            let id = format!("shamir_dist_u8_eval/{metric:?}/dim_128");
            h.bench(&id, move || {
                let mut acc = 0.0f32;
                for cand in &pool_codes {
                    acc += black_box(dist.eval(black_box(&query_code), black_box(cand)));
                }
                black_box(acc);
            });
        }
    }

    h.run();
}
