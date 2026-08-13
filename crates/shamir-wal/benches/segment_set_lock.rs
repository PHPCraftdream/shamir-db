//! Isolated contention probe for `SegmentSet::inner: std::sync::Mutex<Inner>`.
//!
//! This benchmark measures `SegmentSet::append_batch`'s own lock contention
//! directly, bypassing `WalGroupCommit`'s leader-election and `pending` queue
//! entirely. Multiple concurrent tasks call `SegmentSet::append_batch` without
//! any serialization, which tests the lock's behavior under its worst-case
//! scenario (all N callers contend).
//!
//! **Purpose**: Isolate `SegmentSet::inner`'s contribution to the latency rise
//! observed in the `wal_append` bench. If the `wal_append` bench shows latency
//! rising from N=1 to N=64, and the `mem` scenario shows the SAME rise (which
//! never touches `SegmentSet`), then the rise is NOT from `SegmentSet::inner`.
//! However, we need empirical proof — this bench provides it.
//!
//! **Interpretation**:
//!   - If latency stays flat / rises negligibly across 1→64 concurrency:
//!     the lock is NOT the bottleneck. The architectural claim holds (the
//!     single-writer model via group-commit leader election means only one
//!     caller ever contends in production).
//!   - If latency DOES rise meaningfully with concurrency: the lock itself
//!     is a bottleneck, even under the real single-writer model. This would
//!     justify a lock-free migration (see #1095 discussion).
//!
//! **IMPORTANT**: This bench creates an ARTIFICIAL scenario that never occurs
//! in production (the real system serializes via WalGroupCommit's leader
//! election). The measured latency numbers here are NOT representative of
//! real-world performance — we care ONLY about the TREND (flat vs. rising),
//! not the absolute values.

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

fn payload() -> Vec<u8> {
    vec![0xABu8; PAYLOAD_LEN]
}

/// Run `n` concurrent `append_batch` calls directly against `segset`.
/// Each task appends a single-entry batch. NO serialization — all N tasks
/// contend on `SegmentSet::inner`'s mutex simultaneously.
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

fn main() {
    let mut h = Harness::new("segment_set_lock", env!("CARGO_MANIFEST_DIR"));

    // ── Direct SegmentSet append, no WalGroupCommit serialization ────────
    //
    // This measures SegmentSet::inner's mutex contention in isolation.
    // Every concurrent task calls append_batch directly — no leader election,
    // no pending queue, NO serialization at all. This is the worst-case
    // contention scenario for the lock.
    //
    // What we care about: does per-op latency rise with concurrency?
    // - Flat/negligible rise → lock is NOT the bottleneck in production
    // - Meaningful rise → lock IS a bottleneck, even with single-writer model
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
