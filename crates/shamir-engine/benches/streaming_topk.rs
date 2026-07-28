//! F-53a (#874) — streaming top-K memory/latency proof.
//!
//! Proves the fix that moved ORDER BY + LIMIT bounding INTO the scan loop (a
//! `k = skip + take` bounded max-heap fed row-by-row during the scan) instead
//! of materialising every matched+projected row into a `Vec` before the top-K
//! trim ever ran. The pre-F-53a code comment claiming "O(K) memory" was true
//! only for the heap's own internals — the `rec_acc` Vec feeding it was O(N).
//!
//! ## Workload
//!
//! A large-N / small-K scan: N rows `{score, name}`, no sorted index (so an
//! ORDER BY query is forced through `read_collecting`'s in-memory path, never
//! an index-ordered scan), `ORDER BY score ASC LIMIT 10`.
//!
//! ## Before / after
//!
//! Two engine-level reads against the SAME table, same source bytes alive in
//! both (so the peak-allocation DELTA isolates the projected-row
//! accumulation):
//!
//!   - `streaming_topk/limit_10`   — `ORDER BY score LIMIT 10` — the F-53a
//!     inline bounded-heap path. Peak live projected rows ≈ K.
//!   - `full_materialize/no_limit` — `ORDER BY score` (no LIMIT) — the full
//!     materialisation shape the pre-F-53a `ORDER BY + LIMIT` path took
//!     internally (it accumulated the full `rec_acc` Vec before trimming).
//!     Peak live projected rows ≈ N.
//!
//! The peak-allocation ratio (≈ N/K) is the memory proof; the latency delta
//! (O(N log K) heap vs O(N log N) full sort) is the speed proof. Both are
//! real engine `tbl.read()` calls — no synthetic harness.
//!
//! Peak allocation is sampled once per path up front via
//! `shamir_bench_utils::peak_mem` (a `#[global_allocator]` wrapper that tracks
//! the max live-byte watermark); the `Harness` then owns the timed latency
//! sweep.
//!
//! Run (isolated bench target dir, per the workspace bench-cache rule):
//!   `CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p
//!    shamir-engine --bench streaming_topk`

use std::hint::black_box;
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::query::filter::eval_context::FilterContext;
use shamir_engine::query::read::{OrderBy, ReadQuery};
use shamir_engine::table::table_manager::TableManager;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::TouchInd;
use shamir_types::types::common::{new_map, new_map_wc};
use shamir_types::types::value::InnerValue;

/// Rows per table. Large enough that N projected `QueryValue` maps dominate
/// the peak watermark over scan/decode noise, small enough to stay within the
/// fixed-iteration harness's ~10ms/call budget (a full no-index scan + sort
/// at N=2000 is ~1-3ms).
const N: usize = 2000;
/// LIMIT K — deliberately tiny so the N/K memory ratio is dramatic.
const K: u64 = 10;

/// Build a plain (NO sorted index) in-memory table of N `{score, name}` rows.
/// `score` is a non-monotonic permutation so ORDER BY truly reorders.
async fn build_table() -> TableManager {
    let data: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let info: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let mgr = TableManager::create("bench_table".into(), data, info)
        .await
        .unwrap();

    let interner = mgr.interner().get().await.unwrap();
    let touch = |s: &str| match interner.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k,
    };
    let k_score = touch("score");
    let k_name = touch("name");

    let chunk_size = 1000;
    let mut batch = Vec::with_capacity(chunk_size);
    for i in 0..N {
        // Deterministic non-monotonic score (LCG-style permutation).
        let score = ((i as i64).wrapping_mul(0x5DEECE66D) ^ (i as i64 * 37)) % 10_000;
        let mut m = new_map_wc(2);
        m.insert(k_score.clone(), InnerValue::Int(score));
        m.insert(k_name.clone(), InnerValue::Str(format!("rec_{i:06x}")));
        batch.push(InnerValue::Map(m));
        if batch.len() == chunk_size || i == N - 1 {
            mgr.insert_many(&batch).await.unwrap();
            batch.clear();
        }
    }
    mgr
}

/// `ORDER BY score ASC LIMIT K` — the F-53a streaming bounded-heap path.
fn query_streaming_topk() -> ReadQuery {
    ReadQuery::new("bench_table")
        .order_by(OrderBy::asc("score"))
        .limit(K)
}

/// `ORDER BY score ASC` with NO limit — the full-materialisation shape the
/// pre-F-53a `ORDER BY + LIMIT` path took internally (full `rec_acc` Vec +
/// full sort). Used as the "before" baseline.
fn query_full_materialize() -> ReadQuery {
    ReadQuery::new("bench_table").order_by(OrderBy::asc("score"))
}

/// One-shot peak-allocation sample for a single read. Resets the
/// `peak_mem` watermark, runs the read, returns the peak live bytes observed
/// during it. The table's source bytes are alive in both samples, so the
/// DELTA between the two paths isolates the projected-row accumulation.
async fn peak_of_read(mgr: &TableManager, q: &ReadQuery) -> usize {
    shamir_bench_utils::peak_mem::reset();
    let interner = mgr.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    let res = mgr.read(q, &ctx).await.unwrap();
    let peak = shamir_bench_utils::peak_mem::current_peak();
    // Touch the result so it isn't optimised out before the peak is captured.
    black_box(res);
    peak
}

fn main() {
    // Force the peak_mem global allocator to be linked.
    shamir_bench_utils::peak_mem::setup();

    let mut h = Harness::new("streaming_topk", env!("CARGO_MANIFEST_DIR"));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    // Build the table ONCE — both the peak sample and the latency workloads
    // read from the same fixture (shared setup, per harness plan 1).
    let mgr = rt.block_on(build_table());

    // ── Peak-allocation sample (printed, not harness-timed) ───────────────
    // The whole point of F-53a: the streaming path holds O(K) projected rows
    // while the full-materialise path holds O(N). Same table, same source
    // bytes alive — the delta is the projected-row accumulation.
    let peak_streaming = rt.block_on(peak_of_read(&mgr, &query_streaming_topk()));
    let peak_full = rt.block_on(peak_of_read(&mgr, &query_full_materialize()));
    let ratio = peak_full as f64 / peak_streaming.max(1) as f64;
    eprintln!(
        "F-53a peak allocation (N={N}, K={K}): \
         streaming_topk={peak_streaming} bytes, \
         full_materialize={peak_full} bytes, \
         ratio={ratio:.1}x (full/streaming)"
    );

    // ── Latency workloads (harness-timed) ─────────────────────────────────
    // Same two reads, now timed by the fixed-iteration harness. The streaming
    // path pays O(N log K) heap work; the full path pays O(N log N) sort work
    // PLUS the O(N) materialisation. The delta is the F-53a speed win.

    {
        let mgr = mgr.clone();
        let q = query_streaming_topk();
        h.bench_async("streaming_topk/limit_10", move || {
            let mgr = mgr.clone();
            let q = q.clone();
            async move {
                let interner = mgr.interner().get().await.unwrap();
                let refs = new_map();
                let ctx = FilterContext::new(interner, &refs);
                black_box(mgr.read(&q, &ctx).await.unwrap());
            }
        });
    }

    {
        let mgr = mgr.clone();
        let q = query_full_materialize();
        h.bench_async("full_materialize/no_limit", move || {
            let mgr = mgr.clone();
            let q = q.clone();
            async move {
                let interner = mgr.interner().get().await.unwrap();
                let refs = new_map();
                let ctx = FilterContext::new(interner, &refs);
                black_box(mgr.read(&q, &ctx).await.unwrap());
            }
        });
    }

    h.run();
}
