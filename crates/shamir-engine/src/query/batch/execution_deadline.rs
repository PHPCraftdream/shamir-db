//! [`ExecutionDeadline`] — cooperative wall-clock budget for a single
//! `execute_batch` call (#666 follow-up redesign).
//!
//! The original #666 fix wrapped the whole `execute_batch_impl` future in
//! `tokio::time::timeout`, which PREEMPTIVELY dropped the execution future
//! wherever it happened to be suspended. That was unsafe on two counts:
//!
//! 1. **`commit_tx` is explicitly non-cancel-safe at the API boundary**
//!    (see its doc comment in `crates/shamir-engine/src/tx/commit.rs`):
//!    dropping its future between a successful Phase-4 `wal.begin` and
//!    Phase 7 leaves the tx DURABLY COMMITTED (recovery replays the WAL
//!    entry on the next open) while the client is told
//!    `Err(ExecutionTimedOut)` — a genuine WAL/in-memory state divergence.
//! 2. **`TxContext` has no `Drop` impl that frees Level-3 pessimistic
//!    locks** — they live in the per-table `MvccStore` and are released
//!    only by the EXPLICIT `release_pessimistic_locks` call sites on the
//!    normal error/commit paths. Dropping the execution future mid-plan
//!    skipped `execute_transactional_impl`'s `Err`-arm cleanup entirely
//!    and leaked the locks permanently (wound-wait makes younger txs wait
//!    unboundedly on the dead holder — the DoS gate became a DoS vector).
//!
//! The redesign replaces preemptive cancellation with COOPERATIVE deadline
//! checkpoints: the deadline is computed once at the public
//! `execute_batch` entry and threaded through the same call chain
//! `depth`/`params` already travel. At existing safe boundaries — before
//! each stage-alias dispatch, before each `ForEach` iteration, at
//! nested-batch entry, and immediately BEFORE `commit_tx` — [`check`]
//! turns an expired budget into an ordinary
//! `Err(BatchError::ExecutionTimedOut)` that flows through the normal
//! return path. Pessimistic-lock release and RAII rollback therefore
//! happen via the EXISTING `Err`-arm machinery, and nothing is ever
//! externally cancelled: once `commit_tx` is entered it always runs to
//! completion, because the deadline is only ever consulted before it is
//! called — never raced against it.
//!
//! Deliberate non-goal: a SINGLE op that stalls forever inside one
//! `.await` (a pathological I/O hang) is not interrupted by checkpoints —
//! that is a different failure class (an I/O-layer liveness concern), and
//! preemptively cancelling it is exactly the unsafe behaviour this
//! redesign removes. The realistic `max_execution_time_secs` threat model
//! — many ops / many `ForEach` iterations accumulating wall-clock time —
//! is fully covered: the batch is stopped at the next unit-of-work
//! boundary and does no further work.
//!
//! # #1085 investigation: a per-op "stuck op" watchdog was tried and
//! reverted — read this before attempting one again
//!
//! The observability gap this non-goal leaves (a single stuck op produces
//! NO log signal at all until — if ever — it returns) is real, and was the
//! subject of a 2026-08-11 investigation (task #1085) into adding a live
//! diagnostic watchdog: a mechanism that logs "op X still running after Ns"
//! WHILE an op is stuck, from outside its own `.await`, without attempting
//! to cancel it (i.e. strictly additive to the correctness picture above).
//!
//! Two implementations were tried at the two call sites that invoke
//! `execute_single_impl`/`QueryRunner::run` (`batch_execute.rs`'s
//! `execute_plan_impl` and `execute_plan_tx_impl`):
//!
//! 1. **Inline `tokio::select!` racing a `tokio::time::interval` against
//!    the op's future**, the op future pinned via `tokio::pin!`. This
//!    inlines the (already large, and — via `execute_single_impl`'s nested
//!    `BatchOp::Batch`/`ForEach` re-entry into the boxed
//!    `execute_batch_impl` and `fk_actions`' cascade recursion up to
//!    `CASCADE_DEPTH_LIMIT` levels — depth-multiplied) op future's full
//!    state directly into the wrapper's own stack frame, at EVERY
//!    recursive call site. Reproduced "has overflowed its stack" in
//!    `shamir-engine`'s FK-cascade/RI-barrier tests.
//! 2. **`Box::pin`-ing the op future before racing it**, to keep the
//!    wrapper's own frame constant-size (mirroring exactly why
//!    `execute_batch_impl` above is boxed). Fixed the FK-cascade failures,
//!    but a BROADER run of `shamir-db`'s integration suite still showed
//!    ~200 unrelated tests (DDL, access-control, rename, temporal — no
//!    deep recursion involved) failing with the SAME "has overflowed its
//!    stack" error, including in complete isolation (single test, not just
//!    under nextest's parallel load).
//!
//! That second result — an ordinary, non-recursive DDL test overflowing
//! the stack from what should be a few extra bytes of locals — indicates
//! the test binaries' available stack budget on this platform (Windows) is
//! ALREADY thin enough that essentially any addition to the per-op hot
//! path's frame size is unsafe, not just deeply-recursive ones. A THIRD
//! attempt (moving the ticker into its own `tokio::spawn`ed task, so the
//! calling frame holds only a `JoinHandle` + `Instant` and the op future is
//! awaited completely unwrapped) reproduced the SAME broad stack-overflow
//! set — meaning the hazard is not specific to any one wrapping technique
//! tried so far.
//!
//! **Before attempting this again:** first measure the actual headroom
//! (how many bytes of margin exist in the relevant test binaries' thread
//! stacks, on Windows specifically — `RUST_MIN_STACK` / a dedicated
//! large-stack OS thread for the watchdog's own execution context may be
//! required regardless of which in-process technique is used), and
//! consider an OUT-OF-PROCESS or OS-thread-based watchdog (a plain
//! `std::thread::spawn` with an explicit large stack size, communicating
//! via a lock-free flag/atomic timestamp the async side updates on each
//! checkpoint) that never touches the async call graph's own stack at all,
//! rather than another in-runtime `tokio::spawn`/`select!` variant. See
//! task #1094 for the tracked follow-up.
//!
//! **2026-08-12 headroom measurement (task #1094, first investigation
//! step):** a throwaway `#[test]`/`#[tokio::test]` pair calling Win32
//! `GetCurrentThreadStackLimits` was run through the SAME `nextest`
//! harness that reproduced the original failures (deleted after use, not
//! committed). Result, measured immediately at the top of the test
//! function on both a plain nextest-spawned test thread and a
//! `#[tokio::test(flavor = "multi_thread")]` worker thread: total reserve
//! = 2,097,152 bytes (2 MiB, Rust's `std::thread` default) on both, with
//! ~2,090,000+ bytes (99.8%+) still free at that measurement point. **This
//! rules out "the OS/runtime hands these threads a thin stack" as the
//! cause** — the starting allocation is the normal, generous 2 MiB
//! default, not something already constrained by test-harness
//! configuration.
//!
//! This redirects the open question: the crash isn't from a small total
//! budget, it's from how much of that 2 MiB is ALREADY consumed by the
//! time execution reaches deep inside one of the ~200 failing DDL/RI/FK
//! tests' call chains — plausibly large, un-`Box`-ed `async fn` state
//! machines stacked many frames deep through the query planner →
//! evaluator → storage-op layers, leaving little margin before any
//! wrapper is added, however small. **Not yet measured:** actual
//! remaining headroom AT the deep point where the previous three attempts
//! crashed (would require temporary instrumentation inside the batch
//! execution path itself, not just at a test's entry frame) — this is the
//! next concrete step before attempting an implementation, in-process or
//! not. An OS-thread-based watchdog (this doc's existing recommendation)
//! sidesteps the question entirely for the watchdog's OWN stack, but does
//! NOT by itself explain or fix why the async call graph's stack is
//! already this close to its limit — that root cause is still open and
//! worth understanding before shipping any fix, in case it points at a
//! genuine (unrelated) call-depth regression worth its own investigation.
//!
//! [`check`]: ExecutionDeadline::check

use std::time::{Duration, Instant};

use crate::query::batch::BatchError;

/// A wall-clock deadline consulted at cooperative checkpoints during batch
/// execution. `Copy` so it threads through the mutually-recursive executor
/// call chain (`execute_batch_impl` / `execute_plan_tx_impl` /
/// `QueryRunner::run`) as plainly as `depth` does.
#[derive(Debug, Clone, Copy)]
pub struct ExecutionDeadline {
    inner: Option<DeadlineInner>,
}

#[derive(Debug, Clone, Copy)]
struct DeadlineInner {
    /// The instant past which every subsequent checkpoint fails.
    deadline: Instant,
    /// The client-supplied budget, echoed verbatim in the error. A raw `0`
    /// is ENFORCED as the minimum 1-second budget (see
    /// [`ExecutionDeadline::from_budget_secs`]) but still REPORTED as `0`,
    /// matching the original #666 error shape.
    budget_secs: u64,
}

impl ExecutionDeadline {
    /// No budget: every [`check`](Self::check) passes. Used by the
    /// interactive-tx path (`execute_in_open_tx` → `execute_plan_tx`),
    /// which #666 deliberately excluded from the single-call wall-clock
    /// budget — an interactive transaction spans multiple client
    /// round-trips, so no single call's duration corresponds to "the whole
    /// transaction's lifetime".
    pub fn unbounded() -> Self {
        Self { inner: None }
    }

    /// Start the clock for a single `execute_batch` call.
    ///
    /// A client-supplied `max_execution_time_secs: 0` is treated as the
    /// smallest valid budget (1 second), NOT as "no timeout" — interpreting
    /// `0` as unlimited would let a client opt out of the DoS gate
    /// entirely, defeating its purpose. (`.max(1)` carried over unchanged
    /// from the original #666 entry point.)
    ///
    /// A huge `budget_secs` (e.g. `u64::MAX` — the sentinel
    /// `QueryLimitsCap::UNLIMITED` uses, reachable when no operator cap
    /// clamps it, such as the embedded/napi `execute_batch` path) must NOT
    /// panic: `Instant`'s `Add<Duration>` panics on overflow, unlike
    /// `tokio::time::timeout`'s internal `checked_add` (which the code this
    /// replaces relied on implicitly). `checked_add` here falls back to
    /// [`unbounded`](Self::unbounded) on overflow — a budget too large to
    /// even represent as a deadline is, for every practical purpose,
    /// equivalent to no budget at all; it can never actually elapse.
    pub fn from_budget_secs(budget_secs: u64) -> Self {
        let effective = budget_secs.max(1);
        match Instant::now().checked_add(Duration::from_secs(effective)) {
            Some(deadline) => Self {
                inner: Some(DeadlineInner {
                    deadline,
                    budget_secs,
                }),
            },
            None => Self::unbounded(),
        }
    }

    /// Cooperative checkpoint: `Err(BatchError::ExecutionTimedOut)` once
    /// the budget has elapsed, `Ok(())` otherwise (and always `Ok` for an
    /// [`unbounded`](Self::unbounded) deadline).
    ///
    /// The returned error is an ORDINARY executor error — callers `?` it
    /// through the normal return path, so it reaches
    /// `execute_transactional_impl`'s existing `Err` arm (which releases
    /// pessimistic locks and never calls `commit_tx`) exactly like any
    /// other op failure. No cancel-safety surface is involved.
    pub fn check(&self) -> Result<(), BatchError> {
        match &self.inner {
            Some(d) if Instant::now() >= d.deadline => Err(BatchError::ExecutionTimedOut {
                budget_secs: d.budget_secs,
            }),
            _ => Ok(()),
        }
    }
}
