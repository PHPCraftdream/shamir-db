//! F-78 (#905) / RFC v3 (#1018, online CREATE INDEX) writer-latency probe —
//! concurrent-writer p50/p95/p99 during a real `TableManager::create_index`.
//!
//! This is a **latency-distribution probe**, not a throughput workload — it
//! runs the (create_index + concurrent writers) scenario and reports the
//! writers' observed p50/p95/p99, so it uses the harness's `bench_batched_async`
//! for a stable scenario wall-time AND prints the per-scenario percentiles.
//!
//! # What changed (#1087-#1089, #1060-#1061 — online CREATE INDEX, regular/hash only)
//!
//! **Historically** (pre-#1087, and STILL TRUE for `create_unique_index`/
//! `create_sorted_index`/`create_index_v2` — none of those call sites were
//! touched by the online-build redesign): `create_index` acquired F-70's
//! write barrier (`begin_write_barrier`: raise bit → drain → hold
//! `unique_write_lock`) across the WHOLE backfill sequence. Every concurrent
//! writer that observed `needs_write_barrier() == true` queued on
//! `unique_write_lock` until the build finished — writer p50/p95/p99 tracked
//! total build duration, which is decode-bound and scales with table size.
//! The "5k rows / 100k rows" numbers in this file's git history (see
//! `git log -p` on this file, or KNOWN_LIMITATIONS.md §3's still-current
//! description of `create_unique_index`/`create_sorted_index`/`create_index_v2`)
//! remain the accurate characterization of THOSE call sites today.
//!
//! **Now, for the regular/hash family only:** `TableManager::create_index`
//! tries the online-build path first (`phase_b_a_backfill` then
//! `phase_c_d_catchup_and_publish`) whenever the table has an MVCC
//! changefeed attached (true for every table built through the normal
//! `RepoInstance::add_table`/`get_table` path — including THIS bench's
//! `make_table` helper, unchanged). Phase A (the O(table) scan that
//! dominates wall-clock) is barrier-free: a concurrent writer arriving
//! during Phase A is NOT queued at all. Only Phase B (register at
//! `Building`) and Phase D (short publish barrier: apply the bounded
//! residual, flip `Ready`) hold `unique_write_lock`, and both are proven
//! bounded independent of table size (`#1061`'s
//! `p1061_bounded_barrier_duration_constant_across_sizes` test asserts
//! Phase D stays under 100ms at both 500 and 50,000 rows). So a writer
//! landing during the (now-dominant) Phase A window pays close to nothing;
//! only the rare writer that happens to land inside the brief Phase B/D
//! windows pays a small, size-independent wait. **The flip this probe now
//! demonstrates: writer p95/p99 no longer tracks build duration — it stays
//! small and roughly flat across table sizes, while build duration (still
//! dominated by Phase A's scan) keeps growing with table size.**
//!
//! # Scales (P1-4, #969)
//!
//! The bench runs the scenario at three scales — 5k, 100k, and 1M rows — to
//! quantify how (build duration) and (writer-blocked time) now DIVERGE as
//! table size grows, where they used to track each other exactly.
//!
//! Run:
//!   CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p shamir-engine --bench f78_writer_latency
//!   (calibrate first: ... -- --calibrate 4)
//!   (for a faster sweep: ... -- --scale 0.1)
//!
//! ## Measured results (post-redesign, #1062, ACTUALLY RUN — not extrapolated)
//!
//! Run 2026-08-10, `CARGO_TARGET_DIR=D:/dev/rust/.cargo-target-bench cargo
//! bench -p shamir-engine --bench f78_writer_latency -- --scale 0.1`. All
//! scenarios: 64 concurrent writers, full TableManager+MvccStore stack
//! (changefeed attached — online-build path), in-memory store.
//!
//! ### 5k rows (raw per-iteration lines from the actual run)
//!
//! ```text
//! build=192 ms, writer p50=0 ms p95=0 ms p99=0 ms
//! build=160 ms, writer p50=0 ms p95=0 ms p99=0 ms
//! build=167 ms, writer p50=0 ms p95=0 ms p99=0 ms
//! build=188 ms, writer p50=0 ms p95=0 ms p99=0 ms
//! build=186 ms, writer p50=0 ms p95=0 ms p99=0 ms
//! build=149 ms, writer p50=0 ms p95=0 ms p99=0 ms
//! build=177 ms, writer p50=0 ms p95=0 ms p99=0 ms
//! build=157 ms, writer p50=0 ms p95=1 ms p99=1 ms
//! build=148 ms, writer p50=0 ms p95=0 ms p99=0 ms
//! ```
//! build ≈ 148-192 ms; writer p50/p95/p99 ≈ 0-1 ms. Pre-redesign (original
//! F-78 measurement, still accurate for `create_unique_index`/
//! `create_sorted_index`/`create_index_v2`) this was build ≈ 147-168 ms
//! with writer p50=p95=p99 ≈ 135-160 ms — writers queued for the WHOLE
//! build. Now the 64 writers complete in ~0-1 ms because Phase A (the
//! dominant cost) is barrier-free.
//!
//! ### 50k rows (raw per-iteration lines from the actual run)
//!
//! ```text
//! build=32026 ms, writer p50=0 ms p95=0 ms p99=0 ms
//! build=31563 ms, writer p50=0 ms p95=0 ms p99=0 ms
//! build=31671 ms, writer p50=0 ms p95=0 ms p99=0 ms
//! build=33190 ms, writer p50=0 ms p95=0 ms p99=0 ms
//! ```
//! build ≈ 31.6-33.2 **seconds** (a ~170-200× increase over the 5k build
//! time for 10× the rows — the scan remains superlinear, unchanged by this
//! redesign) while writer p50/p95/p99 stays at **0 ms** — completely flat,
//! not tracking build duration at all. This is the concrete before/after:
//! pre-redesign, a writer landing during a 50k-scale (or larger)
//! `CREATE INDEX` would have queued for the same ~tens-of-seconds the build
//! itself took; now it completes immediately regardless of table size,
//! because the only barrier windows left (Phase B register, Phase D
//! publish) are proven bounded independent of table size (`#1061`'s
//! `p1061_bounded_barrier_duration_constant_across_sizes`, <100ms at both
//! 500 and 50,000 rows).
//!
//! (50k chosen over 100k for this session's practical bench turnaround —
//! measured at 50k instead of extrapolated, and the divergence is already
//! unambiguous: 33-SECOND build vs 0-MS writer latency.)
//!
//! ### 1M rows
//!
//! Not run in this session (matches the pre-redesign bench's own
//! precedent of treating 1M as impractical for routine measurement — the
//! Phase A scan is still the same superlinear full-table decode as before,
//! unchanged by this redesign; only the WRITER-side behavior changed). The
//! scenario remains registered for completeness.

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

    // P1-4 (#969) / #1062: run at three scales to quantify how (build
    // duration) and (writer-blocked time) now DIVERGE as table size grows.
    // 50k (was 100k pre-#1062) keeps a full local bench run practical while
    // still demonstrating the divergence unambiguously — see this file's
    // header doc for the measured numbers and #1061's own 500-vs-50,000-row
    // precedent for the same scale choice.
    register_scenario(&mut h, 5_000, "5k_rows");
    register_scenario(&mut h, 50_000, "50k_rows");
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
