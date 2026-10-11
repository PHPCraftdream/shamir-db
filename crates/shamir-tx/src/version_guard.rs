//! RAII guard for an allocated MVCC version.
//!
//! A [`VersionGuard`] is handed out by
//! [`RepoTxGate::assign_next_version_guarded`](crate::RepoTxGate::assign_next_version_guarded)
//! and owns the obligation to settle its version in the [`CompletionTracker`].
//! Before WAL queueing/acceptance, dropping an armed guard marks both
//! trackers `Aborted`. Ticketed queueing retains the replay entry and resolves
//! Drop from physical fate: written frames become orphans, unwritten frames
//! abort, and ambiguous frames transfer to the gate's unresolved registry.
//! Legacy [`VersionGuard::wal_accepted`] still registers a post-WAL orphan.
//! A successful [`VersionGuard::commit`] marks visibility and disarms it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use shamir_collections::THasher;
use shamir_wal::{WalEntryV2, WalTicket, WalTicketState};

use crate::completion_tracker::{CompletionTracker, State};
use crate::metrics::TxMetrics;
use crate::repo_tx_gate::Unresolved;

/// RAII owner of one allocated MVCC version's terminal-mark obligation.
///
/// Holds `Arc` clones of the gate's shared `CompletionTracker` and
/// `last_committed_version` atomic — never a back-reference to the gate
/// itself — so it can fire its `Drop` independently (mirrors `SnapshotGuard`).
///
/// - On success: call [`commit`](Self::commit) → marks `Materialized` and
///   advances `last_committed_version` from the watermark.
/// - Before WAL queueing/acceptance: early return / panic marks `Aborted`
///   in both trackers and advances the visibility floor.
/// - After [`wal_accepted`](Self::wal_accepted): early return / panic registers
///   an orphan and removes active membership, without aborting either tracker.
///   The drainer owns its remaining replay and completion obligation.
///
/// - After [`wal_queued`](Self::wal_queued): physical ticket fate determines
///   abort, orphan replay, or unresolved handover; ambiguity blocks progress.
///
/// All guard transitions are synchronous (atomics / `scc`, never async).
/// Watermark advance follows `RepoTxGate::sync_last_committed_from_watermark`.
#[must_use = "a VersionGuard must be committed or transfer resolution on drop"]
pub struct VersionGuard {
    version: u64,
    completion: Arc<CompletionTracker>,
    /// `Aborted` when the version is proven unwritten (pre-WAL or FAILED).
    /// Written and unresolved drops leave durability pending.
    /// Successful `commit()` marks only visibility; the engine drainer marks
    /// durability after the physical history write succeeds.
    durable_completion: Arc<CompletionTracker>,
    last_committed: Arc<AtomicU64>,
    armed: bool,
    /// Set by wal_accepted; armed Drop transfers completion to the drainer.
    post_wal: bool,
    queued: Option<Unresolved>,
    orphan_entries: Arc<scc::HashMap<u64, Arc<WalEntryV2>, THasher>>,
    wal_unresolved: Arc<scc::HashMap<u64, Unresolved, THasher>>,
    requires_reopen: Arc<AtomicBool>,
    orphaned_post_wal: Arc<scc::HashMap<u64, (), THasher>>,
    orphan_max: Arc<AtomicU64>,
    active_post_wal: Arc<scc::HashMap<u64, (), THasher>>,
    metrics: Option<Arc<TxMetrics>>,
}

impl VersionGuard {
    /// Construct a guard for an already-allocated `version`.
    ///
    /// Internal: the only sanctioned constructor is
    /// [`RepoTxGate::assign_next_version_guarded`](crate::RepoTxGate::assign_next_version_guarded),
    /// which allocates the version and wires the shared `Arc`s.
    #[allow(clippy::too_many_arguments)] // Shared gate state, no back-reference.
    pub(crate) fn new(
        version: u64,
        completion: Arc<CompletionTracker>,
        durable_completion: Arc<CompletionTracker>,
        last_committed: Arc<AtomicU64>,
        orphaned_post_wal: Arc<scc::HashMap<u64, (), THasher>>,
        orphan_max: Arc<AtomicU64>,
        active_post_wal: Arc<scc::HashMap<u64, (), THasher>>,
        orphan_entries: Arc<scc::HashMap<u64, Arc<WalEntryV2>, THasher>>,
        wal_unresolved: Arc<scc::HashMap<u64, Unresolved, THasher>>,
        requires_reopen: Arc<AtomicBool>,
    ) -> Self {
        Self {
            version,
            completion,
            durable_completion,
            last_committed,
            armed: true,
            post_wal: false,
            queued: None,
            orphan_entries,
            wal_unresolved,
            requires_reopen,
            orphaned_post_wal,
            orphan_max,
            active_post_wal,
            metrics: None,
        }
    }

    /// Attach shared repository telemetry before calling `wal_accepted`.
    /// Standalone gate guards without attachment do not emit orphan metrics.
    /// This does not change the guard's completion or orphan obligations.
    pub fn attach_metrics(&mut self, metrics: Arc<TxMetrics>) {
        self.metrics = Some(metrics);
    }

    /// The MVCC version this guard owns.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Retain the ticket and replay entry BEFORE polling WAL completion.
    /// `append_ticketed` is lazy: even cancellation before its first poll is
    /// conservatively unresolved, not evidence that a frame was unwritten.
    /// Active membership starts here so the drainer cannot race the committer.
    /// Call once per guard with the entry stamped with this guard's version.
    pub fn wal_queued(&mut self, ticket: Arc<WalTicket>, entry: Arc<WalEntryV2>) {
        self.queued = Some((ticket, entry));
        let _ = self.active_post_wal.insert_sync(self.version, ());
    }

    /// Record WAL acceptance before the next suspension point. Until commit
    /// or Drop, active membership prevents the drainer racing materialization.
    /// An armed Drop then registers an orphan rather than aborting WAL data.
    pub fn wal_accepted(&mut self) {
        let _ = self.active_post_wal.insert_sync(self.version, ());
        self.post_wal = true;
    }

    /// Mark the version `Materialized` and advance `last_committed_version`
    /// from the resulting watermark, then disarm so `Drop` is a no-op.
    ///
    /// Consumes the guard: the obligation is discharged exactly once.
    pub fn commit(mut self) {
        self.completion.mark(self.version, State::Materialized);
        self.advance_last_committed();
        self.armed = false;
        let _ = self.active_post_wal.remove_sync(&self.version);
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
            if let Some((ticket, entry)) = self.queued.take() {
                match ticket.state() {
                    WalTicketState::Written | WalTicketState::Acked => {
                        // Publish the entry before orphan membership/bound, and
                        // remove active membership last: replay never sees an
                        // unprotected gap during ownership transfer.
                        let _ = self.orphan_entries.insert_sync(self.version, entry);
                        let _ = self.orphaned_post_wal.insert_sync(self.version, ());
                        if let Some(metrics) = &self.metrics {
                            metrics.on_post_wal_orphan();
                        }
                        self.orphan_max.fetch_max(self.version, Ordering::AcqRel);
                        let _ = self.active_post_wal.remove_sync(&self.version);
                        return;
                    }
                    WalTicketState::Failed => {
                        let _ = self.active_post_wal.remove_sync(&self.version);
                        // Ticket fate overrides even a prior wal_accepted call.
                        self.post_wal = false;
                    }
                    WalTicketState::Pending => {
                        let _ = self
                            .wal_unresolved
                            .insert_sync(self.version, (ticket, entry));
                        let _ = self.active_post_wal.remove_sync(&self.version);
                        return;
                    }
                    WalTicketState::Unknown => {
                        let _ = self
                            .wal_unresolved
                            .insert_sync(self.version, (ticket, entry));
                        self.requires_reopen.store(true, Ordering::Release);
                        let _ = self.active_post_wal.remove_sync(&self.version);
                        return;
                    }
                    // Foreign non-exhaustive states cannot prove absence.
                    _ => {
                        let _ = self
                            .wal_unresolved
                            .insert_sync(self.version, (ticket, entry));
                        self.requires_reopen.store(true, Ordering::Release);
                        let _ = self.active_post_wal.remove_sync(&self.version);
                        return;
                    }
                }
            }
        }
        if self.armed && self.post_wal {
            let _ = self.orphaned_post_wal.insert_sync(self.version, ());
            if let Some(metrics) = &self.metrics {
                metrics.on_post_wal_orphan();
            }
            self.orphan_max.fetch_max(self.version, Ordering::AcqRel);
            let _ = self.active_post_wal.remove_sync(&self.version);
            return;
        }
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
