//! Cold-start recovery bench: `load_snapshot` vs full-scan `rebuild` (P2 — V2.4).
//!
//! V0.4's `vector_report` measures search QUALITY (recall/RSS/build). This
//! tool measures a SEPARATE axis: how long it takes to RE-ACQUIRE a working
//! graph after a restart, by the two paths `VectorBackend::restore_on_open`
//! can take:
//!
//! - **load** — a valid snapshot exists → `load_snapshot` rebuilds the graph
//!   from the dumped chunks+sidecar. `rebuild_count == 0`. O(dump size),
//!   no data-store scan.
//! - **rebuild** — no snapshot (or a corrupt one) → `rebuild` scans every
//!   row in the data store and re-inserts each vector into a fresh graph.
//!   `rebuild_count == 1`. O(rows × dim).
//!
//! The point of the persisted-HNSW work (P2 / V2.1–V2.3) is to make a warm
//! restart NOT pay the full-scan cost. This bench quantifies the win.
//!
//! # Determinism
//!
//! Dataset is the shared LCG lineage (`shamir_bench_utils::vector_data`);
//! the only non-determinism is `hnsw_rs`'s unseedable layer-assignment RNG,
//! which affects build wall-time by a few % (acceptable for an order-of-
//! magnitude comparison).
//!
//! # Run
//!
//! Perimeter guard blocks `cargo run`; build then invoke the artefact:
//!
//! ```text
//! cargo build --release --example persisted_hnsw
//! ./target/release/examples/persisted_hnsw
//! ```
//!
//! Env knobs (all optional):
//! - `PH_N`     — point count (default 100_000; QUICK tier).
//! - `PH_DIM`   — dimension (default 128).
//! - `PH_N_1M`  — set to `1` to also run n=1_000_000 (long run, env-gated).
//!
//! Output: a self-contained markdown block ready to paste into
//! `docs/benchmarks/vector/<date>-persisted-hnsw.md`.

use std::sync::Arc;
use std::time::Instant;

use shamir_bench_utils::vector_data::clustered_vectors;
use shamir_engine::index2::backend::IndexBackend;
use shamir_engine::index2::descriptor::IndexDescriptor;
use shamir_engine::index2::kind::{IndexKind, VectorBackendRef, VectorConfig, VectorMetric};
use shamir_engine::index2::vector::adapter::VectorAdapter;
use shamir_engine::index2::vector::hnsw_adapter::{HnswAdapter, HnswConfig};
use shamir_engine::index2::vector::snapshot;
use shamir_engine::index2::vector::vector_backend::VectorBackend;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{KvOp, Store};
use shamir_types::core::interner::{Interner, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use smallvec::SmallVec;

/// HNSW graph parameters — mirror `vector_report` so the two tools describe
/// the SAME index shape (only the measured axis differs).
const M: usize = 16;
const MAX_LAYER: usize = 16;
const EF_CONSTRUCTION: usize = 200;
const EF_SEARCH: usize = 50;

// ── runtime / helpers ──────────────────────────────────────────────

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

fn rid_from(i: usize) -> RecordId {
    let mut a = [0u8; 16];
    a[8..16].copy_from_slice(&(i as u64).to_be_bytes());
    RecordId(a)
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// UTC date `YYYY-MM-DD` via the civil-from-days algorithm (Howard Hinnant).
/// Pure std; mirrors `vector_report::chrono_like_date`.
fn chrono_like_date() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn host_line() -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let cpus = std::thread::available_parallelism()
        .map(|p| p.get().to_string())
        .unwrap_or_else(|_| "?".into());
    format!("{os}/{arch}, {cpus} threads")
}

fn intern(i: &Interner, s: &str) -> u64 {
    match i.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

/// Build a `VectorBackend` (the same shape the engine constructs on open)
/// with an EMPTY adapter — `restore_on_open` is what fills it.
fn make_backend(interner: &Interner, id: u32, dim: u32, metric: VectorMetric) -> VectorBackend {
    let desc = IndexDescriptor::new(
        id,
        format!("vec_idx_{id}"),
        intern(interner, &format!("vec_idx_{id}")),
        SmallVec::new(),
        IndexKind::Vector(Box::new(VectorConfig {
            dim,
            metric,
            backend: VectorBackendRef::InProcessHnsw {
                ef_construct: EF_CONSTRUCTION as u32,
                m: M as u32,
            },
        })),
    );
    let adapter: Arc<dyn VectorAdapter> = Arc::new(HnswAdapter::new(
        dim,
        metric,
        HnswConfig {
            max_elements: 1_100_000,
            m: M,
            max_layer: MAX_LAYER,
            ef_construction: EF_CONSTRUCTION,
            ef_search: EF_SEARCH,
        },
    ));
    let embedding_key = intern(interner, "embedding");
    VectorBackend::new(desc, vec![embedding_key], adapter)
}

/// Encode a vector as the `{embedding: [f64; dim]}` record shape that
/// `VectorBackend::rebuild`'s `extract_vec` pulls out of the data store.
fn encode_rec(interner: &Interner, v: &[f32]) -> bytes::Bytes {
    use shamir_types::core::interner::InternerKey;
    let mut m = new_map_wc(1);
    m.insert(
        InternerKey::new(intern(interner, "embedding")),
        InnerValue::List(v.iter().map(|f| InnerValue::F64(*f as f64)).collect()),
    );
    InnerValue::Map(m).to_bytes().expect("record encodes")
}

// ── the measured cell ──────────────────────────────────────────────

struct ColdStartMetrics {
    n: usize,
    dim: usize,
    metric: VectorMetric,
    /// Wall-time of `load_snapshot` path (restore_on_open with a valid
    /// snapshot). Seconds.
    load_secs: f64,
    /// Wall-time of the full-scan `rebuild` path (restore_on_open with no
    /// snapshot). Seconds.
    rebuild_secs: f64,
    /// Speedup = rebuild / load. >1 means the snapshot load is faster.
    speedup: f64,
    /// Whether the load path proved it did NOT scan (rebuild_count == 0).
    load_skipped_scan: bool,
}

/// Run one `(n, dim, metric)` cell: build a graph, dump it, seed a data
/// store, then time the two cold-start paths.
fn run_cell(
    rt: &tokio::runtime::Runtime,
    n: usize,
    dim: usize,
    metric: VectorMetric,
) -> ColdStartMetrics {
    let interner = Interner::new();
    let keyspace = format!("__vec_snap__{}", 1u32);
    let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    // ── dataset ────────────────────────────────────────────────────
    let ds = clustered_vectors(n, dim, 64, 0.1, 42);
    let batch: Vec<(RecordId, Vec<f32>)> = ds
        .vectors
        .iter()
        .enumerate()
        .map(|(i, v)| (rid_from(i), v.clone()))
        .collect();

    // ── build a live graph via one batched upsert, then dump it ─────
    // The dump is the artefact the load path will read.
    let live = HnswAdapter::new(
        dim as u32,
        metric,
        HnswConfig {
            max_elements: n + 1_000,
            m: M,
            max_layer: MAX_LAYER,
            ef_construction: EF_CONSTRUCTION,
            ef_search: EF_SEARCH,
        },
    );
    rt.block_on(live.upsert_batch(&batch)).expect("hnsw build");
    rt.block_on(snapshot::dump_snapshot(&live, &info_store, &keyspace))
        .expect("dump");

    // ── seed the data store with the SAME vectors (the rebuild scan's
    //    input). Batched transact keeps this fast.
    let mut ops: Vec<KvOp> = Vec::with_capacity(n);
    for (r, v) in &batch {
        let val = encode_rec(&interner, v);
        ops.push(KvOp::Set(r.0.to_vec().into(), val));
    }
    // InMemoryStore::transact default-impl applies ops sequentially; chunk
    // to keep the Vec bounded for very large n.
    for chunk in ops.chunks(10_000) {
        rt.block_on(data_store.transact(chunk.to_vec()))
            .expect("data seed");
    }

    // ── PATH 1: load (valid snapshot → load_snapshot) ───────────────
    let backend_load = make_backend(&interner, 1, dim as u32, metric);
    let load_start = Instant::now();
    rt.block_on(backend_load.restore_on_open(Arc::clone(&info_store), Arc::clone(&data_store)))
        .expect("restore_on_open (load)");
    let load_secs = load_start.elapsed().as_secs_f64();
    let load_skipped_scan = backend_load.rebuild_count() == 0;

    // ── PATH 2: rebuild (no snapshot → full-scan rebuild) ───────────
    // A FRESH info store (no manifest) forces the NotFound branch.
    let fresh_info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let backend_rebuild = make_backend(&interner, 1, dim as u32, metric);
    let rebuild_start = Instant::now();
    rt.block_on(backend_rebuild.restore_on_open(Arc::clone(&fresh_info), Arc::clone(&data_store)))
        .expect("restore_on_open (rebuild)");
    let rebuild_secs = rebuild_start.elapsed().as_secs_f64();
    assert_eq!(
        backend_rebuild.rebuild_count(),
        1,
        "rebuild path must have scanned (rebuild_count == 1)"
    );

    let speedup = if load_secs > 0.0 {
        rebuild_secs / load_secs
    } else {
        f64::INFINITY
    };

    ColdStartMetrics {
        n,
        dim,
        metric,
        load_secs,
        rebuild_secs,
        speedup,
        load_skipped_scan,
    }
}

fn metric_name(m: VectorMetric) -> &'static str {
    match m {
        VectorMetric::Cosine => "cosine",
        VectorMetric::L2 => "l2",
        VectorMetric::Dot => "dot",
    }
}

fn print_report(cells: &[ColdStartMetrics]) {
    let now = chrono_like_date();
    let n = cells.first().map(|c| c.n).unwrap_or(0);
    let dim = cells.first().map(|c| c.dim).unwrap_or(0);
    println!("<!-- persisted_hnsw — paste into docs/benchmarks/vector/{now}-persisted-hnsw.md -->");
    println!();
    println!("## Persisted HNSW cold-start — {now}");
    println!();
    println!(
        "- **Tool**: `persisted_hnsw` example binary, V2.4 (build with cargo, \
         run the artefact directly — the perimeter guard blocks `cargo run`)"
    );
    println!(
        "- **Dataset**: `clustered_vectors` — n={n}, dim={dim}, k_clusters=64, σ=0.1, seed=42"
    );
    println!(
        "- **HNSW**: M={M}, max_layer={MAX_LAYER}, ef_construct={EF_CONSTRUCTION}, ef_search={EF_SEARCH}"
    );
    println!(
        "- **Metrics**: {}",
        cells
            .iter()
            .map(|c| metric_name(c.metric))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("- **Host**: {host}", host = host_line());
    println!();
    println!("### Cold-start wall-time: `load_snapshot` vs full-scan `rebuild`");
    println!();
    println!("| n | dim | metric | load (s) | rebuild (s) | speedup | load skipped scan? |");
    println!("|---:|----:|:-------|---------:|------------:|--------:|:-------------------|");
    for c in cells {
        println!(
            "| {} | {} | {} | {:.3} | {:.3} | {:.2}× | {} |",
            c.n,
            c.dim,
            metric_name(c.metric),
            c.load_secs,
            c.rebuild_secs,
            c.speedup,
            if c.load_skipped_scan { "yes" } else { "NO" },
        );
    }
    println!();
    println!(
        "- **DoD P2**: restart of a {n}-row index without a full data-store scan — \
         **{}** (load path rebuild_count == 0 for every cell).",
        if cells.iter().all(|c| c.load_skipped_scan) {
            "MET"
        } else {
            "NOT MET"
        }
    );
    println!(
        "- **Savings**: the load path skips scanning all {n} rows; on this host it is \
         ~{:.1}× faster than the rebuild scan at dim={dim}.",
        cells.iter().map(|c| c.speedup).fold(0f64, f64::max)
    );
    println!();
    println!("- **Reproducibility key**:");
    println!("  - `cargo build --release --example persisted_hnsw`");
    println!("  - `./target/release/examples/persisted_hnsw` (QUICK: n={n}, dim={dim})");
    println!("  - `PH_N=1000000 PH_DIM=128 ./target/release/examples/persisted_hnsw` for 1M (long; env-gated via `PH_N_1M=1`)");
}

// ── main ───────────────────────────────────────────────────────────

fn main() {
    let n = env_usize("PH_N", 100_000);
    let dim = env_usize("PH_DIM", 128);
    let metrics = [VectorMetric::Cosine, VectorMetric::L2];

    let runtime = rt();
    let mut cells: Vec<ColdStartMetrics> = Vec::with_capacity(metrics.len());

    for &metric in &metrics {
        eprintln!(
            "persisted_hnsw: n={n} dim={dim} metric={} ...",
            metric_name(metric)
        );
        let c = run_cell(&runtime, n, dim, metric);
        eprintln!(
            "  → load={:.3}s rebuild={:.3}s speedup={:.2}× skipped_scan={}",
            c.load_secs, c.rebuild_secs, c.speedup, c.load_skipped_scan
        );
        cells.push(c);
    }

    println!();
    print_report(&cells);

    // Optional 1M rung — appended only when explicitly opted in.
    if std::env::var("PH_N_1M")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
    {
        let big_n = 1_000_000;
        let mut big: Vec<ColdStartMetrics> = Vec::with_capacity(metrics.len());
        for &metric in &metrics {
            eprintln!(
                "persisted_hnsw: PH_N_1M=1 — n={big_n} dim={dim} metric={} (long run)",
                metric_name(metric)
            );
            let c = run_cell(&runtime, big_n, dim, metric);
            eprintln!(
                "  → load={:.3}s rebuild={:.3}s speedup={:.2}× skipped_scan={}",
                c.load_secs, c.rebuild_secs, c.speedup, c.load_skipped_scan
            );
            big.push(c);
        }
        println!();
        println!("### 1M rung (long; `PH_N_1M=1`)");
        println!();
        println!("| n | dim | metric | load (s) | rebuild (s) | speedup | load skipped scan? |");
        println!("|---:|----:|:-------|---------:|------------:|--------:|:-------------------|");
        for c in &big {
            println!(
                "| {} | {} | {} | {:.3} | {:.3} | {:.2}× | {} |",
                c.n,
                c.dim,
                metric_name(c.metric),
                c.load_secs,
                c.rebuild_secs,
                c.speedup,
                if c.load_skipped_scan { "yes" } else { "NO" },
            );
        }
    }
}
