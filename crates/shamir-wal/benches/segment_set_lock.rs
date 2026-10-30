//! End-to-end concurrent append workloads bypassing `WalGroupCommit`.
//!
//! These scenarios exercise `SegmentSet::append_batch` directly, including
//! scheduling, framing and file writes. They do not isolate lock cost, check
//! replay, or inject failures. Concurrent tasks need not contend simultaneously.
//!
//! **Two scenarios, two different questions:**
//!
//! - `raw_append/n_*` (single fan-out-and-join per iteration) — a
//!   concurrent append workload, not a correctness test or lock cost measurement. See the methodology
//!   warning on `fan_out_raw` below: with only one `append_batch` call per
//!   spawned task, the timed region includes one-shot `tokio::spawn`
//!   / `JoinHandle::await` overhead as well as append work. A `#1109` review
//!   found this shape had previously been misread as "per-append latency
//!   improving 5x under concurrency" — that reading divided a
//!   spawn-N-and-join-N round trip by N and mistook amortization of the
//!   fixed per-iteration harness overhead for the lock getting cheaper
//!   under contention. Dividing a round-trip time by N does not isolate
//!   mutex acquisition. The workload checks returned
//!   results, not replay integrity or failure handling.
//! - `sustained_append/n_*` — sustained END-TO-END append throughput with
//!   concurrent callers. Each of the `N` concurrent tasks performs `APPENDS_PER_TASK`
//!   sequential `append_batch` calls in a loop; the one-time `tokio::spawn`/join
//!   overhead is paid once per task and amortized over many appends.
//!   **This is NOT an isolated mutex-acquisition measurement**: the timed
//!   region includes payload clone, append framing/validation, blocking
//!   dispatch (`spawn_blocking`) and file writes. `SegmentSet::inner` is
//!   `pub(super)`, so the bench cannot time bare lock/unlock; no per-
//!   acquisition lock-cost figure is claimed from this scenario. Historical
//!   ratios cited elsewhere (4.4× mem-sink scaling, ~63× fsync dominance)
//!   are UNVERIFIED here — no reproducible measurement exists in this repo
//!   and none is invented by this bench.
//!   The harness reports `ns/op` treating the WHOLE n-task
//!   run as one op (it has no visibility into the nested loop) — divide the
//!   printed `ns/op` by `N * APPENDS_PER_TASK` by hand to get mean
//!   per-append END-TO-END cost for the concurrent workload.
//!
//! **IMPORTANT**: Both scenarios create an ARTIFICIAL situation that never
//! occurs in production (the real system serializes all `SegmentSet` access
//! through `WalGroupCommit`'s single elected leader — see
//! `segment_set.rs`'s struct doc). Neither scenario's absolute numbers are a
//! production latency prediction; `sustained_append` measures end-to-end
//! throughput for direct concurrent callers, not an isolated lock acquisition.
//!
//! **Withdrawn claims**: no number printed by this bench reproduces the
//! historical "4.4×" or "~63×" ratios, and none of them measures isolated
//! mutex acquisition. Until a workload that locks `SegmentSet::inner`
//! directly is possible, lock cost is UNMEASURED, not small.

#![allow(clippy::needless_borrow)]

use std::cell::Cell;
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_wal::segment_set::SegmentSet;
use shamir_wal::wal_entry_v2::WalEntryV2;

/// Concurrency levels under test — same as `wal_append` for comparability.
const CONCURRENCY: &[usize] = &[1, 4, 16, 64];

/// Large seal threshold so no segment rotation fires inside the timed window.
/// Rotation is a rare path, not what we are measuring.
const SEG_MAX_BYTES: u64 = 1 << 30; // 1 GiB

/// Sequential `append_batch` calls per task in the `sustained_append`
/// scenario. Large enough that the one-time `tokio::spawn` + final
/// `JoinHandle::await` overhead per task is amortized over
/// `APPENDS_PER_TASK` appends. The workload measures end-to-end time,
/// not isolated lock cost or per-call latency.
///
/// The payload template is encoded ONCE at setup and cloned per append, so the
/// timed region contains no bincode encode work — only the unavoidable
/// per-append clone, framing, blocking dispatch and file writes.
const APPENDS_PER_TASK: usize = 2_000;

fn payload() -> Vec<u8> {
    WalEntryV2::new(1, 0, Vec::new())
        .encode()
        .expect("benchmark entry encode")
}

/// Run `n` concurrent `append_batch` calls directly against `segset`, ONE
/// call per spawned task. Calls may overlap; simultaneous contention is not guaranteed.
///
/// **Methodology warning (do not use this to derive a per-append latency
/// figure — see `#1109`):** because each task only calls `append_batch`
/// once, the timed region (the whole fan-out-and-join round trip) is
/// includes one-shot `tokio::spawn`/`JoinHandle::await` scheduling
/// overhead as well as append work. Dividing the reported per-iteration
/// time by `n` does NOT recover a clean per-call latency or mutex cost.
/// This workload checks returned results only; it does not check replay or failures.
/// `fan_out_sustained` measures end-to-end cost, not lock acquisition.
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
/// `append_batch` calls in a loop, then join all `n`. Scheduling does not
/// guarantee simultaneous contention throughout the run.
///
/// Workload scope: this times END-TO-END sustained appends — payload clone,
/// frame validation, blocking dispatch (`spawn_blocking`) and the file
/// write. It does NOT isolate `SegmentSet::inner` mutex acquisition (the
/// field is `pub(super)` and unreachable from a bench); do not read its
/// output as a lock-cost figure.
///
/// The caller (the harness, via `bench_async`) times this whole function —
/// spawn loop, `n * APPENDS_PER_TASK` real appends, and join loop — as ONE
/// "op". To read the mean per-append END-TO-END cost, divide the harness's
/// reported `ns/op` for a given `n` by `n * APPENDS_PER_TASK`: the one-time
/// `tokio::spawn`/join overhead (paid once per task, `n` times total) is
/// amortized over `n * APPENDS_PER_TASK` appends instead of just `n` of
/// them; its contribution is not independently measured.
async fn fan_out_sustained(
    segset: Arc<SegmentSet>,
    n: usize,
    version_base: u64,
    payload_template: Vec<u8>,
) {
    let mut handles = Vec::with_capacity(n);
    for w in 0..n {
        let segset = Arc::clone(&segset);
        let payload_template = payload_template.clone();
        // Each task owns a disjoint version range so `max_version` stays
        // monotonically non-decreasing per task without any cross-task
        // coordination (SegmentSet does not require globally monotonic
        // versions across concurrent callers — only per-caller ordering
        // matters for this bench's purposes).
        let base = version_base + (w as u64) * (APPENDS_PER_TASK as u64);
        handles.push(tokio::spawn(async move {
            for i in 0..APPENDS_PER_TASK {
                segset
                    .append_batch(vec![payload_template.clone()], base + i as u64)
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
    // Concurrent end-to-end workload, NOT a correctness test or isolated
    // lock-cost measurement. Each task appends once and checks its result;
    // neither replay nor failure handling is exercised.
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

    // ── Sustained END-TO-END append under contention (#1109) ─────────────
    //
    // Each of the `n` concurrent tasks performs `APPENDS_PER_TASK`
    // SEQUENTIAL `append_batch` calls, so the one-shot spawn/join overhead
    // (paid once per task) is amortized over many real appends
    // instead of just one — unlike `raw_append`, where it is paid once per
    // SINGLE append. Payloads are pre-encoded at setup (see `payload()`),
    // so the timed region excludes encode work.
    //
    // **Not a lock-cost measurement**: this times the full append pipeline
    // (payload clone, framing, blocking dispatch, file writes). Isolated
    // `SegmentSet::inner` mutex-acquisition cost is unmeasured — `inner` is
    // `pub(super)` and cannot be locked from a bench. Do not derive a
    // per-acquisition lock figure from these numbers.
    //
    // **Unit note**: the harness's own `ns/op` column treats one call to
    // this closure (i.e. one whole "spawn `n` tasks, each does
    // `APPENDS_PER_TASK` sequential appends, join `n`" run) as ONE op — it
    // has no visibility into the `n * APPENDS_PER_TASK` real appends nested
    // inside. To read mean end-to-end cost, divide the reported
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
        // Payload production is deliberately OUTSIDE the timed workload:
        // encode once here, clone per append inside the loop.
        let payload_template = payload();
        h.bench_async(&id, move || {
            let _keep_alive = &dir;
            let i = counter.get();
            counter.set(i + 1);
            let base = i * (n as u64) * (APPENDS_PER_TASK as u64) + 1;
            let template = payload_template.clone();
            let segset = Arc::clone(&segset);
            async move {
                fan_out_sustained(segset, n, base, template).await;
            }
        });
    }

    // ── Concurrent append + truncate workload ──────────────────────────────
    //
    // Concurrent append + truncate workload; it checks returned results but
    // neither replay nor failure handling, and does not isolate lock cost.
    //
    // We run this at N=16 only to limit bench wall-time. The append tasks submit
    // entries with increasing versions; the truncator requests deletion below
    // the current watermark. The active segment need not be deleted.
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
                let segset_trunc = Arc::clone(&segset);
                let truncator = tokio::spawn(async move {
                    // Small delay encourages overlap but does not guarantee it.
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
