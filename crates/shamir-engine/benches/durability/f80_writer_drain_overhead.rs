//! F-80 (#907) — WriterDrainBarrier per-write overhead benchmark.
//!
//! Measures the per-write tax the writer-drain barrier primitive adds to
//! EVERY write on a barriered table, and the DDL-side drain cost. This is
//! the measurement gap flagged in the 2026-07-30 remediation roadmap as a
//! P1 follow-up: F-69 collapsed six OR'd conditions into one packed
//! `Arc<AtomicU8>` (`WriteBarrierFlags`), and F-56 strengthened all four
//! cross-atomic ops to `SeqCst` — but nobody had measured the actual
//! per-op latency before vs after.
//!
//! # What is measured
//!
//! Four cells, all isolating the barrier PRIMITIVE only (no data store,
//! no index, no validator — just the atomics):
//!
//! 1. **`fast_path/packed_word`** — the post-F-69 fast-path tax that runs
//!    unconditionally on every write: `enter_writer` (SeqCst `fetch_add`) +
//!    `WriteBarrierFlags::any_set()` (one SeqCst load, returns `false`) +
//!    guard drop (SeqCst `fetch_sub`). This is THE number that matters most
//!    because it runs on every single write to a barriered table, even when
//!    no DDL is in flight.
//!
//! 2. **`fast_path/six_flag_or`** — simulates the PRE-F-69 shape: same
//!    `enter_writer`/drop, but the flag check is SIX independent SeqCst
//!    `AtomicBool` loads OR'd together (the six conditions the old
//!    `needs_write_barrier()` OR'd). The delta between this cell and cell 1
//!    is F-69's measured win: six loads → one load.
//!
//! 3. **`barriered/drain_idle`** — `WriterDrainBarrier::drain()` with no
//!    in-flight writers. The DDL's common-case drain cost: a single SeqCst
//!    load that reads `0` and returns immediately.
//!
//! 4. **`barriered/drain_with_writer`** — `drain()` with one in-flight
//!    fast-path writer: the setup enters the drain set (active=1), the
//!    routine spawns a task that yields once then drops the guard, then
//!    calls `drain()`. Measures the drain spin cost (`load + yield_now` per
//!    iteration) plus realistic tokio scheduling overhead. NOTE: this cell
//!    is inherently noisy because the spawned dropper task may complete
//!    before `drain()` even loads `active` (a multi-thread-runtime race);
//!    the median across iterations captures the typical spin cost but
//!    individual samples vary. The full-stack writer-contention case
//!    (DDL plus N concurrent writers through the real `TableManager`
//!    and lock acquisition) is covered by `f78_writer_latency`.
//!
//! # Why the `active` counter and the flag word are separate atomics here
//!
//! F-69's doc (`write_barrier_flags.rs`) explains why `enter_writer`'s
//! `active` counter is NOT folded into the packed `WriteBarrierFlags` word:
//! packing would turn the single `fetch_add` into a CAS loop. Cells 1-2
//! measure `enter_writer`'s actual `fetch_add` cost (single locked
//! instruction), confirming that cost stays flat.
//!
//! Run:
//!   CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench cargo bench -p shamir-engine --bench f80_writer_drain_overhead
//!   (calibrate first: ... -- --calibrate 4)

use std::hint::black_box;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::table::writer_drain_barrier::WriterDrainBarrier;
use shamir_index::base_index::write_barrier_flags::WriteBarrierFlags;

fn main() {
    let mut h = Harness::new("f80_writer_drain_overhead", env!("CARGO_MANIFEST_DIR"));

    // ── Cell 1: Fast-path per-write tax (current, post-F-69 packed word) ─
    //
    // enter_writer (SeqCst fetch_add) + any_set (one SeqCst load, false) +
    // guard drop (SeqCst fetch_sub). This is the per-write overhead that
    // runs on EVERY write to a barriered table, unconditionally.
    //
    // `black_box` on both the any_set result and the guard prevents the
    // compiler from eliding the fetch_add/load/fetch_sub chain.
    {
        let barrier = WriterDrainBarrier::new();
        let flags = WriteBarrierFlags::new();
        h.bench("fast_path/packed_word", move || {
            let g = barrier.enter_writer();
            let _barriered = black_box(flags.any_set());
            drop(black_box(g));
        });
    }

    // ── Cell 2: Pre-F-69 comparison (six independent SeqCst AtomicBool
    //    loads OR'd together — the old needs_write_barrier() shape) ──────
    //
    // Same enter_writer/drop as cell 1, but the flag check is six separate
    // SeqCst AtomicBool loads OR'd together instead of one packed-word
    // load. The delta between cell 1 and cell 2 is F-69's measured win
    // (six atomics → one atomic). All six use SeqCst (the ordering every
    // individual flag already had before the merge; the one `Relaxed`
    // `has_unique_indexes` operand that F-69 specifically fixed is not
    // simulated separately — the comparison isolates the LOAD COUNT
    // difference, not the ordering change).
    {
        let barrier = WriterDrainBarrier::new();
        let f0 = Arc::new(AtomicBool::new(false));
        let f1 = Arc::new(AtomicBool::new(false));
        let f2 = Arc::new(AtomicBool::new(false));
        let f3 = Arc::new(AtomicBool::new(false));
        let f4 = Arc::new(AtomicBool::new(false));
        let f5 = Arc::new(AtomicBool::new(false));
        h.bench("fast_path/six_flag_or", move || {
            let g = barrier.enter_writer();
            let _barriered = black_box(
                f0.load(Ordering::SeqCst)
                    | f1.load(Ordering::SeqCst)
                    | f2.load(Ordering::SeqCst)
                    | f3.load(Ordering::SeqCst)
                    | f4.load(Ordering::SeqCst)
                    | f5.load(Ordering::SeqCst),
            );
            drop(black_box(g));
        });
    }

    // ── Cell 3: Barriered drain, idle (no in-flight writers) ────────────
    //
    // The DDL's common-case drain cost: drain() finds active==0 on the
    // first SeqCst load and returns. async because drain() is async
    // (yield_now between spin iterations — not reached here since active
    // is 0 on the first load).
    {
        let barrier = WriterDrainBarrier::new();
        h.bench_async("barriered/drain_idle", move || {
            let b = WriterDrainBarrier::clone(&barrier);
            async move {
                b.drain().await;
            }
        });
    }

    // ── Cell 4: Barriered drain, one in-flight writer ───────────────────
    //
    // Setup (untimed): enter the drain set (active=1), return the guard.
    // Routine (timed): spawn a task that yields once then drops the guard,
    // then call drain(). drain() must spin (load + yield_now) until the
    // spawned task drops the guard and active returns to 0.
    //
    // NOTE: inherently noisy — the spawned dropper may complete on a worker
    // thread before drain() loads active, making some iterations measure
    // only the idle drain cost. The median captures the typical
    // one-spin-iteration case. See the module doc for the full caveat.
    {
        let barrier = WriterDrainBarrier::new();
        h.bench_batched_async(
            "barriered/drain_with_writer",
            {
                let barrier = barrier.clone();
                move || {
                    let barrier = barrier.clone();
                    async move { barrier.enter_writer() }
                }
            },
            {
                let barrier = barrier.clone();
                move |guard| {
                    let barrier = barrier.clone();
                    async move {
                        tokio::spawn(async move {
                            tokio::task::yield_now().await;
                            drop(guard);
                        });
                        barrier.drain().await;
                    }
                }
            },
        );
    }

    h.run();
}
