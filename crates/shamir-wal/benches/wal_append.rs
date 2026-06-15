//! CAPSTONE measure-first — WAL group-commit append-path contention probe.
//!
//! Measures whether the two sanctioned locks on the WAL append path —
//! `WalGroupCommit.pending: tokio::sync::Mutex<Vec<Pending>>` (one O(1) push
//! per concurrent committer, one `mem::take` per window) and
//! `WalSegment.file: Arc<std::sync::Mutex<File>>` (held ONLY on the blocking
//! thread inside `spawn_blocking`, ONLY by the single leader) — cap WAL
//! append throughput under concurrent committers, or whether the cost is
//! dominated by I/O (write/fsync) and coordination (CAS leadership election,
//! `Notify` park), with the locks being noise.
//!
//! Group-commit model recap: N concurrent `append` calls each push their
//! payload under `pending`, then ONE rotating leader (elected by a single
//! `AtomicBool` CAS) drains the whole window via `mem::take`, issues ONE
//! batched `write()` (level 2) and at most ONE `fsync` (level 3) for the
//! entire window. Followers park on a per-waiter `Notify` until their
//! physical entry reaches the requested tier. So at concurrency N the lock
//! traffic is N pushes + 1 take on `pending`; the file lock is taken once
//! per window by the leader, off-runtime on a blocking thread.
//!
//! Scenarios (sink × concurrency ∈ {1, 4, 16, 64}):
//!   - `mem`            — `WalSink::mem()`, NO I/O. Isolates lock +
//!     coordination cost. If throughput here scales with concurrency, the
//!     locks are NOT the ceiling; if it plateaus far below the syscall sinks,
//!     the locks (or coordination) bind.
//!   - `file_buffered`  — `SegmentSet` File sink, all `Buffered` appends
//!     (write() → OS page cache, NO fsync). Realistic level-2 durability.
//!   - `file_synced`    — `SegmentSet` File sink, all `Synced` appends (one
//!     fsync per window). Level-3 durability; shows fsync dominance.
//!
//! Metric: `Throughput::Elements(N)` → Criterion reports appends/sec.
//! Each Criterion iteration spawns N tasks (one `append` each) on a
//! multi-thread runtime and joins them, so contention is real, not simulated.
//!
//! Bench-only crate API used is fully public: `WalGroupCommit::new`,
//! `WalGroupCommit::append`, `WalSink::mem`, `SegmentSet::open`. No prod-code
//! visibility was widened. The payload is opaque bytes on the append path
//! (only `replay` decodes), so a fixed filler buffer is a faithful frame.

use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use shamir_bench_utils as bu;
use shamir_wal::segment_set::SegmentSet;
use shamir_wal::wal_group_commit::{WalDurability, WalGroupCommit};
use shamir_wal::wal_sink::WalSink;

/// Concurrency levels under test.
const CONCURRENCY: &[usize] = &[1, 4, 16, 64];

/// Per-append payload size (bytes). Small but representative of an encoded
/// MVCC WAL record header + a short field set. The append path treats this
/// as opaque (CRC + frame), so the exact contents are immaterial.
const PAYLOAD_LEN: usize = 128;

/// Large seal threshold so no segment rotation fires inside the timed
/// window (rotation is a rare path, not what we are measuring).
const SEG_MAX_BYTES: u64 = 1 << 30; // 1 GiB

fn rt() -> tokio::runtime::Runtime {
    // Multi-thread: concurrent committers must actually run in parallel so
    // the lock + coordination contention is genuine.
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn payload() -> Vec<u8> {
    vec![0xABu8; PAYLOAD_LEN]
}

/// Run `n` concurrent `append`s at the given tier against `gc`, return the
/// wall-clock for the whole fan-out (one window's worth of work).
async fn fan_out(gc: Arc<WalGroupCommit>, n: usize, tier: WalDurability, version_base: u64) {
    let mut handles = Vec::with_capacity(n);
    for w in 0..n {
        let gc = Arc::clone(&gc);
        let v = version_base + w as u64;
        handles.push(tokio::spawn(async move {
            gc.append(payload(), v, tier).await.expect("wal append");
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
}

// ── mem sink: lock + coordination cost, NO I/O ─────────────────────────────

fn bench_mem(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("wal_append/mem");
    bu::tune(&mut group, 50, 2, 1);

    for &n in CONCURRENCY {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_function(BenchmarkId::new("n", n), |b| {
            b.to_async(&rt).iter_custom(|iters| async move {
                // One sink reused across iters — there is no per-iter teardown
                // for an in-RAM Vec, and reusing it keeps the measurement on
                // the steady-state append path. A fresh GC each iter would only
                // add allocation noise.
                let gc = Arc::new(WalGroupCommit::new(Arc::new(WalSink::mem())));
                let mut total = Duration::ZERO;
                for i in 0..iters {
                    let base = i * (n as u64) + 1;
                    let start = Instant::now();
                    fan_out(Arc::clone(&gc), n, WalDurability::Buffered, base).await;
                    total += start.elapsed();
                }
                total
            });
        });
    }
    group.finish();
}

// ── file sink, Buffered: write() to page cache, NO fsync ───────────────────

fn bench_file_buffered(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("wal_append/file_buffered");
    bu::tune(&mut group, 30, 2, 1);

    for &n in CONCURRENCY {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_function(BenchmarkId::new("n", n), |b| {
            b.to_async(&rt).iter_custom(|iters| async move {
                let dir = tempfile::TempDir::new().expect("tempdir");
                let segset = SegmentSet::open(dir.path().to_path_buf(), SEG_MAX_BYTES)
                    .await
                    .expect("SegmentSet::open");
                let gc = Arc::new(WalGroupCommit::new(Arc::new(WalSink::File(segset))));
                let mut total = Duration::ZERO;
                for i in 0..iters {
                    let base = i * (n as u64) + 1;
                    let start = Instant::now();
                    fan_out(Arc::clone(&gc), n, WalDurability::Buffered, base).await;
                    total += start.elapsed();
                }
                drop(gc);
                drop(dir);
                total
            });
        });
    }
    group.finish();
}

// ── file sink, Synced: one fsync per window ────────────────────────────────

fn bench_file_synced(c: &mut Criterion) {
    let rt = rt();
    let mut group = c.benchmark_group("wal_append/file_synced");
    // fsync is the slowest variant — keep the sample floor modest.
    bu::tune(&mut group, 20, 2, 1);

    for &n in CONCURRENCY {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_function(BenchmarkId::new("n", n), |b| {
            b.to_async(&rt).iter_custom(|iters| async move {
                let dir = tempfile::TempDir::new().expect("tempdir");
                let segset = SegmentSet::open(dir.path().to_path_buf(), SEG_MAX_BYTES)
                    .await
                    .expect("SegmentSet::open");
                let gc = Arc::new(WalGroupCommit::new(Arc::new(WalSink::File(segset))));
                let mut total = Duration::ZERO;
                for i in 0..iters {
                    let base = i * (n as u64) + 1;
                    let start = Instant::now();
                    fan_out(Arc::clone(&gc), n, WalDurability::Synced, base).await;
                    total += start.elapsed();
                }
                drop(gc);
                drop(dir);
                total
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_mem, bench_file_buffered, bench_file_synced);
criterion_main!(benches);
