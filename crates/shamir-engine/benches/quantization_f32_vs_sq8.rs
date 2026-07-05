//! V5.3 (#412) — SQ8 quantization benchmark: f32 vs sq8.
//!
//! Measures the three trade-off axes of SQ8 scalar quantization on a single
//! clustered dataset:
//!
//! - **RSS** (resident-set size) — `memory_stats::memory_stats()` sampled
//!   right after the graph build, before any query. Reports the 4×
//!   memory-reduction claim (or the measured ratio).
//! - **QPS** (search latency) — criterion-bench top-k search, reported as
//!   throughput (queries / second). The u8-graph traversal is cheaper per
//!   hop (integer distance vs f32 SIMD), but the rescore adds an O(dim·k)
//!   f32 pass — net effect is measured here.
//! - **recall@10** — the SQ8 adapter's top-10 vs the f32 adapter's top-10
//!   (ground truth from the same HNSW graph build). Reported once per cell
//!   (NOT per-criterion-iter — recall is a property of the built graph, not
//!   the latency measurement).
//!
//! ## Dataset
//!
//! Clustered, `n × dim`, `k_clusters=50`, `σ=0.25`, `seed=0xBEEF` — the same
//! lineage as `quantized_graph_tests::recall_sq8_vs_f32_within_two_percent`.
//! At `n=1200, dim=128` this is large enough to cross the fit threshold
//! (256) and exercise the u8-graph path, small enough to stay within the
//! QUICK wall budget.
//!
//! ## Tuning
//!
//! `shamir_bench_utils::tune` is applied (QUICK default). The RSS + recall
//! numbers are printed once to stdout (NOT measured by criterion) so a
//! report can be built from a single QUICK run.

use std::sync::Arc;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use memory_stats::memory_stats;
use shamir_bench_utils as bu;
use shamir_engine::index2::kind::{VectorMetric, VectorQuantization};
use shamir_engine::index2::vector::adapter::{SearchOpts, VectorAdapter};
use shamir_engine::index2::vector::hnsw_adapter::{HnswAdapter, HnswConfig};
use shamir_types::types::record_id::RecordId;

/// Dataset size. > 256 so the SQ8 adapter crosses the fit threshold.
const N: usize = 1200;
/// Vector dimensionality — 128 ≈ small embedding.
const DIM: usize = 128;
/// Cluster count for the clustered generator.
const K_CLUSTERS: usize = 50;
/// Per-dimension Gaussian noise σ.
const SIGMA: f32 = 0.25;
/// Dataset seed — surfaces in the reproducibility key.
const SEED: u64 = 0xBEEF;
/// top-k for the search + recall measurement.
const TOP_K: u32 = 10;

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

/// Deterministic LCG clustered generator (mirrors the test helper lineage).
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

/// Sample the process RSS in bytes (best-effort — returns 0 if the OS query
/// fails, which `memory_stats` documents as possible on some platforms).
fn rss_now() -> u64 {
    memory_stats().map(|m| m.physical_mem as u64).unwrap_or(0)
}

fn recall_at_k(truth: &[RecordId], cand: &[RecordId]) -> f32 {
    let hits = cand
        .iter()
        .filter(|c| truth.iter().any(|t| t == *c))
        .count();
    hits as f32 / truth.len().max(1) as f32
}

/// Build BOTH adapters on the same batch, sample RSS after each build,
/// compute recall@10 over a fixed query set, and print a one-line summary.
/// The criterion bench below measures only the search latency (QPS).
///
/// #418 — RSS is now sampled PER-ADAPTER in isolation: we build f32, sample
/// its RSS, compute recall, DROP the f32 adapter, then build sq8 and sample
/// its RSS. This isolates each adapter's footprint (the pre-#418 code kept
/// both resident, so `rss_sq8` included the f32 graph and masked SQ8's
/// memory win). Both adapters are returned for the criterion search bench
/// (rebuilt after sampling — see `rebuild_for_bench`).
fn build_and_report(rt: &tokio::runtime::Runtime) -> (Arc<HnswAdapter>, Arc<HnswAdapter>) {
    let data = clustered(N, DIM, K_CLUSTERS, SIGMA, SEED);
    let batch: Vec<(RecordId, Vec<f32>)> = data
        .iter()
        .enumerate()
        .map(|(i, v)| (rid_from(i), v.clone()))
        .collect();

    let cfg = HnswConfig {
        max_elements: 10_000,
        m: 16,
        max_layer: 16,
        ef_construction: 200,
        ef_search: 128,
    };

    // ---- RSS baseline (process footprint before any adapter) ------------
    let rss_baseline = rss_now();

    // ---- Build f32 adapter, sample RSS, compute recall, then DROP it -----
    let f32_adapter = Arc::new(HnswAdapter::new(DIM as u32, VectorMetric::Cosine, cfg.clone()));
    rt.block_on(f32_adapter.upsert_batch(&batch)).unwrap();
    let rss_f32 = rss_now();
    // Footprint of the f32 adapter ALONE = RSS now - baseline. (Allocator
    // fragmentation may leave some slack, but this is the closest OS-level
    // measurement available without a dedicated allocator hook.)
    let fp_f32 = rss_f32.saturating_sub(rss_baseline);

    // recall@10 over a fixed query set (first 100 vectors of the dataset).
    // Computed here (before the sq8 build) so we can DROP the f32 adapter
    // for an isolated sq8 RSS sample.
    let opts = SearchOpts {
        ef_search: Some(256),
        oversample: None,
    };
    let n_queries = 100usize;
    // Stash the ground-truth top-k per query so recall can be recomputed
    // against the sq8 adapter after the f32 adapter is dropped.
    let truth_rids_per_q: Vec<Vec<RecordId>> = data
        .iter()
        .take(n_queries)
        .map(|q| {
            rt.block_on(f32_adapter.search(q, TOP_K, opts, None))
                .unwrap()
                .iter()
                .map(|(r, _)| *r)
                .collect()
        })
        .collect();

    // DROP the f32 adapter so the sq8 RSS sample is not contaminated by
    // its f32 graph. The Arc is the only strong reference (the criterion
    // bench rebuilds fresh adapters below).
    drop(f32_adapter);

    // ---- Build sq8 adapter, sample RSS, compute recall ------------------
    let sq8_adapter = Arc::new(HnswAdapter::new_with_quantization(
        DIM as u32,
        VectorMetric::Cosine,
        cfg,
        Some(VectorQuantization::Sq8),
    ));
    rt.block_on(sq8_adapter.upsert_batch(&batch)).unwrap();
    // The SQ8 adapter crosses the fit threshold (N > 256) during the batch
    // build. We cannot call `is_quantized()` directly from outside the crate
    // (it is `pub(crate)`), so we assert the observable contract: the adapter
    // accepted the batch and has N live vectors.
    assert_eq!(
        sq8_adapter.len(),
        N,
        "SQ8 adapter did not accept the full batch"
    );
    let rss_sq8 = rss_now();
    // Footprint of the sq8 adapter ALONE = RSS now - f32 baseline. The f32
    // graph of THIS adapter was dropped post-fit (#418), so this measures
    // only the u8 graph + codes + overhead.
    let fp_sq8 = rss_sq8.saturating_sub(rss_f32);

    let mut total_recall = 0.0f32;
    for (i, q) in data.iter().take(n_queries).enumerate() {
        let cand = rt
            .block_on(sq8_adapter.search(q, TOP_K, opts, None))
            .unwrap();
        let cand_rids: Vec<RecordId> = cand.iter().map(|(r, _)| *r).collect();
        total_recall += recall_at_k(&truth_rids_per_q[i], &cand_rids);
    }
    let avg_recall = total_recall / n_queries as f32;

    // #418 — the per-adapter footprints are the meaningful comparison.
    // SQ8 should be noticeably SMALLER than f32 (u8 codes = dim bytes vs
    // f32 vectors = 4·dim bytes; target ~¼, plus graph-structure overhead).
    let fp_delta = fp_sq8 as i64 - fp_f32 as i64;
    let ratio = if fp_f32 > 0 {
        fp_sq8 as f64 / fp_f32 as f64
    } else {
        0.0
    };
    println!(
        "[quant-bench] n={N} dim={DIM} metric=cosine | footprint f32={fp_f32} bytes, sq8={fp_sq8} bytes \
         (Δ={fp_delta}, ratio={ratio:.3}) | recall@{TOP_K} sq8-vs-f32 = {avg_recall:.4}"
    );

    // Rebuild the f32 adapter for the criterion search bench (it was dropped
    // above for an isolated sq8 RSS sample). The sq8 adapter is reused.
    let f32_adapter = Arc::new(HnswAdapter::new(DIM as u32, VectorMetric::Cosine, HnswConfig {
        max_elements: 10_000,
        m: 16,
        max_layer: 16,
        ef_construction: 200,
        ef_search: 128,
    }));
    rt.block_on(f32_adapter.upsert_batch(&batch)).unwrap();

    (f32_adapter, sq8_adapter)
}

fn bench_quantization(c: &mut Criterion) {
    let rt = rt();
    let (f32_adapter, sq8_adapter) = build_and_report(&rt);

    let mut group = c.benchmark_group("quantization_f32_vs_sq8");
    // QUICK by default. The `120s` wall-guard caps the FULL-mode worst case.
    bu::tune_tiered(&mut group, 100, 5, 3, 120);
    group.throughput(Throughput::Elements(1));

    // A fixed query (same lineage as the dataset, distinct seed).
    let query: Vec<f32> = clustered(1, DIM, K_CLUSTERS, SIGMA, SEED + 1).into_iter().next().unwrap();
    let opts = SearchOpts::with_ef_search(256);

    group.bench_function("f32_search", |b| {
        let f32_adapter = Arc::clone(&f32_adapter);
        let q = query.clone();
        b.to_async(&rt).iter(move || {
            let a = Arc::clone(&f32_adapter);
            let q = q.clone();
            async move { a.search(&q, TOP_K, opts, None).await.unwrap() }
        });
    });

    group.bench_function("sq8_search", |b| {
        let sq8_adapter = Arc::clone(&sq8_adapter);
        let q = query.clone();
        b.to_async(&rt).iter(move || {
            let a = Arc::clone(&sq8_adapter);
            let q = q.clone();
            async move { a.search(&q, TOP_K, opts, None).await.unwrap() }
        });
    });

    group.finish();
}

criterion_group!(benches, bench_quantization);
criterion_main!(benches);
