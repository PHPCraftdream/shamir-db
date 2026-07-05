//! Vector search benchmarks: HNSW vs BruteForce.
//!
//! V0.3 rewrite — measures top-k (k=10) **search latency** across the
//! production-relevant axes:
//!
//! - **n** ∈ {10_000, 100_000} — 1_000_000 is opt-in via env (see [`ladder`]).
//! - **dim** ∈ {128, 768} — 128 ≈ small embedding, 768 ≈ BERT/Sentence-BERT.
//! - **metric** ∈ {Cosine, L2} — Dot is omitted: HNSW requires non-negative
//!   distances, so the Dot path is just Cosine-on-normalized-vectors; covered
//!   by Cosine.
//!
//! ## Data — clustered, not uniform
//!
//! Datasets come from [`shamir_bench_utils::vector_data::clustered_vectors`]
//! (the shared V0.1 generator). Parameters: `k_clusters=64`, `sigma=0.1`,
//! `seed=42`. Clustered data is harder for ANN than a uniform cloud — in high
//! dimensions uniform points are nearly equidistant, flattering recall — so
//! the numbers here are a sterner test than the old local uniform-LCG. The
//! `(n, dim, k=64, σ=0.1, seed=42)` triple is the reproducibility key; surface
//! it in any report built off these numbers.
//!
//! ## Build path — batched
//!
//! The HNSW graph is built via ONE [`VectorAdapter::upsert_batch`] call (V0.2):
//! a single rayon `parallel_insert` over the whole dataset instead of N serial
//! `upsert`s. At n=100_000 the serial path took minutes; the batched path is
//! seconds. `BruteForceAdapter` also goes through `upsert_batch` (its default
//! impl loops `upsert`, but the actor coalesces publishes — still far cheaper
//! than awaiting per-row in the bench harness).
//!
//! ## BruteForce scope
//!
//! `BruteForceAdapter` is exact KNN — O(N·dim) per query. At n=10_000 that is
//! microseconds (good baseline); at n=100_000×768 it is tens of milliseconds
//! per query, which (a) is uninteresting (we know brute-force is slow at
//! scale) and (b) risks pushing a QUICK cell past its wall budget. So
//! BruteForce is measured **only at n=10_000**. HNSW is measured at both
//! rungs.
//!
//! ## 1M rung
//!
//! n=1_000_000 is intentionally **not in the default ladder**: building a
//! 768-dim HNSW graph over 1M points takes minutes and would dominate any
//! smoke/QUICK run. It is available ad-hoc via `BENCH_VECTOR_1M=1` as a single
//! extra Cosine/128 point — enable only for a deliberate long run, never in CI.
//!
//! ## Tuning
//!
//! Every group calls [`shamir_bench_utils::tune_tiered`] with a wall-guard,
//! so SMOKE/QUICK keep cells to seconds while a hypothetical FULL run would
//! still be bounded. `Throughput::Elements(1)` — one top-k query per iter.

use std::sync::Arc;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use shamir_bench_utils as bu;
use shamir_bench_utils::vector_data::clustered_vectors;
use shamir_engine::index2::kind::VectorMetric;
use shamir_engine::index2::vector::adapter::VectorAdapter;
use shamir_engine::index2::vector::brute_force::BruteForceAdapter;
use shamir_engine::index2::vector::hnsw_adapter::{HnswAdapter, HnswConfig};
use shamir_types::types::record_id::RecordId;

/// Fixed dataset seed — surfaces in the reproducibility key
/// `(n, dim, k=64, σ=0.1, seed=42)`.
const SEED: u64 = 42;
/// Cluster count for the clustered generator. 64 keeps clusters non-degenerate
/// at n=10K (~156 points/cluster) and well-populated at n=100K.
const K_CLUSTERS: usize = 64;
/// Per-dimension Gaussian noise σ around each centroid. 0.1 keeps clusters
/// tight relative to the `[-1,1]` centroid box (intra << inter).
const SIGMA: f32 = 0.1;
/// top-k search depth — the value the production read path uses for
/// "give me the 10 nearest" queries.
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

/// The default n-ladder. 1M is opt-in via `BENCH_VECTOR_1M=1` (see module docs).
fn ladder() -> Vec<usize> {
    let mut rungs: Vec<usize> = vec![10_000, 100_000];
    if std::env::var("BENCH_VECTOR_1M")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
    {
        // 1M only as Cosine/128 — 768-dim at 1M is a multi-minute build,
        // deliberately not auto-added.
        rungs.push(1_000_000);
    }
    rungs
}

/// One (n, dim, metric) cell: build HNSW (+ optional BruteForce) and bench
/// top-k search latency.
fn bench_cell(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    rt: &tokio::runtime::Runtime,
    n: usize,
    dim: usize,
    metric: VectorMetric,
) {
    // Reproducibility key: (n, dim, k=64, σ=0.1, seed=42).
    let ds = clustered_vectors(n, dim, K_CLUSTERS, SIGMA, SEED);
    debug_assert_eq!(ds.n(), n);
    debug_assert_eq!(ds.dim(), dim);

    // Build the (rid, vec) batch ONCE — reused for both adapters.
    let batch: Vec<(RecordId, Vec<f32>)> = ds
        .vectors
        .iter()
        .enumerate()
        .map(|(i, v)| (rid_from(i), v.clone()))
        .collect();

    // A fixed query drawn from the same generator lineage (seed+1 so it is
    // deterministic but distinct from the dataset seed). Dim matches.
    let query: Vec<f32> = clustered_vectors(1, dim, K_CLUSTERS, SIGMA, SEED + 1).vectors[0].clone();

    // ── Build HNSW via one batched upsert ──────────────────────────────────
    let hnsw = Arc::new(HnswAdapter::new(
        dim as u32,
        metric,
        HnswConfig {
            max_elements: n + 1_000,
            m: 16,
            max_layer: 16,
            ef_construction: 200,
            ef_search: 50,
        },
    ));
    rt.block_on(hnsw.upsert_batch(&batch)).unwrap();

    // ── Build BruteForce (n=10K only; see module docs) ─────────────────────
    // `BruteForceAdapter::new` spawns an actor task via `tokio::spawn`, which
    // needs a runtime CONTEXT (not just `block_on`) — so construct + feed +
    // settle it all inside `block_on`. Bound to a local so its drop (actor
    // shutdown) happens at cell end, not mid-group. `Option<Arc<...>>` keeps
    // the borrow checker happy when we skip it at n=100K.
    let brute: Option<Arc<BruteForceAdapter>> = if n <= 10_000 {
        let a = rt.block_on(async {
            let a = Arc::new(BruteForceAdapter::new(dim as u32, metric));
            a.upsert_batch(&batch).await.unwrap();
            // Bounded channel + per-drained-batch publish; one short sleep is
            // enough for the actor to coalesce. Mirrors the legacy bench's
            // 100ms settle and the brute-force actor's coalesce-on-drain.
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            // Verify the snapshot is populated before we start timing —
            // avoids measuring an empty index if the actor is slow.
            for _ in 0..50 {
                if a.len() == n {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            a
        });
        Some(a)
    } else {
        None
    };

    let cell = format!("n{n}_d{dim}_{}", metric_name(metric));

    group.throughput(Throughput::Elements(1));
    group.bench_with_input(BenchmarkId::new("hnsw", &cell), &n, |b, _| {
        let hnsw = Arc::clone(&hnsw);
        let q = query.clone();
        b.to_async(rt).iter(move || {
            let hnsw = Arc::clone(&hnsw);
            let q = q.clone();
            async move { hnsw.search(&q, TOP_K, None).await.unwrap() }
        });
    });

    if let Some(bf) = brute {
        group.bench_with_input(BenchmarkId::new("brute_force", &cell), &n, |b, _| {
            let bf = Arc::clone(&bf);
            let q = query.clone();
            b.to_async(rt).iter(move || {
                let bf = Arc::clone(&bf);
                let q = q.clone();
                async move { bf.search(&q, TOP_K, None).await.unwrap() }
            });
        });
    }
}

fn metric_name(m: VectorMetric) -> &'static str {
    match m {
        VectorMetric::Cosine => "cosine",
        VectorMetric::L2 => "l2",
        VectorMetric::Dot => "dot",
    }
}

fn bench_vector(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("vector_search");
    // QUICK by default (sample=10, measurement=500ms, warm_up=500ms). The
    // `120s` wall-guard caps the FULL-mode worst case (sample × measurement);
    // in SMOKE/QUICK the tier constants already dominate.
    bu::tune_tiered(&mut group, 100, 5, 3, 120);

    let dims: &[usize] = &[128, 768];
    let metrics: &[VectorMetric] = &[VectorMetric::Cosine, VectorMetric::L2];

    for &n in &ladder() {
        for &dim in dims {
            for &metric in metrics {
                bench_cell(&mut group, &rt, n, dim, metric);
            }
        }
    }

    group.finish();
}

criterion_group!(benches, bench_vector);
criterion_main!(benches);
