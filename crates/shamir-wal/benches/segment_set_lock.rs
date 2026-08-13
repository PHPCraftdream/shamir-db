//! Isolated contention probe for `SegmentSet::inner: std::sync::Mutex<Inner>`.
//!
//! This benchmark measures `SegmentSet::append_batch`'s own lock contention
//! directly, bypassing `WalGroupCommit`'s leader-election and `pending` queue
//! entirely. Multiple concurrent tasks call `SegmentSet::append_batch` without
//! any serialization, which tests the lock's behavior under its worst-case
//! scenario (all N callers contend simultaneously, for the whole run).
//!
//! **Two scenarios, two different questions:**
//!
//! - `raw_append/n_*` (single fan-out-and-join per iteration) — a lock
//!   CORRECTNESS stress test, not a cost measurement. See the methodology
//!   warning on `fan_out_raw` below: with only one `append_batch` call per
//!   spawned task, the timed region is dominated by one-shot `tokio::spawn`
//!   / `JoinHandle::await` overhead, not by the mutex. A `#1109` review
//!   found this shape had previously been misread as "per-append latency
//!   improving 5x under concurrency" — that reading divided a
//!   spawn-N-and-join-N round trip by N and mistook amortization of the
//!   fixed per-iteration harness overhead for the lock getting cheaper
//!   under contention, which is not physically possible (contention can
//!   only add wait time, never remove it). Kept here only for its original
//!   purpose: proving `append_batch` stays correct under worst-case,
//!   all-N-contend concurrency.
//! - `sustained_append/n_*` — the actual PER-ACQUISITION cost measurement.
//!   Each of the `N` concurrent tasks performs `APPENDS_PER_TASK` sequential
//!   `append_batch` calls in a loop; the one-time `tokio::spawn`/join
//!   overhead is paid once per task and amortized over many real lock
//!   acquisitions (not once per acquisition, as `raw_append` effectively
//!   does at n_1). The harness reports `ns/op` treating the WHOLE n-task
//!   run as one op (it has no visibility into the nested loop) — divide the
//!   printed `ns/op` by `N * APPENDS_PER_TASK` by hand to get genuine mean
//!   per-append cost while N tasks sustain contention throughout the whole
//!   timed window. See `fan_out_sustained`'s doc comment for the exact
//!   arithmetic.
//!
//! **IMPORTANT**: Both scenarios create an ARTIFICIAL situation that never
//! occurs in production (the real system serializes all `SegmentSet` access
//! through `WalGroupCommit`'s single elected leader — see
//! `segment_set.rs`'s struct doc). Neither scenario's absolute numbers are a
//! production latency prediction; `sustained_append` answers "how expensive
//! is one lock acquisition+critical-section under sustained contention",
//! which is the number needed to judge whether `SegmentSet::inner` would be
//! a bottleneck IF the single-writer architecture were ever violated or
//! revisited. The single-writer argument itself — only the group-commit
//! leader ever calls into `SegmentSet` — is what actually establishes
//! "negligible in production today"; see `CLAUDE.md`'s
//! `SegmentSet::inner` entry.

#![allow(clippy::needless_borrow)]

use std::cell::Cell;
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_wal::segment_set::SegmentSet;

/// Concurrency levels under test — same as `wal_append` for comparability.
const CONCURRENCY: &[usize] = &[1, 4, 16, 64];

/// Per-append payload size (bytes) — same as `wal_append` for fair comparison.
const PAYLOAD_LEN: usize = 128;

/// Large seal threshold so no segment rotation fires inside the timed window.
/// Rotation is a rare path, not what we are measuring.
const SEG_MAX_BYTES: u64 = 1 << 30; // 1 GiB

/// Sequential `append_batch` calls per task in the `sustained_append`
/// scenario. Large enough that the one-time `tokio::spawn` + final
/// `JoinHandle::await` overhead per task (paid once) is negligible next to
/// the aggregate cost of `APPENDS_PER_TASK` real lock acquisitions (paid
/// `APPENDS_PER_TASK` times) — i.e. large enough to actually isolate
/// per-acquisition cost rather than re-measuring spawn/join overhead under
/// a different name.
const APPENDS_PER_TASK: usize = 2_000;

fn payload() -> Vec<u8> {
    vec![0xABu8; PAYLOAD_LEN]
}

/// Run `n` concurrent `append_batch` calls directly against `segset`, ONE
/// call per spawned task. NO serialization — all N tasks contend on
/// `SegmentSet::inner`'s mutex simultaneously.
///
/// **Methodology warning (do not use this to derive a per-append latency
/// figure — see `#1109`):** because each task only calls `append_batch`
/// once, the timed region (the whole fan-out-and-join round trip) is
/// dominated by one-shot `tokio::spawn`/`JoinHandle::await` scheduling
/// overhead, not by the mutex's own cost. Dividing the reported per-iteration
/// time by `n` does NOT recover a clean "per single append" cost — it mixes
/// in the (roughly n-invariant) spawn/join overhead, diluted differently at
/// each `n`. A prior interpretation did exactly this division and read the
/// resulting falling curve as "the lock gets 5x cheaper under contention",
/// which cannot be correct (contention can only add wait time to a mutex
/// acquisition, never subtract it) — the falling curve was amortization of
/// fixed overhead across more real work, not the lock improving. This
/// function is kept for what it's actually good at: proving `append_batch`
/// stays correct (no lost/corrupted entries, no deadlock) under worst-case,
/// all-N-contend-at-once concurrency. For a per-acquisition cost estimate,
/// see `fan_out_sustained` / the `sustained_append` scenario below.
async fn fan_out_raw(segset: Arc<SegmentSet>, n: usize, version_base: u64) {
    let mut handles = Vec::with_capacity(n);
    for w in 0..n {
        let segset = Arc::clone(&segset);
        let v = version_base + w as u64;
        handles.push(tokio::spawn(async move {
            segset
                .append_batch(vec![payload()], v)
                .await
                .expect("SegmentSet append_batch");
        }));
    }
    for handle in handles {
        handle.await.unwrap();
    }
}

/// Run `n` concurrent tasks, each performing `APPENDS_PER_TASK` SEQUENTIAL
/// `append_batch` calls in a loop, then join all `n`. All `n` tasks run for
/// the entire duration, so — unlike `fan_out_raw` — this sustains real
/// contention on `SegmentSet::inner` across many acquisitions per task, not
/// just one.
///
/// The caller (the harness, via `bench_async`) times this whole function —
/// spawn loop, `n * APPENDS_PER_TASK` real appends, and join loop — as ONE
/// "op". To read genuine per-acquisition cost, divide the harness's
/// reported `ns/op` for a given `n` by `n * APPENDS_PER_TASK`: the one-time
/// `tokio::spawn`/join overhead (paid once per task, `n` times total) is
/// amortized over `n * APPENDS_PER_TASK` real lock acquisitions instead of
/// just `n` of them, so it no longer dominates the way it does in
/// `fan_out_raw` at low `n`.
async fn fan_out_sustained(segset: Arc<SegmentSet>, n: usize, version_base: u64) {
    let mut handles = Vec::with_capacity(n);
    for w in 0..n {
        let segset = Arc::clone(&segset);
        // Each task owns a disjoint version range so `max_version` stays
        // monotonically non-decreasing per task without any cross-task
        // coordination (SegmentSet does not require globally monotonic
        // versions across concurrent callers — only per-caller ordering
        // matters for this bench's purposes).
        let base = version_base + (w as u64) * (APPENDS_PER_TASK as u64);
        handles.push(tokio::spawn(async move {
            for i in 0..APPENDS_PER_TASK {
                segset
                    .append_batch(vec![payload()], base + i as u64)
                    .await
                    .expect("SegmentSet append_batch");
            }
        }));
    }
    for handle in handles {
        handle.await.unwrap();
    }
}

fn main() {
    let mut h = Harness::new("segment_set_lock", env!("CARGO_MANIFEST_DIR"));

    // ── Direct SegmentSet append, no WalGroupCommit serialization ────────
    //
    // Lock CORRECTNESS stress test, NOT a per-append cost measurement — see
    // `fan_out_raw`'s doc comment. Every concurrent task calls append_batch
    // directly, once, — no leader election, no pending queue, NO
    // serialization at all. This is the worst-case *correctness* scenario
    // for the lock (all N contend simultaneously); it does NOT isolate
    // per-acquisition cost, because the timed region is dominated by
    // one-shot spawn/join overhead at low n. Use `sustained_append` below
    // for the cost question.
    for &n in CONCURRENCY {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let setup_rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let segset = setup_rt
            .block_on(SegmentSet::open(dir.path().to_path_buf(), SEG_MAX_BYTES))
            .expect("SegmentSet::open");
        let segset = Arc::new(segset);
        let counter = Cell::new(0u64);
        let id = format!("segment_set_lock/raw_append/n_{n}");
        // `dir` (TempDir) must outlive every iteration — captured by the
        // closure so it is dropped only when the harness drops the workload.
        h.bench_async(&id, move || {
            let _keep_alive = &dir;
            let i = counter.get();
            counter.set(i + 1);
            let base = i * (n as u64) + 1;
            let segset = Arc::clone(&segset);
            async move {
                fan_out_raw(segset, n, base).await;
            }
        });
    }

    // ── Sustained-contention per-acquisition cost measurement (#1109) ────
    //
    // Each of the `n` concurrent tasks performs `APPENDS_PER_TASK`
    // SEQUENTIAL `append_batch` calls, so the one-shot spawn/join overhead
    // (paid once per task) is amortized over many real lock acquisitions
    // instead of just one — unlike `raw_append`, where it is paid once per
    // SINGLE acquisition.
    //
    // **Unit note**: the harness's own `ns/op` column treats one call to
    // this closure (i.e. one whole "spawn `n` tasks, each does
    // `APPENDS_PER_TASK` sequential appends, join `n`" run) as ONE op — it
    // has no visibility into the `n * APPENDS_PER_TASK` real appends nested
    // inside. To read genuine per-append latency, divide the reported
    // `ns/op` for `sustained_append/n_{n}` by `n * APPENDS_PER_TASK`
    // (constants defined above/at the call site). This is a manual
    // division at report-reading time, not a division hidden inside the
    // bench — see the module doc and the CLAUDE.md entry this bench feeds.
    for &n in CONCURRENCY {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let setup_rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let segset = setup_rt
            .block_on(SegmentSet::open(dir.path().to_path_buf(), SEG_MAX_BYTES))
            .expect("SegmentSet::open");
        let segset = Arc::new(segset);
        let counter = Cell::new(0u64);
        let id = format!("segment_set_lock/sustained_append/n_{n}");
        h.bench_async(&id, move || {
            let _keep_alive = &dir;
            let i = counter.get();
            counter.set(i + 1);
            let base = i * (n as u64) * (APPENDS_PER_TASK as u64) + 1;
            let segset = Arc::clone(&segset);
            async move {
                fan_out_sustained(segset, n, base).await;
            }
        });
    }

    // ── Append + truncate concurrent stress test ──────────────────────────
    //
    // The doc comment in `segment_set.rs` claims: "The single-writer model is
    // the group-commit leader (the sole appender) plus the truncator (rare);
    // they do not contend on a hot path." This test forces a concurrent
    // append + truncate scenario to verify that the lock behavior is
    // acceptable even when both paths overlap.
    //
    // Note: this is NOT the production pattern (truncation is a rare
    // background drainer operation, not per-commit). This is a stress test
    // for lock correctness under edge-case concurrency, not a realistic
    // workload measurement.
    //
    // We run this at N=16 only (high enough to stress the lock, low enough
    // to keep the bench wall-time reasonable). The append tasks submit
    // entries with version numbers 1..16, then the truncator deletes all
    // of them (versioned 1..16 are all <= durable_watermark 16).
    {
        let n: usize = 16;
        let dir = tempfile::TempDir::new().expect("tempdir");
        let setup_rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let segset = setup_rt
            .block_on(SegmentSet::open(dir.path().to_path_buf(), SEG_MAX_BYTES))
            .expect("SegmentSet::open");
        let segset = Arc::new(segset);
        let counter = Cell::new(0u64);
        let id = "segment_set_lock/append_truncate_concurrent";
        h.bench_async(&id, move || {
            let _keep_alive = &dir;
            let i = counter.get();
            counter.set(i + 1);
            let base = i * (n as u64) + 1;
            let segset = Arc::clone(&segset);
            async move {
                // Spawn N append tasks, each writing one entry with a
                // version number in the 1..N range.
                let mut handles = Vec::with_capacity(n);
                for w in 0..n {
                    let segset = Arc::clone(&segset);
                    let v = base + w as u64;
                    handles.push(tokio::spawn(async move {
                        segset
                            .append_batch(vec![payload()], v)
                            .await
                            .expect("append in append+truncate test");
                    }));
                }
                // Spawn a truncator that tries to delete all entries up to
                // version N (the durable watermark). This will race with the
                // appends — some may still be in-flight when truncate runs.
                // The lock must handle this safely (the real system never hits
                // this pattern, but we test lock correctness under it).
                let segset_trunc = Arc::clone(&segset);
                let truncator = tokio::spawn(async move {
                    // Small delay to let some appends finish — not required
                    // for correctness, just creates a mixed overlap pattern.
                    tokio::time::sleep(tokio::time::Duration::from_micros(10)).await;
                    let _ = segset_trunc
                        .truncate_below(base + n as u64)
                        .await
                        .expect("truncate in append+truncate test");
                });
                // Wait for all appends to complete.
                for handle in handles {
                    handle.await.unwrap();
                }
                // Wait for the truncator to complete.
                truncator.await.unwrap();
            }
        });
    }

    h.run();
}
