//! RAII guard for an allocated MVCC version.
//!
//! A [`VersionGuard`] is handed out by
//! [`RepoTxGate::assign_next_version_guarded`](crate::RepoTxGate::assign_next_version_guarded)
//! and owns the obligation to terminally mark its version in the
//! [`CompletionTracker`]. The guard makes the abort-path census invariant
//! (every allocated version is marked exactly once — Materialized on success,
//! Aborted on any early exit or panic) hold **by construction**: a `Drop`
//! reaching the end of any scope without a prior [`VersionGuard::commit`]
//! marks the version `Aborted`, so the contiguous watermark can never wedge
//! at `version - 1` because a marking call was skipped on an error path.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::completion_tracker::{CompletionTracker, State};

/// RAII owner of one allocated MVCC version's terminal-mark obligation.
///
/// Holds `Arc` clones of the gate's shared `CompletionTracker` and
/// `last_committed_version` atomic — never a back-reference to the gate
/// itself — so it can fire its `Drop` independently (mirrors `SnapshotGuard`).
///
/// - On success: call [`commit`](Self::commit) → marks `Materialized` and
///   advances `last_committed_version` from the watermark.
/// - On any early return / panic before `commit`: `Drop` marks `Aborted`
///   and advances `last_committed_version` from the watermark.
///
/// Both transitions are synchronous (atomics / `scc` — never `async`), so
/// the `Drop` is sound. Watermark advance replays the exact semantics of
/// `RepoTxGate::sync_last_committed_from_watermark`.
#[must_use = "a VersionGuard must be committed on success, else it marks the \
              version Aborted on drop"]
pub struct VersionGuard {
    version: u64,
    completion: Arc<CompletionTracker>,
    /// P1d-1: second tracker for "value durable in history". On the abort
    /// path (Drop with `armed == true`) we mark this Aborted so its
    /// contiguous watermark advances past the burned version — otherwise it
    /// would wedge below the visibility watermark on every SSI / phantom /
    /// WAL-begin abort, violating the inline-materialize invariant
    /// `durable_watermark == last_committed`. On the success path
    /// (`commit()`) we do NOT mark durable here — the caller (engine
    /// commit / non-tx write) calls `gate.mark_durable(version)` AFTER the
    /// physical history write succeeds (Phase 5a `Complete` for tx,
    /// `history.set/transact` for non-tx). Until P1d-2 the two marks land
    /// on the same code path so the durable watermark stays in lock-step.
    durable_completion: Arc<CompletionTracker>,
    last_committed: Arc<AtomicU64>,
    armed: bool,
}

impl VersionGuard {
    /// Construct a guard for an already-allocated `version`.
    ///
    /// Internal: the only sanctioned constructor is
    /// [`RepoTxGate::assign_next_version_guarded`](crate::RepoTxGate::assign_next_version_guarded),
    /// which allocates the version and wires the shared `Arc`s.
    pub(crate) fn new(
        version: u64,
        completion: Arc<CompletionTracker>,
        durable_completion: Arc<CompletionTracker>,
        last_committed: Arc<AtomicU64>,
    ) -> Self {
        Self {
            version,
            completion,
            durable_completion,
            last_committed,
            armed: true,
        }
    }

    /// The MVCC version this guard owns.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Mark the version `Materialized` and advance `last_committed_version`
    /// from the resulting watermark, then disarm so `Drop` is a no-op.
    ///
    /// Consumes the guard: the obligation is discharged exactly once.
    pub fn commit(mut self) {
        self.completion.mark(self.version, State::Materialized);
        self.advance_last_committed();
        self.armed = false;
    }

    /// Advance `last_committed_version` to the tracker's current watermark
    /// via a monotonic `fetch_max` — identical to
    /// `RepoTxGate::sync_last_committed_from_watermark`.
    fn advance_last_committed(&self) {
        let wm = self.completion.watermark();
        // Only ever moves the floor forward; coexists with concurrent
        // non-tx writers that also `fetch_max` this atomic.
        self.last_committed.fetch_max(wm, Ordering::AcqRel);
    }
}

impl Drop for VersionGuard {
    fn drop(&mut self) {
        if self.armed {
            // No prior `commit()` reached this scope — the version was
            // allocated but never materialized (SSI/phantom/empty-tx abort,
            // WAL-begin failure, or a panic). Mark it Aborted so the
            // contiguous watermark advances past it.
            self.completion.mark(self.version, State::Aborted);
            // P1d-1: also advance the durable watermark past the aborted
            // version. An aborted version was never written to history, so
            // it cannot block durable contiguity — but if we do not mark it
            // here the durable tracker would wedge at `version - 1` while
            // visibility moves past it, breaking the inline-materialize
            // invariant `durable_watermark == last_committed`.
            self.durable_completion.mark(self.version, State::Aborted);
            self.advance_last_committed();
        }
    }
}
