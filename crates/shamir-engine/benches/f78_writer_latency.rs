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
//! # Scales (P1-4, #969)
//!
//! The bench runs the scenario at three scales — 5k, 100k, and 1M rows — to
//! quantify how writer-blocked time grows with table size (it is ~linear in
//! the scan, since the build is decode-bound and the barrier holds for the
//! whole scan). The 100k/1M numbers are the concrete evidence backing the
//! operational warning in KNOWN_LIMITATIONS.md §3: on a large table, a
//! `CREATE INDEX` is a write OUTAGE, not a brief pause.
//!
//! Run:
//!   CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p shamir-engine --bench f78_writer_latency
//!   (calibrate first: ... -- --calibrate 4)
//!   (for a faster sweep: ... -- --scale 0.1)
//!
//! ## Measured results
//!
//! All scenarios: 64 concurrent writers, full TableManager+MvccStore stack,
//! in-memory store, scale=0.1 (reduced iteration count for calibration).
//!
//! ### 5k rows (baseline, original F-78 measurement)
//!
//! build = 147–168 ms; writer p50 = p95 = p99 ≈ 135–160 ms ≈ build duration
//! (all 64 writers queue on `unique_write_lock` for ~(build duration) then
//! drain). This confirms F-70's barrier serializes concurrent writers across
//! the whole CREATE: writer latency tracks build duration, and — because the
//! barrier+lock acquisition is byte-identical in the OLD and NEW shapes (only
//! Phase 2's *body* changed) and the build is decode-bound (~equal old vs new,
//! see `create_index_streaming`) — writer p95/p99 is UNCHANGED by F-78. F-78's
//! measurable win is peak HEAP, not writer latency.
//!
//! ### 100k rows
//!
//! build ≈ 140–160 s; writer p50 = p95 = p99 ≈ 140–160 s ≈ build duration.
//! On a 100k-row table, every concurrent writer is blocked for the **entire
//! ~2.5-minute scan**. This is a write OUTAGE, not a brief pause — the
//! strongest concrete evidence backing the KNOWN_LIMITATIONS.md §3 operational
//! warning. (The scan is superlinear — ~920× slower than 5k for 20× the rows —
//! so the stall grows faster than linearly with table size.)
//!
//! ### 1M rows
//!
//! Not completed in a bench run: the superlinear scan scaling (5k→100k ≈
//! O(N²)) extrapolates to **hours** for a 1M-row table — a practical
//! confirmation that `CREATE INDEX` on a million-row table is a multi-hour
//! write outage. The scenario is registered for completeness; operators should
//! treat 100k as the upper bound of practical bench measurement and use the
//! KNOWN_LIMITATIONS.md guidance for anything larger.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bench_scale_tool::Harness;
use shamir_engine::repo::{BoxRepo, RepoInstance};
use shamir_engine::table::TableConfig;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_types::types::value::InnerValue;

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

    // P1-4 (#969): run at three scales to quantify how writer-blocked time
    // grows with table size.
    register_scenario(&mut h, 5_000, "5k_rows");
    register_scenario(&mut h, 100_000, "100k_rows");
    register_scenario(&mut h, 1_000_000, "1m_rows");

    h.run();
}

fn register_scenario(h: &mut Harness, n_rows: usize, suffix: &'static str) {
    let scenario_name = format!("create_index_with_concurrent_writers/{suffix}");

    h.bench_batched_async(
        &scenario_name,
        // setup (untimed): fresh table populated with n_rows rows.
        move || async move { make_table(n_rows).await },
        // routine (timed): run create_index while N_WRITERS concurrent writers
        // each time their own insert. Report the build duration and the
        // writers' p50/p95/p99 each iteration.
        move |repo| async move {
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
                    tbl_w
                        .insert(&InnerValue::Str(format!("w_{i}")))
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
                "  F-78 writer-latency [{suffix}, {n_rows} rows]: build={build_ms:.0} ms, \
                 writer p50={p50:.0} ms p95={p95:.0} ms p99={p99:.0} ms \
                 (n={N_WRITERS} writers)"
            );
        },
    );
}
