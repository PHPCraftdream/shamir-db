//! Filtered vector search benchmarks: pre-filter vs co-filter vs post-filter
//! across varying selectivities.
//!
//! Measures the three filter paths at selectivities {0.1%, 1%, 5%, 10%, 25%, 50%}
//! on a fixed HNSW index (n=10_000, dim=128, Cosine). Validates cost-based
//! thresholds `PRE_FILTER_MAX_CANDIDATES` (4096) and `CO_FILTER_MAX_SELECTIVITY`
//! (0.20).
//!
//! Dataset: `clustered_vectors(10_000, 128, k=64, sigma=0.1, seed=42)`.

use std::sync::Arc;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use shamir_bench_utils as bu;
use shamir_bench_utils::vector_data::clustered_vectors;
use shamir_engine::index2::kind::VectorMetric;
use shamir_engine::index2::vector::adapter::{SearchOpts, VectorAdapter};
use shamir_engine::index2::vector::hnsw_adapter::{
    HnswAdapter, HnswConfig, CO_FILTER_EF_MULTIPLIER, CO_FILTER_MAX_SELECTIVITY,
    PRE_FILTER_MAX_CANDIDATES,
};
use shamir_types::types::record_id::RecordId;

const N: usize = 10_000;
const DIM: usize = 128;
const SEED: u64 = 42;
const K_CLUSTERS: usize = 64;
const SIGMA: f32 = 0.1;
const TOP_K: u32 = 10;

/// Selectivities to benchmark (fraction of dataset passing the filter).
const SELECTIVITIES: &[f64] = &[0.001, 0.01, 0.05, 0.10, 0.25, 0.50];

/// Post-filter oversample multiplier (matches V3.1 default).
const POST_FILTER_OVERSAMPLE: u32 = 4;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn rid_from(i: usize) -> RecordId {
    let mut a = [0u8; 16];
    a[8..16].copy_from_slice(&(i as u64).to_be_bytes());
    RecordId(a)
}

/// Build a deterministic allow-set of ceil(selectivity * n) RIDs using a
/// simple stride-based selection (deterministic, O(n) worst case but builds
/// once per selectivity slice).
fn build_allow_set(n: usize, selectivity: f64, seed: u64) -> Vec<RecordId> {
    let count = ((selectivity * n as f64).ceil() as usize).max(1);
    // Use a simple LCG seeded by `seed` to pick `count` distinct indices.
    let mut rng = seed;
    let mut selected = shamir_collections::new_fx_set_wc::<usize>(count);
    while selected.len() < count {
        // LCG: x' = (a*x + c) mod m
        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let idx = (rng >> 33) as usize % n;
        selected.insert(idx);
    }
    selected.into_iter().map(rid_from).collect()
}

fn bench_filtered(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("filtered_vector_search");
    // QUICK-default via tune_tiered + wall-guard 60s.
    bu::tune_tiered(&mut group, 100, 5, 3, 60);
    group.throughput(Throughput::Elements(1));

    // Build dataset and HNSW once.
    let ds = clustered_vectors(N, DIM, K_CLUSTERS, SIGMA, SEED);
    let batch: Vec<(RecordId, Vec<f32>)> = ds
        .vectors
        .iter()
        .enumerate()
        .map(|(i, v)| (rid_from(i), v.clone()))
        .collect();

    let query: Vec<f32> = clustered_vectors(1, DIM, K_CLUSTERS, SIGMA, SEED + 1).vectors[0].clone();

    let hnsw = Arc::new(HnswAdapter::new(
        DIM as u32,
        VectorMetric::Cosine,
        HnswConfig {
            max_elements: N + 1_000,
            m: 16,
            max_layer: 16,
            ef_construction: 200,
            ef_search: 50,
        },
    ));
    rt.block_on(hnsw.upsert_batch(&batch)).unwrap();

    // Log thresholds for reference (visible in bench output).
    eprintln!(
        "[filtered_vector_search] PRE_FILTER_MAX_CANDIDATES={}, CO_FILTER_MAX_SELECTIVITY={}, CO_FILTER_EF_MULTIPLIER={}",
        PRE_FILTER_MAX_CANDIDATES, CO_FILTER_MAX_SELECTIVITY, CO_FILTER_EF_MULTIPLIER
    );

    for &sel in SELECTIVITIES {
        let permille = (sel * 1000.0) as u32;
        let allow_set = build_allow_set(N, sel, SEED + 100);
        let n_candidates = allow_set.len();
        let label = format!("sel_{permille}p_n{n_candidates}");

        // ── Pre-filter: brute-force SIMD over candidate set ──────────────
        {
            let hnsw = Arc::clone(&hnsw);
            let q = query.clone();
            let candidates = allow_set.clone();
            group.bench_with_input(BenchmarkId::new("prefilter", &label), &sel, |b, _| {
                let hnsw = Arc::clone(&hnsw);
                let q = q.clone();
                let candidates = candidates.clone();
                b.to_async(&rt).iter(move || {
                    let hnsw = Arc::clone(&hnsw);
                    let q = q.clone();
                    let candidates = candidates.clone();
                    async move { hnsw.search_prefilter(&q, TOP_K, &candidates).await.unwrap() }
                });
            });
        }

        // ── Co-filter: HNSW search_filter with allow-set ────────────────
        {
            let hnsw = Arc::clone(&hnsw);
            let q = query.clone();
            let candidates = allow_set.clone();
            group.bench_with_input(BenchmarkId::new("cofilter", &label), &sel, |b, _| {
                let hnsw = Arc::clone(&hnsw);
                let q = q.clone();
                let candidates = candidates.clone();
                b.to_async(&rt).iter(move || {
                    let hnsw = Arc::clone(&hnsw);
                    let q = q.clone();
                    let candidates = candidates.clone();
                    async move {
                        hnsw.search_cofilter(&q, TOP_K, None, &candidates)
                            .await
                            .unwrap()
                    }
                });
            });
        }

        // ── Post-filter: oversample + manual filter ──────────────────────
        {
            let hnsw = Arc::clone(&hnsw);
            let q = query.clone();
            let candidates = allow_set.clone();
            group.bench_with_input(BenchmarkId::new("postfilter", &label), &sel, |b, _| {
                let hnsw = Arc::clone(&hnsw);
                let q = q.clone();
                let candidates = candidates.clone();
                b.to_async(&rt).iter(move || {
                    let hnsw = Arc::clone(&hnsw);
                    let q = q.clone();
                    let candidates = candidates.clone();
                    async move {
                        // Oversample: fetch k * multiplier, then filter to allow-set.
                        let oversample_k = TOP_K * POST_FILTER_OVERSAMPLE;
                        let mut results = hnsw
                            .search(&q, oversample_k, SearchOpts::default(), None)
                            .await
                            .unwrap();
                        // Filter to allowed RIDs only.
                        let allow: shamir_collections::TFxSet<RecordId> =
                            candidates.iter().copied().collect();
                        results.retain(|(rid, _)| allow.contains(rid));
                        results.truncate(TOP_K as usize);
                        results
                    }
                });
            });
        }
    }

    group.finish();
}

criterion_group!(benches, bench_filtered);
criterion_main!(benches);
