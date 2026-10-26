//! RepoTxGate progress/diagnostics surface: the commit-write log (Phase C),
//! visibility waiting (`await_visible`), durable-watermark tracking (P1d),
//! and the diagnostics snapshot. Split out of `super` for file size only —
//! same type, same private-field access (child module of `repo_tx_gate`).

use shamir_collections::TFxMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::{CommitWriteRecord, RepoTxGate, VisibleOutcome};
use crate::completion_tracker::CompletionTracker;
use crate::gate_diagnostics::{GateBlocker, GateBlockerClass, GateDiagnostics};
use crate::pending_commit::PendingCommit;

impl RepoTxGate {
    // ── Stage D: group-commit queue ───────────────────────────────────
    // F-79 (#906): the leader/follower group-commit path that drove this
    // queue was removed in F-54 (#865). Both accessors below are currently
    // DEAD (zero callers — see the `pending_commits` field doc). They are
    // retained as Stage D scaffolding; see the field's doc comment for the
    // sanctioned `std::sync::Mutex` exception and the revival re-audit note.

    /// Enqueue a `PendingCommit` for the next group-commit batch.
    ///
    /// F-79 (#906): DEAD — zero live callers (the F-54/#865 group-commit
    /// removal orphaned this accessor). See `pending_commits` field doc.
    pub fn enqueue_pending(&self, p: PendingCommit) {
        self.pending_commits.lock().unwrap().push(p);
    }

    /// Drain all pending commits, returning them to the leader.
    ///
    /// F-79 (#906): DEAD — zero live callers (the F-54/#865 group-commit
    /// removal orphaned this accessor). See `pending_commits` field doc.
    pub fn drain_pending(&self) -> Vec<PendingCommit> {
        let mut guard = self.pending_commits.lock().unwrap();
        std::mem::take(&mut *guard)
    }

    // ── Phase C: commit-write log ────────────────────────────────────

    /// PROPOSED (Phase C). Append one footprint at publish time.
    ///
    /// Called by `commit_tx::materialize` UNDER `commit_lock`, immediately
    /// after `gate.publish_committed(commit_version)`. The caller already
    /// holds `commit_lock`, so the append is serialised against every other
    /// publish. We use `TreeIndex::insert` (CAS-based, lock-free).
    ///
    /// Zero-overhead invariant: NO-OP off Serializable AND no-op when the
    /// footprint has no touched tables. The caller MUST construct the
    /// footprint via `build_footprint_from_tx` which short-circuits to
    /// empty under Snapshot — but we re-gate here defensively, so this
    /// function is safe to call unconditionally.
    /// Footprints may be recorded before WAL queueing: an eventual abort
    /// leaves only conservative footprints (spurious SSI aborts), never a
    /// missed conflict. Engine ordering is unchanged in this additive stage.
    pub fn record_commit_writes(&self, rec: CommitWriteRecord) {
        if rec.per_table.is_empty() {
            return;
        }
        let _ = self
            .commit_write_log
            .insert_sync(rec.commit_version, Arc::new(rec));
    }

    /// PROPOSED (Phase C). True iff some committer in the window
    /// `(snapshot, last_committed()]` wrote a footprint that conflicts
    /// with `dep`. Called by `pre_commit` Phase 2-bis UNDER `commit_lock`.
    ///
    /// Walks ONLY records with `snapshot < commit_version <= last_committed`.
    /// The upper bound is the publication floor that ALL writers advance:
    /// tx commits via `publish_committed` / watermark sync, non-tx writes
    /// via their successful `guard.commit` materialisation (both feed the
    /// same contiguous completion watermark that `last_committed_version`
    /// tracks). A record above the floor has not yet been marked
    /// materialized — it is "pending" and must NOT be treated as a
    /// concurrent committed write relative to this tx's predicate window.
    ///
    /// Uses `(snapshot + 1)..=last_committed` to express the exclusive lower
    /// / inclusive upper bound (snapshot is always far from `u64::MAX` in
    /// practice — versions are monotonic from a small seed).
    pub fn predicate_conflicts(
        &self,
        dep: &crate::predicate_set::PredicateDep,
        snapshot: u64,
    ) -> bool {
        let guard = scc::Guard::new();
        let last = self.last_committed_version.load(Ordering::Acquire);
        if last <= snapshot {
            // Fast path: no new commits since snapshot.
            return false;
        }
        // `snapshot + 1` is safe: version 0 is the initial seed, and
        // a tx at snapshot u64::MAX cannot exist (versions are monotonic
        // from a small seed and would exhaust before reaching MAX).
        let start = snapshot.saturating_add(1);
        for (_v, rec) in self.commit_write_log.range(start..=last, &guard) {
            if record_conflicts(rec, dep) {
                return true;
            }
        }
        false
    }

    /// Phase C — inverted batch predicate validation.
    ///
    /// Walks the commit window `(snapshot, last_committed()]` **once** and
    /// tests ALL `deps` against each `CommitWriteRecord`, short-circuiting
    /// on the FIRST conflict. This is the order-inverted equivalent of
    /// calling [`predicate_conflicts`](Self::predicate_conflicts) per dep:
    /// the set of conflicts detected is identical (a conflict exists iff
    /// *some* dep conflicts with *some* record in the window), but the cost
    /// drops from O(P×W) to O(W×P_avg) with a single shared EBR guard
    /// instead of P separate `Guard::new()` allocations.
    ///
    /// Returns the index of the FIRST conflicting dep (in `deps` iteration
    /// order), or `None` when no dep conflicts with any record in the window.
    /// The caller formats the dep for the `PhantomConflict` error.
    ///
    /// # Calling contract
    ///
    /// Both call sites (`pre_commit_locked_validate` in the legacy
    /// AsyncIndex path AND `pre_commit_locked` / `commit_tx_lockfree`)
    /// invoke this ONLY for Serializable txs (the caller gates on
    /// `tx.isolation == Serializable`). For Serializable txs the
    /// `commit_tx_lockfree` path takes `gate.commit_lock()` for the
    /// validate→publish window (CRIT-4 / #438: Serializable txs must
    /// serialize that window to prevent write-skew), so in practice
    /// this method runs UNDER `commit_lock` on every live call path.
    /// The lock-free claim that "the lock-free commit path does NOT
    /// hold the lock here" was true before CRIT-4 but is no longer
    /// accurate for the Serializable branch. Snapshot txs never reach
    /// this method (no predicate validation), so the no-lock property
    /// for Snapshot is moot here.
    ///
    /// `deps` is already snapshot-collected by the caller (cloned out of
    /// the lock-free `PredicateSet` via `snapshot_deps`), so this method
    /// holds no `PredicateSet` reader guard.
    pub fn predicate_conflicts_batch(
        &self,
        deps: &[crate::predicate_set::PredicateDep],
        snapshot: u64,
    ) -> Option<usize> {
        debug_assert!(!deps.is_empty(), "caller guards the empty case");
        // Single shared EBR guard for the ENTIRE validation — not one per dep.
        let guard = scc::Guard::new();
        let last = self.last_committed_version.load(Ordering::Acquire);
        if last <= snapshot {
            // Fast path: no new commits since snapshot.
            return None;
        }
        let start = snapshot.saturating_add(1);
        // Inverted loop: walk the window ONCE, test ALL deps per record.
        // Short-circuit on the first (record, dep) pair that conflicts.
        for (_v, rec) in self.commit_write_log.range(start..=last, &guard) {
            for (idx, dep) in deps.iter().enumerate() {
                if record_conflicts(rec, dep) {
                    return Some(idx);
                }
            }
        }
        None
    }

    /// PROPOSED (Phase C). Drop records with `commit_version <= floor`.
    ///
    /// Called by the same GC tick that prunes `MvccStore::version_cache`.
    /// The caller MUST pass `gate.min_alive()` — never anything higher.
    pub fn prune_commit_log_below(&self, floor: u64) -> usize {
        // Use `remove_range` for efficiency.
        let guard = scc::Guard::new();
        let count = self.commit_write_log.range(..=floor, &guard).count();
        self.commit_write_log.remove_range_sync(..=floor);
        count
    }

    /// Number of records currently in the commit-write log.
    /// Useful for telemetry and engine-side tests (GC prune assertions).
    /// Not on any hot path — walks the tree under an EBR guard.
    pub fn commit_log_len(&self) -> usize {
        let guard = scc::Guard::new();
        self.commit_write_log.range::<u64, _>(.., &guard).count()
    }

    /// Highest contiguous fully settled version (Acquire), not the eager
    /// latest-state publication floor.
    pub fn visible_floor(&self) -> u64 {
        self.completion().watermark()
    }

    /// Best-effort progress snapshot using atomic reads and existing registries.
    /// Counts scan sparse completion entries (not the potentially huge H-W
    /// interval). scc reads follow its concurrent-map protocol, not a global
    /// lock; no Mutex/RwLock or commit-path instrumentation is added.
    /// Independent reads are NOT a coherent cut. The oldest candidate is W+1
    /// and may already be settled by the time its ownership is classified.
    pub fn diagnostics(&self) -> GateDiagnostics {
        let visible_floor = self.visible_floor();
        let allocation_high_water = self.version_allocation_high_water_mark();
        let (unsettled_versions, pending_versions, missing_versions) = self
            .completion
            .unsettled_counts(visible_floor, allocation_high_water);
        let oldest_blocker = if allocation_high_water > visible_floor {
            let version = visible_floor + 1; // H>W proves this cannot overflow.
            let class = if self.is_active_post_wal(version) {
                GateBlockerClass::ActiveTail
            } else if self.is_orphaned_post_wal(version) {
                GateBlockerClass::OrphanAwaitingReplay
            } else if self.wal_unresolved.contains_sync(&version) {
                GateBlockerClass::UnresolvedTicket
            } else {
                GateBlockerClass::PendingUnclassified
            };
            Some(GateBlocker {
                version,
                class,
                age: self.oldest_blocker_age(version),
            })
        } else {
            None
        };
        GateDiagnostics {
            allocation_high_water,
            visible_floor,
            last_committed: self.last_committed(),
            durable_watermark: self.durable_watermark(),
            requires_reopen: self.requires_reopen(),
            visibility_waiters: self.completion.waiter_count(),
            oldest_blocker,
            unsettled_versions,
            pending_versions,
            missing_versions,
            ..GateDiagnostics::default()
        }
    }

    fn oldest_blocker_age(&self, version: u64) -> std::time::Duration {
        let now = tokio::time::Instant::now();
        // W is monotonic: stale diagnostic readers must not replace a newer
        // version's observation. Same-version races preserve the first winner.
        self.oldest_observation.rcu(|observed| match observed {
            Some(previous) if previous.0 >= version => Some(Arc::clone(previous)),
            _ => Some(Arc::new((version, now))),
        });
        let observed = self.oldest_observation.load();
        observed
            .as_ref()
            .map_or(std::time::Duration::ZERO, |observed| {
                if observed.0 == version {
                    now.saturating_duration_since(observed.1)
                } else {
                    std::time::Duration::ZERO
                }
            })
    }

    /// Lock-free rolling 5 s warning reservation, scoped to this repository.
    /// Relaxed is sufficient: this atomic publishes no payload, only a deadline.
    fn reserve_visibility_warning(&self) -> bool {
        let now = u64::try_from(self.warning_epoch.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.next_visibility_warning_ns
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                if now >= next && now != u64::MAX {
                    Some(now.saturating_add(5_000_000_000))
                } else {
                    None
                }
            })
            .is_ok()
    }

    #[cfg(test)]
    pub(crate) fn visibility_warning_count(&self) -> u64 {
        self.visibility_warning_count.load(Ordering::Relaxed)
    }

    /// Wait until every version through `v` is materialized or aborted.
    ///
    /// A 50 ms park slice detects reopen even without completion progress.
    /// After 5 s, count once per wait and warn at most once per repository per
    /// 5 s, but NEVER give up on visibility. Existing SeqCst registration and
    /// release RMWs are preserved; diagnostics add no hot-path RMW/allocation.
    /// If both conditions hold, visibility takes precedence.
    ///
    /// cancel-safe: yes — the only suspension is a notification under a
    /// bounded timeout; dropping it releases the notification registration,
    /// and the RAII guard restores the waiter count. No version state is
    /// modified by this future.
    pub async fn await_visible(&self, v: u64) -> VisibleOutcome {
        const PARK_SLICE: std::time::Duration = std::time::Duration::from_millis(50);
        const SLOW_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

        // Already-visible calls touch only an Acquire load: no RMW, allocation,
        // notification registration, or timer construction.
        if self.visible_floor() >= v {
            return VisibleOutcome::Visible;
        }
        if self.requires_reopen() {
            return VisibleOutcome::ReopenRequired;
        }

        let tracker = self.completion();
        let _registration = tracker.register_waiter();
        let started = tokio::time::Instant::now();
        let mut warned = false;
        loop {
            let notified = tracker.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            // SeqCst is required here, not just Acquire: together with the
            // publisher's CAS/load and registration RMW it rules out BOTH
            // the publisher seeing zero and this recheck seeing the old floor.
            if tracker.registered_watermark() >= v {
                return VisibleOutcome::Visible;
            }
            if self.requires_reopen() {
                return VisibleOutcome::ReopenRequired;
            }
            if started.elapsed() >= SLOW_WAIT {
                if !warned {
                    warned = true;
                    tracker.record_slow_visible_wait();
                }
                if self.reserve_visibility_warning() {
                    let diagnostics = self.diagnostics();
                    #[cfg(test)]
                    self.visibility_warning_count
                        .fetch_add(1, Ordering::Relaxed);
                    log::warn!(
                        "visibility wait exceeded 5 s: target={v}, floor={}, waiters={}, \
                         oldest_blocker={:?}",
                        diagnostics.visible_floor,
                        diagnostics.visibility_waiters,
                        diagnostics.oldest_blocker,
                    );
                }
            }
            // Expiry only triggers a predicate recheck, never a failed wait.
            let _ = tokio::time::timeout(PARK_SLICE, notified.as_mut()).await;
            // Each iteration creates and enables a fresh Notified, including
            // after a timer wake; a completed notification is never re-polled.
        }
    }

    /// Access the completion tracker (P1a scaffolding; abort-path wired in P1b).
    pub fn completion(&self) -> &CompletionTracker {
        &self.completion
    }

    // ── P1d-1: durable watermark (additive, zero behavior change) ──────────

    /// Highest contiguous version V for which the value is durable in the
    /// history log.
    ///
    /// Decoupled from [`last_committed`](Self::last_committed) since the
    /// P1d-2b cutover: the commit ack-path publishes visibility
    /// (`last_committed`) BEFORE the value lands in history, and the
    /// background drainer's [`mark_durable`](Self::mark_durable) advances
    /// this watermark asynchronously once `replay_v2_entry` has written
    /// the version's data + index ops into history. Under steady state
    /// `durable_watermark() <= last_committed()` and the gap is bounded
    /// by the drainer's lag (one drain pass). The truncation gate
    /// (drainer Phase C / F6 segment unlink) refuses to advance past
    /// this watermark, so a sealed WAL segment holding an un-drained
    /// version is never unlinked.
    ///
    /// Invariant: `durable_watermark() <= last_committed()` always — every
    /// site that marks durable does so AFTER the visibility mark
    /// (`guard.commit()`), so visibility either equals or leads durable, never
    /// trails it.
    pub fn durable_watermark(&self) -> u64 {
        self.durable_completion.watermark()
    }

    /// Record that `version`'s value is durable in the history log.
    ///
    /// # Contract — "durable" here means HISTORY-WRITTEN, not fsynced
    ///
    /// **Naming caveat (do not be misled by the name):** "durable" in
    /// this method's name and in [`durable_watermark`](Self::durable_watermark)
    /// means "the version's data + index ops have been written into the
    /// history log (which on buffered backends lands in the OS page cache
    /// / store write-back buffer, NOT necessarily fsynced to disk)". A
    /// real on-disk fsync of history happens only later, at the truncation
    /// gate's checkpoint flush (see `InternerManager::persist`'s CRIT-2
    /// `info_store.flush()` and `RepoInstance::flush_buffers`). The name
    /// is retained because it is on every call site and a rename is a
    /// separate, higher-risk refactor; this doc comment is the contract.
    ///
    /// Concretely, `mark_durable(v)` is called by the drainer's
    /// `drain_step` (Phase C) AFTER `replay_v2_entry` returns Ok for `v`.
    /// It is NOT called from the commit ack-path (which only publishes
    /// visibility); the drainer owns the durable-watermark advance.
    ///
    /// `State::Materialized` is reused as the "durable" terminal state for
    /// this tracker: there is no Aborted analogue (an aborted version was
    /// never written and is irrelevant to durability — but the contiguous
    /// watermark still needs to advance past it). Aborted versions are
    /// marked here too (see
    /// [`mark_durable_aborted`](Self::mark_durable_aborted)) so the
    /// contiguous prefix on the durable tracker matches the visibility
    /// tracker's contiguous prefix.
    ///
    /// Idempotent: marking the same version twice (or marking a version at
    /// or below the current watermark) is a no-op.
    pub fn mark_durable(&self, version: u64) {
        self.durable_completion
            .mark(version, crate::completion_tracker::State::Materialized);
        // D2 P1e: wake any backpressured committer waiting on durable progress.
        // `notify_waiters` wakes ALL currently-registered waiters without
        // storing a permit — paired with the committer registering its
        // `notified()` future BEFORE re-checking the gap (see
        // `apply_backpressure`), there is no lost-wakeup window.
        self.durable_progress.notify_waiters();
    }

    /// D2 P1e — a future that resolves on the next durable-progress signal.
    ///
    /// Backpressure callers register this BEFORE re-reading the gap so a
    /// concurrent [`mark_durable`](Self::mark_durable) cannot slip a wakeup
    /// between the gap check and the park (no lost wakeup). The returned future
    /// borrows the gate; await it (optionally under a timeout) and loop.
    pub fn durable_notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.durable_progress.notified()
    }

    /// Record that `version` will never be durable (aborted before any
    /// physical write), so the contiguous durable watermark advances past
    /// it. Used by the same RAII path that marks the visibility tracker
    /// `Aborted` on early returns; under inline materialize this keeps
    /// `durable_watermark() == last_committed()` even when an SSI / phantom
    /// / WAL-begin abort burns a version.
    pub fn mark_durable_aborted(&self, version: u64) {
        self.durable_completion
            .mark(version, crate::completion_tracker::State::Aborted);
    }
}

/// PROPOSED (Phase C). Conflict-check for ONE committed record.
/// Free function so it stays testable without a `RepoTxGate` instance.
/// Public so the group-commit leader can check batch-local footprints
/// against a survivor's predicates (inter-batch phantom detection, P3a).
pub fn record_conflicts(rec: &CommitWriteRecord, dep: &crate::predicate_set::PredicateDep) -> bool {
    match dep {
        crate::predicate_set::PredicateDep::TableScan { table_token } => {
            rec.per_table.get(table_token).is_some_and(|f| f.touched)
        }
        crate::predicate_set::PredicateDep::IndexRange {
            table_token,
            index_id,
            lo,
            hi,
        } => rec.per_table.get(table_token).is_some_and(|f| {
            // Keys are sorted ascending (build_footprint_from_tx) — use the
            // bounded interval search instead of a linear scan.
            crate::predicate_set::index_range_overlaps_sorted(
                &f.inserted_index_keys,
                *index_id,
                lo,
                hi,
            )
        }),
    }
}

/// PROPOSED (Phase C). Project the on-commit `TxContext` state into a
/// `CommitWriteRecord`. Zero-cost off the common path: returns an empty
/// record whose `per_table` map is empty (no allocations except the
/// stack-resident empty `HashMap`) whenever the tx is Snapshot-isolated AND
/// `footprint_tokens` is empty — the caller's `record_commit_writes`
/// no-ops on the result either way.
///
/// Called by `commit_tx::materialize` AT publish time.
/// Inputs already collected by the existing pipeline:
///   - `tx.index_write_set` (tx_context.rs:72) — SetPosting keys per table.
///   - `tx.write_set`       (tx_context.rs:67) — touched tables (data).
///   - `tx.counter_deltas`  (tx_context.rs:93) — touched tables (counter).
///
/// F-28 Step 5 (S3-C): the per-table inclusion gate is
/// `tx.isolation == Serializable OR tx.footprint_tokens.contains(&token)` —
/// widened from the old whole-function `isolation != Serializable → empty`
/// short-circuit. This lets a plain Snapshot writer publish a footprint for
/// SPECIFIC tables the engine flagged via `TxContext::require_footprint_for`
/// (an FK-child table), without publishing anything for its other tables.
/// Off the common path (`footprint_tokens` empty, Snapshot isolation) this
/// is a single `is_empty()` check per table — same zero-overhead shape as
/// before for every existing caller that never touches `footprint_tokens`.
pub fn build_footprint_from_tx(tx: &crate::TxContext, commit_version: u64) -> CommitWriteRecord {
    let mut rec = CommitWriteRecord {
        commit_version,
        per_table: TFxMap::default(),
    };

    let serializable = tx.isolation == crate::IsolationLevel::Serializable;
    // Zero-overhead invariant: off Serializable, a single `is_empty()` check
    // on the (usually-empty) footprint-tokens set decides everything.
    if !serializable && tx.footprint_tokens.is_empty() {
        return rec;
    }
    let wants_footprint = |token: &u64| serializable || tx.footprint_tokens.contains(token);

    // Touched-bit: any table that ANY non-empty staging touched.
    for token in tx.write_set.keys() {
        if wants_footprint(token) {
            rec.per_table.entry(*token).or_default().touched = true;
        }
    }
    for token in tx.counter_deltas.keys() {
        if wants_footprint(token) {
            rec.per_table.entry(*token).or_default().touched = true;
        }
    }

    // Precise index-posting keys (only SetPosting; RemovePosting does not
    // introduce a phantom in a predicate — coarse `touched` covers it).
    for (token, op) in &tx.index_write_set {
        if !wants_footprint(token) {
            continue;
        }
        // Index ops always touch their table even if SetPosting/Remove split.
        rec.per_table.entry(*token).or_default().touched = true;
        if let crate::IndexWriteOp::SetPosting { key, .. } = op {
            rec.per_table
                .entry(*token)
                .or_default()
                .inserted_index_keys
                .push(key.clone());
        }
    }

    // Sort each table's keys ascending — frees a future binary_search
    // optimisation (doc section 5.4) and makes test assertions stable.
    for f in rec.per_table.values_mut() {
        f.inserted_index_keys.sort_unstable();
    }

    rec
}
