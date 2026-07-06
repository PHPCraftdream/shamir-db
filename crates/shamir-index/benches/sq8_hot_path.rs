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

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use hnsw_rs::anndists::dist::distances::Distance;
use shamir_bench_utils::{tune_tiered, vector_data::clustered_vectors};
use shamir_index::kind::VectorMetric;
use shamir_index::vector::quantized_dist::ShamirDistU8;
use shamir_index::vector::sq8::Sq8Quantizer;
use std::sync::Arc;

/// Embedding dimensionality for the bench (typical small-model shape).
const DIM: usize = 128;
/// Candidate pool size per query — approximates one HNSW layer-0 hop batch
/// (M=16, ef ~ a few hundred). 256 keeps the cell wall-clock modest in QUICK.
const POOL: usize = 256;
/// Training-set size for the quantizer fit (>= FIT_THRESHOLD so the params
/// are non-degenerate on a realistic spread).
const N_TRAIN: usize = 1024;

fn bench_approx_dot(c: &mut Criterion) {
    let mut group = c.benchmark_group("sq8_approx_dot");
    // ~10 samples × 0.5s × 2 metrics; cap QUICK cell at ~15s worst case.
    tune_tiered(&mut group, 10, 1, 1, 15);

    let ds = clustered_vectors(N_TRAIN, DIM, 32, 0.15, 4242);
    let training: Vec<Vec<f32>> = ds.vectors.clone();
    let q = Sq8Quantizer::fit(&training, DIM);

    // Quantize the candidate pool + a fixed query code.
    let pool_codes: Vec<Vec<u8>> = ds
        .vectors
        .iter()
        .take(POOL)
        .map(|v| q.quantize(v))
        .collect();
    let query_code = q.quantize(&ds.centroids[0]);

    group.throughput(Throughput::Elements(POOL as u64));
    group.bench_function(BenchmarkId::new("dim", DIM), |b| {
        b.iter(|| {
            let mut acc = 0.0f32;
            for cand in &pool_codes {
                acc += black_box(q.approx_dot(black_box(&query_code), black_box(cand)));
            }
            acc
        })
    });
    group.finish();
}

fn bench_dist_eval(c: &mut Criterion) {
    let mut group = c.benchmark_group("shamir_dist_u8_eval");
    tune_tiered(&mut group, 10, 1, 1, 20);

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
        group.throughput(Throughput::Elements(POOL as u64));
        group.bench_with_input(
            BenchmarkId::new(format!("{metric:?}"), DIM),
            &dist,
            |b, dist: &ShamirDistU8| {
                b.iter(|| {
                    let mut acc = 0.0f32;
                    for cand in &pool_codes {
                        acc += black_box(dist.eval(black_box(&query_code), black_box(cand)));
                    }
                    acc
                })
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_approx_dot, bench_dist_eval);
criterion_main!(benches);
