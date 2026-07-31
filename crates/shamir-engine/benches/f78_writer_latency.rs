//! F-78 (#905) writer-latency probe — concurrent-writer p50/p95/p99 during a
//! real `TableManager::create_index` (which acquires F-70's write barrier:
//! raise `REGULAR_INDEX_CREATE` → drain → hold `unique_write_lock` across the
//! WHOLE Phase 1→2→3 sequence).
//!
//! This is a **latency-distribution probe**, not a throughput workload — it
//! runs the (create_index + concurrent writers) scenario and reports the
//! writers' observed p50/p95/p99, so it uses the harness's `bench_batched_async`
//! for a stable scenario wall-time AND prints the per-scenario percentiles.
//!
//! # What it shows
//!
//! Under F-70's barrier, every concurrent writer that observes
//! `needs_write_barrier() == true` acquires `unique_write_lock` and QUEUES
//! until the build drops the barrier at the end of Phase 3. So a writer that
//! arrives during the build is blocked for ~(remaining build duration) + its
//! own insert. The build itself is decode-bound (one full-table scan in BOTH
//! the old materialize shape and the new streaming shape), so its wall-time —
//! and therefore the writer-blocked time — is ~UNCHANGED by F-78's
//! memory-only fix; F-78's benefit is peak HEAP (see the
//! `create_index_streaming` bench), not writer latency. This probe confirms
//! that directly with measured percentiles rather than an assertion.
//!
//! # Old-vs-new
//!
//! `TableManager::create_index` is now ALWAYS the streaming path (F-78
//! rewrote the production call site), so this probe measures the NEW path.
//! The OLD path's writer latency is bounded IDENTICALLY: the barrier+lock
//! acquisition is byte-for-byte the SAME code (it lives in `create_index`'s
//! caller, ABOVE the Phase-2 body F-78 rewrote — only the body changed, not
//! the barrier), and the build wall-time is ~equal for old vs new (decode-
//! bound — measured in `create_index_streaming`). Hence OLD writer p95/p99 ≈
//! OLD build duration ≈ NEW build duration ≈ the percentiles reported here.
//!
//! Run:
//!   CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p shamir-engine --bench f78_writer_latency
//!   (calibrate first: ... -- --calibrate 4)
//!
//! ## Measured results (F-78, 5_000 rows, 64 concurrent writers, full
//! ## TableManager+MvccStore stack under peak_mem)
//!
//! build = 153–156 ms; writer p50 = p95 = p99 ≈ 136–137 ms ≈ build duration
//! (all 64 writers queue on `unique_write_lock` for ~(build duration) then
//! drain). This confirms F-70's barrier serializes concurrent writers across
//! the whole CREATE: writer latency tracks build duration, and — because the
//! barrier+lock acquisition is byte-identical in the OLD and NEW shapes (only
//! Phase 2's *body* changed) and the build is decode-bound (~equal old vs new,
//! see `create_index_streaming`) — writer p95/p99 is UNCHANGED by F-78. F-78's
//! measurable win is peak HEAP, not writer latency.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bench_scale_tool::Harness;
use shamir_engine::repo::{BoxRepo, RepoInstance};
use shamir_engine::table::TableConfig;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::types::value::InnerValue;

const N_ROWS: usize = 5_000;
const N_WRITERS: usize = 64;

async fn make_table(n_rows: usize) -> RepoInstance {
    let repo = Arc::new(InMemoryRepo::new());
    let instance = RepoInstance::new("bench".into(), BoxRepo::InMemory(repo), Vec::new());
    instance.add_table(TableConfig::new("bench_table".to_string()));
    let tbl = instance.get_table("bench_table").await.unwrap();
    // Populate with `n_rows` bare rows. They need NOT carry the indexed field:
    // the build still scans + decodes every row (its dominant cost), giving the
    // writers a non-trivial window to queue against. Direct `insert` keeps the
    // fixture free of the execute_batch/interner machinery.
    for i in 0..n_rows {
        tbl.insert(&InnerValue::Str(format!("r{i}"))).await.unwrap();
    }
    instance
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn main() {
    let mut h = Harness::new("f78_writer_latency", env!("CARGO_MANIFEST_DIR"));

    h.bench_batched_async(
        "create_index_with_concurrent_writers/5k_rows",
        // setup (untimed): fresh table populated with N rows.
        || async move { make_table(N_ROWS).await },
        // routine (timed): run create_index while N_WRITERS concurrent writers
        // each time their own insert. Report the build duration and the
        // writers' p50/p95/p99 each iteration.
        |repo| async move {
            let tbl = repo.get_table("bench_table").await.unwrap();

            // Spawn CREATE INDEX — it acquires the barrier + unique_write_lock
            // and holds them across the whole build.
            let tbl_build = tbl.clone();
            let build_handle = tokio::spawn(async move {
                let start = Instant::now();
                tbl_build.create_index("by_city", &["city"]).await.unwrap();
                start.elapsed()
            });

            // Let the build task start + complete `begin_write_barrier`
            // (raise bit → drain → acquire `unique_write_lock`) BEFORE the
            // writers are issued, so the writers reliably observe
            // `needs_write_barrier() == true` and queue on the lock (rather
            // than racing through the few-µs window before the bit goes up).
            // `begin_write_barrier` is sub-ms once no writers are in-flight.
            tokio::time::sleep(Duration::from_millis(5)).await;

            // Spawn N_WRITERS concurrent writers; each times its OWN insert
            // (queue-wait + insert work).
            let mut writer_handles = Vec::with_capacity(N_WRITERS);
            for i in 0..N_WRITERS {
                let tbl_w = tbl.clone();
                writer_handles.push(tokio::spawn(async move {
                    let start = Instant::now();
                    tbl_w.insert(&InnerValue::Str(format!("w_{i}")))
                        .await
                        .unwrap();
                    start.elapsed()
                }));
            }

            let build_ms = build_handle.await.unwrap().as_secs_f64() * 1e3;
            let mut lats: Vec<Duration> = Vec::with_capacity(N_WRITERS);
            for wh in writer_handles {
                lats.push(wh.await.unwrap());
            }
            lats.sort_unstable();
            let p50 = percentile(&lats, 0.50).as_secs_f64() * 1e3;
            let p95 = percentile(&lats, 0.95).as_secs_f64() * 1e3;
            let p99 = percentile(&lats, 0.99).as_secs_f64() * 1e3;
            eprintln!(
                "  F-78 writer-latency: build={build_ms:.0} ms, writer p50={p50:.0} ms p95={p95:.0} ms p99={p99:.0} ms (n={N_WRITERS} writers)"
            );
        },
    );

    h.run();
}
