//! Bulk-load and compaction benchmarks: `upsert_batch` vs serial `upsert`,
//! and rebuild-aside compaction cost at varying tombstone fractions.
//!
//! V4.3 (#409) — proves the two P4 capabilities:
//!
//! - **bulk-load** (`vector_bulk_load`): `upsert_batch` (one rayon
//!   `parallel_insert`) vs N serial `upsert` on the same dataset. DoD target:
//!   batch ≥5× faster than serial.
//! - **compaction** (`vector_compaction`): build an index of n=10_000, tombstone
//!   a fraction d ∈ {0.3, 0.5} via `delete`, then measure rebuild-aside cost —
//!   collect the live-set and build a fresh adapter from it via `upsert_batch`.
//!   This models the hot path of `run_background_compaction` (Steps 3–4:
//!   `collect_live_vectors` + `backfill_if_absent`) at the adapter level,
//!   without the VectorBackend/Store plumbing that the full compaction task
//!   carries.
//!
//! ## Compaction modelling note
//!
//! The rebuild-aside primitives (`collect_live_vectors`, `new_compaction_target`,
//! `backfill_if_absent`) are `pub(crate)` in `shamir-index` — the bench (an
//! external consumer via `shamir-engine`) cannot call them directly. Since the
//! bench drives the dataset with a deterministic seed, it knows exactly which
//! rids were deleted, so it assembles the live-set (original batch minus
//! tombstoned rids) and feeds it to a fresh `HnswAdapter` via the public
//! `upsert_batch`. The work performed — O(live) graph inserts in one
//! `parallel_insert` — is identical to what `backfill_if_absent` does on the
//! live-set collected from a dirty adapter; the cost measured here is a faithful
//! proxy for the compaction rebuild step.
//!
//! ## Data — clustered, not uniform
//!
//! Same generator as `vector_search.rs`: `clustered_vectors(n, dim, k=64,
//! sigma=0.1, seed=42)`. Reproducibility key: `(n, dim, k=64, σ=0.1, seed=42)`.
//!
//! ## Tuning
//!
//! `tune_tiered` + wall-guard on every group (QUICK-default). The compaction
//! group's per-iter cost (rebuild-aside of thousands of vectors) is bounded by
//! the wall-guard so a QUICK cell stays in the seconds range.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use shamir_bench_utils as bu;
use shamir_bench_utils::vector_data::clustered_vectors;
use shamir_collections::TFxSet;
use shamir_engine::index2::kind::VectorMetric;
use shamir_engine::index2::vector::adapter::VectorAdapter;
use shamir_engine::index2::vector::hnsw_adapter::{HnswAdapter, HnswConfig};
use shamir_types::types::record_id::RecordId;

/// Fixed dataset seed — part of the reproducibility key
/// `(n, dim, k=64, σ=0.1, seed=42)`.
const SEED: u64 = 42;
/// Cluster count for the clustered generator.
const K_CLUSTERS: usize = 64;
/// Per-dimension Gaussian noise σ around each centroid.
const SIGMA: f32 = 0.1;
/// Fixed dimensionality for both groups (128 ≈ small embedding).
const DIM: usize = 128;
/// Compaction group base index size.
const COMPACTION_N: usize = 10_000;
/// Tombstone fractions for the compaction group.
const TOMBSTONE_FRACTIONS: &[f64] = &[0.3, 0.5];
/// Bulk-load n-ladder.
const BULK_LADDER: &[usize] = &[1_000, 10_000];

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

/// Deterministic LCG to pick `count` distinct indices from `[0, n)` for
/// tombstoning. Same Numerical-Recipes constants as the bench-utils `Lcg` so
/// the deleted set is reproducible across runs.
fn pick_deleted(n: usize, count: usize, seed: u64) -> Vec<usize> {
    let mut state = seed;
    let mut chosen: Vec<usize> = Vec::with_capacity(count);
    let mut seen: TFxSet<usize> = shamir_collections::new_fx_set_wc(count);
    while chosen.len() < count {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let idx = (state >> 33) as usize % n;
        if seen.insert(idx) {
            chosen.push(idx);
        }
    }
    chosen
}

fn hnsw_config(capacity: usize) -> HnswConfig {
    HnswConfig {
        max_elements: capacity + 1_000,
        m: 16,
        max_layer: 16,
        ef_construction: 200,
        ef_search: 50,
    }
}

// ─── Group 1: bulk-load (serial vs batch) ───────────────────────────────────

fn bench_bulk_load(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("vector_bulk_load");
    // QUICK-default. The serial n=10K cell is the slowest (10K serial upserts);
    // wall-guard 120s keeps even a FULL worst case bounded.
    bu::tune_tiered(&mut group, 100, 5, 3, 120);

    for &n in BULK_LADDER {
        let ds = clustered_vectors(n, DIM, K_CLUSTERS, SIGMA, SEED);
        let batch: Vec<(RecordId, Vec<f32>)> = ds
            .vectors
            .iter()
            .enumerate()
            .map(|(i, v)| (rid_from(i), v.clone()))
            .collect();
        let cell = format!("n{n}");

        group.throughput(Throughput::Elements(n as u64));

        // Serial: N individual upsert calls (each its own spawn_blocking graph
        // insert). This is the pre-V0.2 path the batch replaces.
        group.bench_with_input(BenchmarkId::new("serial", &cell), &n, |b, &n| {
            b.to_async(&rt).iter(|| {
                let batch = batch.clone();
                async move {
                    let adapter =
                        HnswAdapter::new(DIM as u32, VectorMetric::Cosine, hnsw_config(n));
                    for (rid, vec) in &batch {
                        adapter.upsert(*rid, vec).await.unwrap();
                    }
                    adapter
                }
            });
        });

        // Batch: ONE upsert_batch call (single rayon parallel_insert over the
        // whole dataset). This is the V0.2 fast path.
        group.bench_with_input(BenchmarkId::new("batch", &cell), &n, |b, &n| {
            b.to_async(&rt).iter(|| {
                let batch = batch.clone();
                async move {
                    let adapter =
                        HnswAdapter::new(DIM as u32, VectorMetric::Cosine, hnsw_config(n));
                    adapter.upsert_batch(&batch).await.unwrap();
                    adapter
                }
            });
        });
    }

    group.finish();
}

// ─── Group 2: compaction rebuild-aside cost ─────────────────────────────────

fn bench_compaction(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("vector_compaction");
    // QUICK-default. Each iter rebuilds an adapter from the live-set (up to 7K
    // vectors at d=0.3); wall-guard 120s bounds the worst case.
    bu::tune_tiered(&mut group, 100, 5, 3, 120);

    let n = COMPACTION_N;
    let ds = clustered_vectors(n, DIM, K_CLUSTERS, SIGMA, SEED);
    let batch: Vec<(RecordId, Vec<f32>)> = ds
        .vectors
        .iter()
        .enumerate()
        .map(|(i, v)| (rid_from(i), v.clone()))
        .collect();

    for &frac in TOMBSTONE_FRACTIONS {
        let n_deleted = (frac * n as f64).round() as usize;
        let n_live = n - n_deleted;
        // Deterministic set of rids to tombstone.
        let deleted_idxs = pick_deleted(n, n_deleted, SEED + 700);
        debug_assert_eq!(deleted_idxs.len(), n_deleted);

        // Pre-build the live-set ONCE: this is the data that
        // `collect_live_vectors` would return from the dirty adapter. The bench
        // measures the cost of building a fresh adapter from it — the hot path
        // of `backfill_if_absent`.
        let deleted_set: TFxSet<usize> = deleted_idxs.iter().copied().collect();
        let live_set: Vec<(RecordId, Vec<f32>)> = batch
            .iter()
            .enumerate()
            .filter(|(i, _)| !deleted_set.contains(i))
            .map(|(_, (rid, vec))| (*rid, vec.clone()))
            .collect();
        debug_assert_eq!(live_set.len(), n_live);

        let cell = format!("d{}/n{n}", (frac * 100.0) as u32);
        group.throughput(Throughput::Elements(n_live as u64));

        // The measured operation: build a fresh adapter (compaction target)
        // from the live-set via upsert_batch. This is the rebuild-aside cost —
        // graph construction over the surviving vectors.
        group.bench_with_input(
            BenchmarkId::new("rebuild_aside", &cell),
            &(frac, n_live),
            |b, &(_, n_live)| {
                let live_set = live_set.clone();
                b.to_async(&rt).iter(move || {
                    let live_set = live_set.clone();
                    async move {
                        let target =
                            HnswAdapter::new(DIM as u32, VectorMetric::Cosine, hnsw_config(n_live));
                        target.upsert_batch(&live_set).await.unwrap();
                        target
                    }
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_bulk_load, bench_compaction);
criterion_main!(benches);
