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
//! Migrated to the fixed-iteration harness (`bench_scale_tool`, `async`
//! feature). Each scenario's sink (`WalGroupCommit` + backing store) is
//! built ONCE outside the timed closure and reused across iterations —
//! exactly as the prior Criterion `iter_custom` reused `gc` across its
//! `iters` loop — so this is plan 1 (`bench_async`) per concurrency level.
//! One `fan_out` call (N concurrent `append`s, joined) is one timed
//! iteration; the harness's fixed iteration count replaces Criterion's
//! `iters` parameter.
//!
//! Bench-only crate API used is fully public: `WalGroupCommit::new`,
//! `WalGroupCommit::append`, `WalSink::mem`, `SegmentSet::open`. No prod-code
//! visibility was widened. The payload is opaque bytes on the append path
//! (only `replay` decodes), so a fixed filler buffer is a faithful frame.

use std::cell::Cell;
use std::hint::black_box;
use std::sync::Arc;

use bench_scale_tool::Harness;
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

fn payload() -> Vec<u8> {
    vec![0xABu8; PAYLOAD_LEN]
}

/// Run `n` concurrent `append`s at the given tier against `gc`.
async fn fan_out(gc: Arc<WalGroupCommit>, n: usize, tier: WalDurability, version_base: u64) {
    let mut handles = Vec::with_capacity(n);
    for w in 0..n {
        let gc = Arc::clone(&gc);
        let v = version_base + w as u64;
        handles.push(tokio::spawn(async move {
            gc.append(payload(), v, tier).await.expect("wal append");
        }));
    }
    for handle in handles {
        handle.await.unwrap();
    }
}

fn main() {
    let mut h = Harness::new("wal_append", env!("CARGO_MANIFEST_DIR"));

    // ── mem sink: lock + coordination cost, NO I/O ─────────────────────────
    for &n in CONCURRENCY {
        // One sink reused across iterations — there is no per-iter teardown
        // for an in-RAM Vec, and reusing it keeps the measurement on the
        // steady-state append path. A fresh GC each iter would only add
        // allocation noise.
        let gc = Arc::new(WalGroupCommit::new(Arc::new(WalSink::mem())));
        let counter = Cell::new(0u64);
        let id = format!("wal_append/mem/n_{n}");
        h.bench_async(&id, move || {
            let i = counter.get();
            counter.set(i + 1);
            let base = i * (n as u64) + 1;
            let gc = Arc::clone(&gc);
            async move {
                black_box(fan_out(gc, n, WalDurability::Buffered, base).await);
            }
        });
    }

    // ── file sink, Buffered: write() to page cache, NO fsync ───────────────
    for &n in CONCURRENCY {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let setup_rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let segset = setup_rt
            .block_on(SegmentSet::open(dir.path().to_path_buf(), SEG_MAX_BYTES))
            .expect("SegmentSet::open");
        let gc = Arc::new(WalGroupCommit::new(Arc::new(WalSink::File(segset))));
        let counter = Cell::new(0u64);
        let id = format!("wal_append/file_buffered/n_{n}");
        // `dir` (TempDir) must outlive every iteration — captured by the
        // closure so it is dropped only when the harness drops the workload.
        h.bench_async(&id, move || {
            let _keep_alive = &dir;
            let i = counter.get();
            counter.set(i + 1);
            let base = i * (n as u64) + 1;
            let gc = Arc::clone(&gc);
            async move {
                black_box(fan_out(gc, n, WalDurability::Buffered, base).await);
            }
        });
    }

    // ── file sink, Synced: one fsync per window ────────────────────────────
    for &n in CONCURRENCY {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let setup_rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let segset = setup_rt
            .block_on(SegmentSet::open(dir.path().to_path_buf(), SEG_MAX_BYTES))
            .expect("SegmentSet::open");
        let gc = Arc::new(WalGroupCommit::new(Arc::new(WalSink::File(segset))));
        let counter = Cell::new(0u64);
        let id = format!("wal_append/file_synced/n_{n}");
        h.bench_async(&id, move || {
            let _keep_alive = &dir;
            let i = counter.get();
            counter.set(i + 1);
            let base = i * (n as u64) + 1;
            let gc = Arc::clone(&gc);
            async move {
                black_box(fan_out(gc, n, WalDurability::Synced, base).await);
            }
        });
    }

    h.run();
}
