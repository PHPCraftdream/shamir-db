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
    pub fn from_budget_secs(budget_secs: u64) -> Self {
        let effective = budget_secs.max(1);
        Self {
            inner: Some(DeadlineInner {
                deadline: Instant::now() + Duration::from_secs(effective),
                budget_secs,
            }),
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
