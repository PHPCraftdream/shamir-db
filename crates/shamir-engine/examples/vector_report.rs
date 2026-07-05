//! Vector-index QUALITY + RSS report (V0.4 of the bench foundation).
//!
//! Complements the criterion latency bench (`benches/vector_search.rs`,
//! V0.3). Criterion is the wrong tool for the metrics here:
//!
//! - **Recall** is deterministic — there is no measurement noise to
//!   average out, so a criterion sample loop adds nothing. We just need
//!   the exact top-k vs the HNSW top-k for a fixed query set.
//! - **RSS** is the OS resident-set size, not anything criterion's
//!   pluggable `Measurement` knows about (it times closures). We sample
//!   the process's own RSS via `memory-stats` (procfs on Linux, mach
//!   task info on macOS, `GetProcessMemoryInfo` on Windows).
//!
//! So this is an **example binary**, not a test or a criterion bench.
//!
//! # What it prints
//!
//! A self-contained markdown block ready to paste into
//! `docs/benchmarks/vector/<date>-baseline.md`:
//!
//! - A **reproducibility header** (dataset params, seed, host info) per
//!   the Release Benchmark Checklist in the roadmap doc.
//! - A **markdown table** with one row per `(dim, metric)` cell:
//!   `recall@1`, `recall@10`, build time (wall), and peak RSS.
//!
//! # Ground truth
//!
//! For each query we compute the EXACT top-k via `BruteForceAdapter`
//! (lock-free snapshot, SIMD-scanned — the same adapter production
//! uses as a fallback for small indexes). Recall = |HNSW ∩ exact| / k.
//! We score a fixed set of ~100 seeded queries drawn from the same LCG
//! lineage as the dataset (seed offset so queries are distinct from
//! every stored point) — deterministic across runs.
//!
//! # Determinism
//!
//! No global RNG. The dataset comes from `clustered_vectors` (LCG); the
//! query set comes from a second `clustered_vectors` call with a
//! distinct seed. The ONLY non-determinism is `hnsw_rs`'s internal
//! layer-assignment RNG, which is not seedable — that is exactly the
//! nondeterminism whose recall impact this tool exists to surface.
//!
//! # Run
//!
//! ```text
//! cargo run --release --example vector_report
//! ```
//!
//! Env knobs (all optional):
//! - `VR_N`        — point count (default 10_000).
//! - `VR_DIMS`     — comma-separated dims (default `128,768`).
//! - `VR_QUERIES`  — query count for recall (default 100).
//! - `VR_SEED`     — dataset seed (default 42).
//! - `VR_K_CLUSTERS` — cluster count (default 64).
//! - `VR_SIGMA`    — cluster spread (default 0.1).
//! - `VR_N_1M`     — set to `1` to also run n=1_000_000 (long run).

use std::sync::Arc;
use std::time::Instant;

use memory_stats::memory_stats;
use shamir_bench_utils::vector_data::clustered_vectors;
use shamir_engine::index2::kind::VectorMetric;
use shamir_engine::index2::vector::adapter::VectorAdapter;
use shamir_engine::index2::vector::brute_force::BruteForceAdapter;
use shamir_engine::index2::vector::hnsw_adapter::{HnswAdapter, HnswConfig};
use shamir_types::types::record_id::RecordId;

/// HNSW graph parameters — mirror the V0.3 criterion bench so the two
/// tools describe the SAME index shape (only the measurement differs).
const M: usize = 16;
const MAX_LAYER: usize = 16;
const EF_CONSTRUCTION: usize = 200;
const EF_SEARCH: usize = 50;
/// top-k depth for recall@10.
const TOP_K: u32 = 10;

// ── runtime / helpers ──────────────────────────────────────────────

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

/// Deterministic `RecordId` from a `usize` index — same encoding the
/// V0.3 bench uses, so rids line up across the two tools.
fn rid_from(i: usize) -> RecordId {
    let mut a = [0u8; 16];
    a[8..16].copy_from_slice(&(i as u64).to_be_bytes());
    RecordId(a)
}

fn metric_name(m: VectorMetric) -> &'static str {
    match m {
        VectorMetric::Cosine => "cosine",
        VectorMetric::L2 => "l2",
        VectorMetric::Dot => "dot",
    }
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn env_f32(name: &str, default: f32) -> f32 {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// Parse a comma-separated list of dims from `VR_DIMS`.
fn env_dims(default: &[usize]) -> Vec<usize> {
    match std::env::var("VR_DIMS") {
        Ok(s) => s
            .split(',')
            .filter_map(|t| t.trim().parse().ok())
            .filter(|&d| d > 0)
            .collect::<Vec<_>>(),
        Err(_) => default.to_vec(),
    }
}

// ── RSS ────────────────────────────────────────────────────────────

/// One RSS sample (bytes). Returns `None` if the OS read fails — we
/// surface that as `n/a` in the report rather than aborting, per the
/// brief (RSS is order-of-magnitude; a missing sample is not fatal).
fn rss_now() -> Option<usize> {
    memory_stats().map(|m| m.physical_mem)
}

/// Format bytes as a human-friendly KiB/MiB string for the report.
fn fmt_bytes(b: usize) -> String {
    let kib = b as f64 / 1024.0;
    if kib >= 1024.0 {
        format!("{:.1} MiB", kib / 1024.0)
    } else {
        format!("{:.0} KiB", kib)
    }
}

// ── recall ─────────────────────────────────────────────────────────

/// `|a ∩ b|` for two slices of `RecordId`, treating each as a set.
/// Both slices are small (k ≤ 10) so an O(k²) intersection is cheaper
/// than building a HashSet.
fn intersection_size(a: &[RecordId], b: &[RecordId]) -> usize {
    let mut count = 0;
    for x in a {
        if b.iter().any(|y| y == x) {
            count += 1;
        }
    }
    count
}

/// Per-cell metrics gathered for the report table.
struct CellMetrics {
    dim: usize,
    metric: VectorMetric,
    recall_at_1: f64,
    recall_at_10: f64,
    build_secs: f64,
    rss: Option<usize>,
}

/// Dataset + query parameters shared between cell runs and the report
/// header. Bundling them keeps `run_cell` / `print_report` under the
/// clippy argument-count limit and gives a single source of truth for
/// the reproducibility key.
struct DatasetParams {
    n: usize,
    k_clusters: usize,
    sigma: f32,
    seed: u64,
    n_queries: usize,
}

/// Build HNSW + BruteForce over a clustered dataset, then score recall
/// against a seeded query set.
///
/// Returns `None` only if an adapter op fails — that indicates a real
/// bug, surfaced as a panic in the caller (this is a bench tool).
#[allow(clippy::too_many_lines)] // linear report pipeline; splitting muddies it
fn run_cell(
    rt: &tokio::runtime::Runtime,
    p: &DatasetParams,
    dim: usize,
    metric: VectorMetric,
) -> CellMetrics {
    let DatasetParams {
        n,
        k_clusters,
        sigma,
        seed,
        n_queries,
    } = *p;
    // ── dataset (shared V0.1 generator) ────────────────────────────
    let ds = clustered_vectors(n, dim, k_clusters, sigma, seed);
    debug_assert_eq!(ds.n(), n);
    debug_assert_eq!(ds.dim(), dim);

    let batch: Vec<(RecordId, Vec<f32>)> = ds
        .vectors
        .iter()
        .enumerate()
        .map(|(i, v)| (rid_from(i), v.clone()))
        .collect();

    // ── query set: distinct lineage (seed+1), same params ──────────
    let queries: Vec<Vec<f32>> =
        clustered_vectors(n_queries, dim, k_clusters, sigma, seed + 1).vectors;

    // ── ground truth: BruteForceAdapter (exact KNN, SIMD-scanned) ──
    // Built and settled inside block_on (its actor needs a runtime ctx).
    let brute = Arc::new(rt.block_on(async {
        let a = BruteForceAdapter::new(dim as u32, metric);
        a.upsert_batch(&batch).await.expect("brute upsert");
        // Bounded channel + per-drained-batch publish; let the actor
        // coalesce the whole batch into one snapshot. Mirrors the
        // settle loop in the V0.3 bench.
        for _ in 0..100 {
            if a.len() == n {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(a.len(), n, "brute-force actor did not settle");
        a
    }));

    // ── build HNSW via ONE batched upsert; time the wall ───────────
    let rss_pre = rss_now();
    let hnsw = Arc::new(HnswAdapter::new(
        dim as u32,
        metric,
        HnswConfig {
            max_elements: n + 1_000,
            m: M,
            max_layer: MAX_LAYER,
            ef_construction: EF_CONSTRUCTION,
            ef_search: EF_SEARCH,
        },
    ));
    let build_start = Instant::now();
    rt.block_on(hnsw.upsert_batch(&batch)).expect("hnsw upsert");
    let build_secs = build_start.elapsed().as_secs_f64();
    // Peak RSS is sampled AFTER the build (graph + retained vectors are
    // all live); we take the max of a pre/post pair to catch the build
    // high-water mark on platforms where RSS is monotonic.
    let rss_post = rss_now();
    let rss = match (rss_pre, rss_post) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    };

    // ── score recall@1 and recall@10 over the query set ────────────
    rt.block_on(async {
        let mut hits1 = 0usize;
        let mut hits10 = 0usize;
        for q in &queries {
            let exact = brute.search(q, TOP_K, None).await.expect("brute search");
            let approx = hnsw.search(q, TOP_K, None).await.expect("hnsw search");

            let exact_rids: Vec<RecordId> = exact.iter().map(|(r, _)| *r).collect();
            let approx_rids: Vec<RecordId> = approx.iter().map(|(r, _)| *r).collect();

            // recall@1: does HNSW's single nearest match exact's nearest?
            if !exact_rids.is_empty() && !approx_rids.is_empty() && exact_rids[0] == approx_rids[0]
            {
                hits1 += 1;
            }
            // recall@10: |HNSW-top10 ∩ exact-top10| / 10.
            hits10 += intersection_size(&approx_rids, &exact_rids);
        }
        let q = n_queries as f64;
        CellMetrics {
            dim,
            metric,
            recall_at_1: hits1 as f64 / q,
            recall_at_10: hits10 as f64 / (q * TOP_K as f64),
            build_secs,
            rss,
        }
    })
}

// ── report ─────────────────────────────────────────────────────────

fn host_line() -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let cpus = std::thread::available_parallelism()
        .map(|p| p.get().to_string())
        .unwrap_or_else(|_| "?".into());
    format!("{os}/{arch}, {cpus} threads")
}

/// Print the full markdown report (header + table) for the gathered cells.
fn print_report(
    p: &DatasetParams,
    dims: &[usize],
    metrics: &[VectorMetric],
    cells: &[CellMetrics],
) {
    let now = chrono_like_date();
    println!("<!-- vector_report — paste into docs/benchmarks/vector/{now}-baseline.md -->");
    println!();
    println!("## Vector baseline — {now}");
    println!();
    println!(
        "- **Tool**: `vector_report` example binary, V0.4 (build with cargo, \
         run the artefact directly — the perimeter guard blocks `cargo run`)"
    );
    println!(
        "- **Dataset**: `clustered_vectors` — n={}, dims={dims:?}, k_clusters={}, σ={}, seed={}",
        p.n, p.k_clusters, p.sigma, p.seed
    );
    println!("- **Queries**: {} (seed={})", p.n_queries, p.seed + 1);
    println!("- **HNSW**: M={M}, max_layer={MAX_LAYER}, ef_construct={EF_CONSTRUCTION}, ef_search={EF_SEARCH}");
    println!("- **top-k**: {TOP_K}");
    println!(
        "- **Metrics**: {metrics}",
        metrics = metrics
            .iter()
            .map(|m| metric_name(*m))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("- **Host**: {host}", host = host_line());
    println!("- **RSS**: order-of-magnitude via `memory-stats` (peak of pre/post-build sample)");
    println!();
    println!("| dim | metric | recall@1 | recall@10 | build (s) | peak RSS |");
    println!("|----:|:-------|--------:|----------:|----------:|---------:|");
    for c in cells {
        let rss = c.rss.map(fmt_bytes).unwrap_or_else(|| "n/a".to_string());
        println!(
            "| {} | {} | {:.3} | {:.3} | {:.2} | {} |",
            c.dim,
            metric_name(c.metric),
            c.recall_at_1,
            c.recall_at_10,
            c.build_secs,
            rss,
        );
    }
}

/// UTC date `YYYY-MM-DD` without pulling in a date crate — std only.
fn chrono_like_date() -> String {
    // Days since the Unix epoch (UTC). Pure std: count whole days from
    // `SystemTime::now()`; convert (days, 0..0) back to Y-M-D with the
    // civil-from-days algorithm (Howard Hinnant). Good enough for a
    // report filename; we do not need wall-clock time.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Howard Hinnant's `civil_from_days` — inverse of days-from-civil.
/// Returns proleptic-Gregorian (year, month [1-12], day [1-31]).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ── main ───────────────────────────────────────────────────────────

fn main() {
    let n = env_usize("VR_N", 10_000);
    let dims = env_dims(&[128, 768]);
    let n_queries = env_usize("VR_QUERIES", 100);
    let seed = env_u64("VR_SEED", 42);
    let k_clusters = env_usize("VR_K_CLUSTERS", 64);
    let sigma = env_f32("VR_SIGMA", 0.1);

    let p = DatasetParams {
        n,
        k_clusters,
        sigma,
        seed,
        n_queries,
    };
    let metrics = [VectorMetric::Cosine, VectorMetric::L2];

    let rt = rt();
    let mut cells: Vec<CellMetrics> = Vec::with_capacity(dims.len() * metrics.len());

    for &dim in &dims {
        for &metric in &metrics {
            eprintln!(
                "vector_report: n={n} dim={dim} metric={} ...",
                metric_name(metric)
            );
            let c = run_cell(&rt, &p, dim, metric);
            eprintln!(
                "  → recall@1={:.3} recall@10={:.3} build={:.2}s",
                c.recall_at_1, c.recall_at_10, c.build_secs
            );
            cells.push(c);
        }
    }

    println!();
    print_report(&p, &dims, &metrics, &cells);

    // Optional 1M rung — appended as a separate table (with an `n` column)
    // so the long-run rows are distinguishable from the default-n rows.
    if std::env::var("VR_N_1M")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
    {
        let mut big_p = p;
        big_p.n = 1_000_000;
        let mut big: Vec<CellMetrics> = Vec::with_capacity(dims.len() * metrics.len());
        for &dim in &dims {
            for &metric in &metrics {
                eprintln!(
                    "vector_report: VR_N_1M=1 — n=1_000_000 dim={dim} metric={} (long run)",
                    metric_name(metric)
                );
                let c = run_cell(&rt, &big_p, dim, metric);
                eprintln!(
                    "  → recall@1={:.3} recall@10={:.3} build={:.2}s",
                    c.recall_at_1, c.recall_at_10, c.build_secs
                );
                big.push(c);
            }
        }
        println!();
        println!("<!-- 1M rung (VR_N_1M=1) -->");
        println!();
        println!("| n | dim | metric | recall@1 | recall@10 | build (s) | peak RSS |");
        println!("|---:|----:|:-------|--------:|----------:|----------:|---------:|");
        for c in &big {
            let rss = c.rss.map(fmt_bytes).unwrap_or_else(|| "n/a".into());
            println!(
                "| 1000000 | {} | {} | {:.3} | {:.3} | {:.2} | {} |",
                c.dim,
                metric_name(c.metric),
                c.recall_at_1,
                c.recall_at_10,
                c.build_secs,
                rss,
            );
        }
    }
}
