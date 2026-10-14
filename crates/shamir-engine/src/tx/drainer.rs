//! Repo-level background drainer — generalized inflight-WAL recovery run
//! as a continuous loop instead of only once on open (D2 P1d-2a).
//!
//! ## Why this exists
//!
//! P1d-2 moves the expensive `history.transact` (the version-log DATA
//! write) OFF the commit ack-path into a background task. The §8 refinement
//! of `docs/dev-artifacts/perf/d2-p1d2-subplan.md` observes that this background work is
//! EXACTLY [`recover_inflight_v2`](crate::tx::recovery::recover_inflight_v2)
//! prowled in a loop: the source of truth is the inflight tail of the WAL
//! (`wal.recover()` → `Vec<WalEntryV2>`, each carrying `commit_version` +
//! ops), and [`replay_v2_entry`](crate::tx::recovery::replay_v2_entry)
//! already routes those ops per-table into history. So the drain step is a
//! generalization of the recovery body — no separate `SegQueue<DrainJob>`
//! (a third copy of the ops) is needed.
//!
//! ## What [`drain_step`](Drainer::drain_step) does, vs recovery
//!
//! `recover_inflight_v2` replays EVERY inflight entry unconditionally (on
//! open, everything visible must be made durable). `drain_step` replays
//! only the entries in the window `durable_watermark < commit_version <=
//! last_committed` (visibility), in ascending `commit_version` order, then:
//!   1. `replay_v2_entry(entry, repo)` → history (idempotent, last-write-wins)
//!   2. `gate.mark_durable(commit_version)` — advance the durable watermark
//!   3. A5 interner-hwm gate, then `wal.commit(txn_id)` — truncate the
//!      inflight marker ONLY when the interner delta is durably covered.
//!
//! Both recovery (cold) and the drainer (warm) converge to the same state.
//! The shared "replay V → history" core is [`replay_v2_entry`]; the A5
//! truncation gate is shared via
//! [`interner_delta_safe_to_truncate`](crate::tx::materialize::interner_delta_safe_to_truncate).
//!
//! ## Current flow (P1d-2b cutover — DONE)
//!
//! The drainer is the SOLE history writer on the warm path. The commit
//! ack-path writes only the overlay (visibility) + the WAL entry, then
//! calls [`offer`](Drainer::offer) + [`wake`](Drainer::wake) to hand
//! the entry to the background drain task. That task runs
//! [`drain_step`](Drainer::drain_step) in a loop:
//!   - **Phase A** — for each entry in the window
//!     `durable_watermark < commit_version <= last_committed` (ascending),
//!     apply its interner delta in memory (A4 keystone) and replay
//!     non-MVCC ops; data ops (Put/Delete) are accumulated per table.
//!   - **Phase B** — ONE `write_committed_batch_to_history` per touched
//!     table (coalesces E entries × T tables down to T history-transact
//!     calls).
//!   - **Phase C** — for each finalized entry: `gate.mark_durable(v)`,
//!     then the A5 interner-hwm gate
//!     ([`interner_delta_safe_to_truncate`](crate::tx::materialize::interner_delta_safe_to_truncate)),
//!     then `wal.commit(txn_id)` to advance the truncation watermark
//!     ONLY when the interner delta is durably covered.
//!
//! Cold recovery ([`recover_inflight_v2`](crate::tx::recovery::recover_inflight_v2))
//! and the warm drainer converge to the same state. The shared
//! "replay V → history" core is [`replay_v2_entry`]; the A5 truncation
//! gate is shared via [`interner_delta_safe_to_truncate`].
//!
//! ## Truncation liveness bound (A5)
//!
//! `drain_step` does NOT force an interner persist itself — the A5 gate's
//! `persisted_high_water()` only advances via the per-table background
//! checkpoint (fires every `INTERNER_CHECKPOINT_INTERVAL`, default 64,
//! commits — see `shamir-tunables`) or a graceful shutdown
//! (`RepoInstance::flush_buffers`, which persists every table's interner
//! synchronously). So a table that references a new interned field id and
//! then commits FEWER than `INTERNER_CHECKPOINT_INTERVAL` more times before
//! going idle forever has its covering WAL segments held un-truncatable
//! until either the checkpoint eventually fires or the process next
//! shuts down gracefully — a bounded, self-healing gap, not unconditional
//! growth. In practice this window is negligible: at the default
//! `WAL_SEGMENT_MAX_BYTES` (8 MiB), a segment holds tens of thousands of
//! records before sealing, so any table with real traffic clears the
//! 64-commit checkpoint interval long before a single segment could even
//! seal. (Previously this doc claimed `drain_step` runs an unconditional
//! forced persist as a "Phase B" step mirroring cold recovery — that never
//! shipped; this section corrects the record. Found during task #572's
//! review of task #571's crash_recovery.rs fix, where a test using a tiny
//! non-default segment cap hit exactly this window.)

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use arc_swap::ArcSwapOption;

/// Default high-watermark for the drain window. When `window.len() >=` this
/// value, `offer` drops the entry (backpressure). The gap-reseed path in
/// `drain_step` recovers dropped entries from the WAL on the next pass.
const DEFAULT_WINDOW_HIGH_WATERMARK: usize = 64 * 1024;

use scc::TreeIndex;
use shamir_collections::{TFxMap, THasher};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{KvOp, RecordKey};
use shamir_wal::{WalEntryV2, WalOpV2, WalTicketState};
use tokio::sync::{Mutex, Notify};

use crate::repo::RepoInstance;
use crate::tx::clean_horizon::repair_pending_drain::entry_interner_max_id;
#[path = "orphan_repair/mod.rs"]
mod orphan_repair;
#[cfg(test)]
use crate::tx::clean_horizon::checkpoint::test_seam;
use crate::tx::materialize::interner_delta_safe_to_truncate;

/// Test-only injection: when set to a non-zero `table_id`, the Phase B
/// index-posting batch write for that table returns a synthetic error
/// instead of calling `Store::transact`, so the "a batch failure surfaces
/// and blocks only the affected table" contract can be exercised
/// deterministically. `InMemoryStore` (the backend the unit tests use)
/// never fails `set`/`remove`/the default `transact`, so there is no
/// natural way to trigger this path from the public API.
///
/// Mirrors `recovery.rs`'s `FAIL_HISTORY_SEED_TX_ID` pattern, keyed here by
/// `table_id` instead of `txn_id` (this batch's own grouping key). Reset to
/// 0 between tests to avoid cross-test bleed. `#[cfg(debug_assertions)]` so
/// a `--release` build pays zero cost — the static is absent and the load
/// branch is elided.
#[cfg(debug_assertions)]
pub(crate) static FAIL_INDEX_BATCH_TABLE_ID: AtomicU64 = AtomicU64::new(0);

/// Repo-level single-owner drainer.
///
/// Holds a [`Notify`] so a producer (the commit path, in P1d-2b) can wake
/// the drain loop on every commit, plus a monotonic counter of versions
/// drained so far (telemetry / tests). One background task per repo owns
/// the loop; explicit drains serialize with it across awaited replay I/O.
/// Producers still offer entries and publish watermarks lock-free.
pub struct Drainer {
    /// Woken on every commit (P1d-2b) so the loop drains promptly without
    /// busy-polling. The interval in [`spawn`](Self::spawn) is a backstop.
    notify: Notify,
    /// Total versions drained across all `drain_step` calls — telemetry and
    /// a hook for tests to assert progress without reading the gate.
    drained_total: AtomicU64,
    orphan_replay_attempts_total: AtomicU64,
    orphan_replay_failures_total: AtomicU64,
    orphan_replay_consecutive_failures: AtomicU64,
    orphan_replay_last_error: ArcSwapOption<String>,
    orphan_repair_metrics: orphan_repair::Metrics,
    /// Per-repo ordered window of inflight WAL entries already offered by
    /// the commit path. Replaces wal.recover() on the drain hot path —
    /// see Op #2 design.
    window: TreeIndex<u64, Arc<WalEntryV2>>,
    /// Op #2 Stage 4: hard cap on the window depth. When `window.len() >=
    /// high_watermark`, `offer` becomes a no-op (the entry is dropped).
    /// Default 64K. Configurable via `set_window_high_watermark`.
    window_high_watermark: AtomicUsize,
    /// Op #2 Stage 4: total entries dropped by `offer` due to backpressure.
    /// Telemetry only — never affects correctness (drops are recovered by
    /// `drain_step`'s gap-reseed path).
    offer_dropped_total: AtomicU64,
    /// PT 2: total `wal.recover()` calls fired from `drain_step`'s
    /// gap-reseed path. Telemetry; load-bearing in regression tests that
    /// assert the new empty-prefix trigger does NOT fire spuriously on
    /// interior aborts.
    recover_calls: AtomicU64,
    /// PT 1: approximate depth of `window`. Atomic mirror so `offer`'s
    /// backpressure check is O(1) instead of `scc::TreeIndex::len()`'s
    /// O(N) full-tree walk (verified in scc-2.4.0 source). Updated at
    /// every site that mutates `window`. Approximate under contention
    /// (multiple offers racing the check + insert) — fine because
    /// `window_high_watermark` is a SOFT cap (Stage 4 contract: drops
    /// recovered via gap-reseed). Bounded slack ≤ concurrent committer
    /// count.
    window_depth: AtomicUsize,
    /// CRIT-2 (#436): every entry version whose A5 interner-hwm gate has
    /// EVER reported unsafe and has not yet been individually reconfirmed
    /// safe. Keyed by `commit_version`, value is the entry's interner-delta
    /// max id (`0` = none, matching `entry_interner_max_id`'s `None`;
    /// interner ids are 1-based — see
    /// `interner_manager.rs::persisted_high_water`).
    ///
    /// This is the SINGLE authoritative source for the truncation ceiling:
    /// `ceiling = min(durable_watermark, min_key(pending_unsafe) - 1)`,
    /// recomputed fresh on every call, never cached as a per-call local. Two
    /// earlier (rejected, @sh-caught) designs both failed under concurrent
    /// `drain_step` callers sharing one `Drainer` via `repo.drainer()` (the
    /// ambient background loop races any explicit `drain_all` caller):
    ///   1. A per-call `truncation_ceiling` local seeded from
    ///      `durable_watermark()` read at call start — poisoned by a
    ///      CONCURRENT call's `mark_durable` advancing the SAME watermark
    ///      for still-unsafe entries before this call computed its ceiling.
    ///   2. A single `unsafe_floor_version` "first-unsafe-only" latch —
    ///      only ever recorded the FIRST entry to trip unsafe; a LATER
    ///      still-unsafe entry in a subsequent pass silently had no floor
    ///      to latch onto (the single-slot floor was already occupied), so
    ///      once the FIRST entry's delta was confirmed safe, truncation
    ///      jumped straight to `durable_watermark` — past the second
    ///      entry's still-unpersisted delta.
    ///
    /// A concurrent map (lock-free, `scc::TreeIndex`) that tracks EVERY
    /// pending-unsafe version and re-derives the ceiling as "one below the
    /// smallest still-pending version" closes both gaps: it is always
    /// exhaustive (every unsafe entry is tracked, not just the first) and
    /// always current (no per-call snapshot to go stale).
    pending_unsafe: TreeIndex<u64, u64>,
    /// `(commit_version, table_token)` pairs whose WAL index ops (`IndexPut` /
    /// `IndexDel`) the commit path ALREADY applied inline (Phase 5c) before the
    /// version became visible. The drain must not replay those: replaying an
    /// older entry's `Set` posting after a newer inline commit removed it
    /// resurrects the posting until the newer entry is replayed too, and the
    /// raw `info_store` write never invalidated the posting cache, so a read
    /// that cached the resurrected list kept serving it after the removal
    /// (SET NULL left `dept_id = 100` matching a row whose FK was already
    /// null — CI flake, ~12% under load). Keys are captured before offer's
    /// backpressure check; cold recovery/async-index entries start unapplied.
    index_applied: scc::HashMap<(u64, u64), (bool, Vec<bytes::Bytes>), THasher>,
    /// Exact info-store key granularity (not table/index): unrelated postings
    /// must still replay. Retained only above the contiguous durable watermark.
    posting_applied: scc::HashMap<(u64, bytes::Bytes), u64, THasher>,
    counter_applied: scc::HashMap<(u64, u64), (), THasher>,
    // Sanctioned async mutex: bounded contention between the background pass
    // and explicit drains; the guard must span awaited replay/history I/O.
    // Lock order: replay -> per-table drain/marker locks. No callee re-enters
    // drain_step/drain_all; truncation uses flush_all_history, not flush_buffers.
    replaying: Mutex<()>,
}

impl Default for Drainer {
    fn default() -> Self {
        Self::new()
    }
}

impl Drainer {
    /// Create a fresh, idle drainer.
    pub fn new() -> Self {
        Self {
            notify: Notify::new(),
            drained_total: AtomicU64::new(0),
            orphan_replay_attempts_total: AtomicU64::new(0),
            orphan_replay_failures_total: AtomicU64::new(0),
            orphan_replay_consecutive_failures: AtomicU64::new(0),
            orphan_replay_last_error: ArcSwapOption::empty(),
            orphan_repair_metrics: orphan_repair::Metrics::default(),
            window: TreeIndex::new(),
            window_high_watermark: AtomicUsize::new(DEFAULT_WINDOW_HIGH_WATERMARK),
            offer_dropped_total: AtomicU64::new(0),
            recover_calls: AtomicU64::new(0),
            window_depth: AtomicUsize::new(0),
            pending_unsafe: TreeIndex::new(),
            index_applied: scc::HashMap::with_hasher(THasher::default()),
            posting_applied: scc::HashMap::with_hasher(THasher::default()),
            counter_applied: scc::HashMap::with_hasher(THasher::default()),
            replaying: Mutex::new(()),
        }
    }

    /// Record that the commit path applied `(commit_version, table_token)`'s
    /// index postings inline, so [`drain_step`](Self::drain_step) skips that
    /// entry's `IndexPut` / `IndexDel` ops for the table. Must be called
    /// before the version is published (the drain only sees published ones).
    pub(crate) fn mark_index_applied(&self, commit_version: u64, table_token: u64) {
        // offer precedes inline materialization, even when the window is full.
        // Clone keys out before touching the second map: never nest bucket guards.
        let keys = {
            let applied = self
                .index_applied
                .entry_sync((commit_version, table_token))
                .and_modify(|state| state.0 = true)
                .or_insert((true, Vec::new()));
            // A batch containing only BumpFtsStats has no WAL posting keys,
            // but must still preserve the existing inline-applied membership.
            applied.get().1.clone()
        };
        for key in keys {
            self.record_posting_applied(commit_version, table_token, key);
        }
    }

    fn record_posting_applied(&self, version: u64, table: u64, key: bytes::Bytes) {
        self.posting_applied
            .entry_sync((table, key))
            .and_modify(|highest| *highest = (*highest).max(version))
            .or_insert(version);
    }

    fn posting_superseded(&self, version: u64, table: u64, key: &bytes::Bytes) -> bool {
        self.posting_applied
            .read_sync(&(table, key.clone()), |_, highest| *highest > version)
            .unwrap_or(false)
    }

    #[cfg(test)]
    pub(crate) fn posting_version_for_test(&self, table: u64, key: &bytes::Bytes) -> Option<u64> {
        self.posting_applied
            .read_sync(&(table, key.clone()), |_, version| *version)
    }

    fn index_applied_inline(&self, commit_version: u64, table_token: u64) -> bool {
        self.index_applied
            .read_sync(&(commit_version, table_token), |_, state| state.0)
            .unwrap_or(false)
    }

    fn prune_index_applied(&self, durable: u64) {
        // The bucket-locked predicates preserve racing newer inline updates.
        // No unresolved orphan can lie in the contiguous durable prefix.
        self.index_applied
            .retain_sync(|(version, _), _| *version > durable);
        self.posting_applied
            .retain_sync(|_, highest| *highest > durable);
    }

    fn clear_index_applied(&self, entry: &WalEntryV2) {
        for op in &entry.ops {
            if let WalOpV2::IndexPut {
                table_id_interned, ..
            }
            | WalOpV2::IndexDel {
                table_id_interned, ..
            } = op
            {
                self.index_applied
                    .remove_sync(&(entry.commit_version, *table_id_interned));
            }
        }
    }

    fn remember_index_keys(&self, entry: &WalEntryV2) {
        let mut keys: TFxMap<u64, Vec<bytes::Bytes>> = TFxMap::default();
        for op in &entry.ops {
            if let WalOpV2::IndexPut {
                table_id_interned,
                key,
                ..
            }
            | WalOpV2::IndexDel {
                table_id_interned,
                key,
                ..
            } = op
            {
                keys.entry(*table_id_interned)
                    .or_default()
                    .push(key.clone());
            }
        }
        for (table, keys) in keys {
            // Reseeding must not erase an already-applied inline marker.
            let _ = self
                .index_applied
                .insert_sync((entry.commit_version, table), (false, keys));
        }
    }

    pub(crate) fn mark_counter_applied(&self, version: u64, table: u64) {
        let _ = self.counter_applied.insert_sync((version, table), ());
    }

    fn counter_applied(&self, version: u64, table: u64) -> bool {
        self.counter_applied.contains_sync(&(version, table))
    }

    /// Best-effort telemetry reads; never initializes repository state.
    pub(crate) fn enrich_diagnostics(&self, snapshot: &mut shamir_tx::GateDiagnostics) {
        snapshot.orphan_replay_attempts_total =
            self.orphan_replay_attempts_total.load(Ordering::Relaxed);
        snapshot.orphan_replay_failures_total =
            self.orphan_replay_failures_total.load(Ordering::Relaxed);
        snapshot.orphan_replay_consecutive_failures = self
            .orphan_replay_consecutive_failures
            .load(Ordering::Relaxed);
        snapshot.orphan_replay_last_error = self
            .orphan_replay_last_error
            .load_full()
            .map(|error| (*error).clone());
        let (repaired, gaps) = self.orphan_repair_metrics.snapshot();
        log::debug!("orphan journal repair totals: repaired={repaired}, gaps={gaps}");
    }

    fn record_orphan_failure(&self, failed: &mut bool, error: impl std::fmt::Display) {
        orphan_repair::record_orphan_failure(self, failed, error);
    }

    #[cfg(test)]
    pub(crate) fn record_orphan_failure_for_test(&self, failed: &mut bool, error: &str) {
        self.record_orphan_failure(failed, error);
    }

    fn record_orphan_table_failure(
        &self,
        entries: &[Arc<WalEntryV2>],
        orphan: Option<u64>,
        table: u64,
        failed: &mut bool,
        error: &DbError,
    ) {
        if orphan.is_some_and(|version| {
            entries.iter().any(|entry| {
                entry.commit_version == version
                    && entry.ops.iter().any(|op| match op {
                        WalOpV2::Put {
                            table_id_interned, ..
                        }
                        | WalOpV2::Delete {
                            table_id_interned, ..
                        }
                        | WalOpV2::IndexPut {
                            table_id_interned, ..
                        }
                        | WalOpV2::IndexDel {
                            table_id_interned, ..
                        } => *table_id_interned == table,
                        _ => false,
                    })
            })
        }) {
            self.record_orphan_failure(failed, error);
        }
    }

    /// Total `wal.recover()` reseed calls fired from `drain_step`.
    pub fn recover_calls(&self) -> u64 {
        self.recover_calls.load(Ordering::Relaxed)
    }

    /// Wake the drain loop. Called by the producer (commit path) in P1d-2b
    /// after publishing a version; a no-op safety net otherwise.
    pub fn wake(&self) {
        self.notify.notify_one();
    }

    /// Total versions drained across the drainer's lifetime.
    pub fn drained_total(&self) -> u64 {
        self.drained_total.load(Ordering::Relaxed)
    }

    /// Offer a WAL entry to the drain window (called by the commit path
    /// AFTER wal.begin_grouped returns durable, Op #2 Stage 2).
    /// Insert is idempotent at the commit_version key. Lock-free.
    ///
    /// PT 1: backpressure check uses `window_depth.load(Relaxed)` —
    /// O(1) atomic — instead of `scc::TreeIndex::len()` which iterates
    /// the whole tree per call (re-introducing the O(N) cliff Op #2
    /// killed). The depth is approximate (racy +∆ ≤ concurrent committer
    /// count) and the cap is soft, so this is safe.
    pub fn offer(&self, entry: Arc<WalEntryV2>) {
        self.remember_index_keys(&entry);
        let hw = self.window_high_watermark.load(Ordering::Relaxed);
        if self.window_depth.load(Ordering::Relaxed) >= hw {
            // Backpressure: drop, recovery is via drain_step's gap-reseed.
            self.offer_dropped_total.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if self.window.insert_sync(entry.commit_version, entry).is_ok() {
            self.window_depth.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Seed the window from a cold-start `wal.recover()` result.
    /// Called ONCE during Drainer::spawn (or RepoInstance::drainer init).
    pub fn seed_from_recover(&self, entries: Vec<WalEntryV2>) {
        for e in entries {
            self.remember_index_keys(&e);
            let v = e.commit_version;
            if self.window.insert_sync(v, Arc::new(e)).is_ok() {
                self.window_depth.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Test-only window length probe (replaces public `len` to keep API tight).
    #[cfg(test)]
    #[allow(clippy::disallowed_methods)] // O(N) ack: test-only accessor; depth mirror is window_depth()
    pub fn window_len(&self) -> usize {
        self.window.len()
    }

    /// Test-only: collect window keys in ascending order.
    #[cfg(test)]
    pub fn window_keys(&self) -> Vec<u64> {
        let guard = scc::Guard::new();
        self.window.iter(&guard).map(|(k, _)| *k).collect()
    }

    /// Test-only: retrieve a window entry by commit_version.
    #[cfg(test)]
    pub fn window_entry(&self, version: u64) -> Option<Arc<WalEntryV2>> {
        self.window.peek_with(&version, |_, v| Arc::clone(v))
    }

    /// Total entries dropped by `offer` due to backpressure (telemetry).
    pub fn offer_dropped_total(&self) -> u64 {
        self.offer_dropped_total.load(Ordering::Relaxed)
    }

    /// Set the window high-watermark (soft cap for `offer` backpressure).
    pub fn set_window_high_watermark(&self, hw: usize) {
        self.window_high_watermark.store(hw, Ordering::Relaxed);
    }

    /// Test-only: read the current window high-watermark.
    #[cfg(test)]
    pub fn window_high_watermark(&self) -> usize {
        self.window_high_watermark.load(Ordering::Relaxed)
    }

    /// Test-only: remove a window entry by commit_version (for gap simulation).
    #[cfg(test)]
    pub(crate) fn window_remove_for_test(&self, version: u64) {
        if self.window.remove_sync(&version) {
            self.window_depth.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Test-only: read the approximate window depth (atomic mirror).
    #[cfg(test)]
    pub fn window_depth(&self) -> usize {
        self.window_depth.load(Ordering::Relaxed)
    }

    /// Test-only: hold the same replay lock used by drain_step.
    #[cfg(test)]
    pub(crate) async fn replay_lock_for_test(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.replaying.lock().await
    }

    /// cancel-safe: NO — multi-step state mutation per entry (replay →
    /// mark_durable → wal.commit). Cancellation mid-loop leaves the durable
    /// watermark partially advanced; replay is idempotent so a re-run
    /// converges, but the step is not atomic. Mirrors `recover_inflight_v2`.
    ///
    /// L1 coalesced drain: three-phase pass that collapses E entries x T tables
    /// from E*T `history.transact` calls down to T calls (one per table).
    ///
    /// **Phase A** (per-entry, ascending v): apply interner delta (A4 keystone,
    /// BEFORE data), replay non-MVCC ops, and accumulate data ops into a
    /// per-table batch `TFxMap<table_id, Vec<(v, Vec<KvOp>)>>`.
    ///
    /// **Phase B** (per table): ONE `write_committed_batch_to_history` per table.
    /// On the first table transact failure, stop and do not finalize any entry
    /// whose data touched a failed table.
    ///
    /// **Phase C** (per-entry, ascending v): for entries whose tables ALL
    /// succeeded: `mark_durable(v)` -> A5 gate -> `wal.commit` -> crash seams.
    /// Stop at the first entry with a failed table (contiguity).
    pub async fn drain_step(&self, repo: &RepoInstance) -> DbResult<usize> {
        let _replay_guard = self.replaying.lock().await;
        let gate = repo.tx_gate().await?;
        let mut unresolved_ceiling = u64::MAX;
        let mut blocked_by_unresolved = false;
        for version in gate.unresolved_wal_versions() {
            let Some((ticket, entry)) = gate.unresolved_wal(version) else {
                continue;
            };
            match ticket.state() {
                WalTicketState::Written | WalTicketState::Acked => {
                    gate.register_written_orphan(version, entry.clone());
                    self.offer(entry);
                    gate.take_unresolved(version);
                }
                WalTicketState::Failed => {
                    if gate.take_unresolved(version).is_some() {
                        gate.abort_unwritten(version);
                        repo.settle_unwritten_version(version).await;
                    }
                }
                WalTicketState::Pending => {
                    blocked_by_unresolved = true;
                    unresolved_ceiling = version.saturating_sub(1);
                    break;
                }
                WalTicketState::Unknown => {
                    gate.set_requires_reopen();
                    log::error!(
                        "drain_step: WAL outcome UNKNOWN at version {version}; \
                         halting at {}; repository requires reopen",
                        version.saturating_sub(1),
                    );
                    blocked_by_unresolved = true;
                    unresolved_ceiling = version.saturating_sub(1);
                    break;
                }
                _ => {
                    gate.set_requires_reopen();
                    log::error!("drain_step: unsupported WAL fate at {version}; reopen required");
                    blocked_by_unresolved = true;
                    unresolved_ceiling = version.saturating_sub(1);
                    break;
                }
            }
        }
        for version in gate.orphaned_post_wal_versions() {
            if version > unresolved_ceiling {
                break;
            }
            if let Some(entry) = gate.orphan_entry(version) {
                self.offer(entry.clone());
                // Retained entries must not depend on recover or the soft offer cap.
                if self.window.insert_sync(version, entry).is_ok() {
                    self.window_depth.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        let visible = gate.last_committed();
        let vis = visible.max(gate.orphan_max()).min(unresolved_ceiling);
        let dur = gate.durable_watermark();
        // Also clean up late async-index markers on otherwise idle passes.
        self.prune_index_applied(dur);
        // Nothing visible is un-durable → nothing new to REPLAY. But CRIT-2
        // (#436): truncation may still be pending on an entry whose A5 gate
        // tripped unsafe in an earlier pass — the interner can catch up
        // without any new commit, so re-settle `pending_unsafe` and attempt
        // truncation here before bailing out.
        if dur >= vis {
            self.settle_and_truncate(repo).await?;
            return Ok(0);
        }

        // Op #2 Stage 3: scan the in-memory window for the contiguous
        // ascending prefix [dur+1 .. vis]. No I/O in steady state.
        let mut window_entries: Vec<Arc<WalEntryV2>> = Vec::new();
        let mut blocked_by_inflight = false;
        {
            let guard = scc::Guard::new();
            for (expected, (k, v)) in (dur + 1..).zip(self.window.range(dur + 1..=vis, &guard)) {
                if *k != expected {
                    break; // genuine missing key — reseed if the prefix is empty
                }
                if gate.is_active_post_wal(*k) || (*k > visible && !gate.is_orphaned_post_wal(*k)) {
                    blocked_by_inflight = true;
                    break;
                }
                window_entries.push(Arc::clone(v));
                if gate.is_orphaned_post_wal(*k) {
                    break;
                }
            }
        }

        // PT 2: gap-reseed fallback fires ONLY when the contiguous prefix
        // is empty. `dur = durable_watermark()` already crosses every
        // leading aborted version (VersionGuard::drop marks aborts on
        // the durable tracker), so `dur+1` is NEVER aborted — it is
        // either in the window or a true dropped-offer gap. A non-empty
        // prefix therefore proves real progress; the next aborted hole
        // above is crossed via the moving watermark on the next pass.
        // The old `len < vis-dur` trigger fired on EVERY interior abort
        // and paid a spurious O(W) wal.recover() that the recover could
        // not fill — aborted versions have no WAL entry.
        // A present in-flight entry is not a gap: wait for a later pass rather
        // than decoding WAL segments that cannot make the entry complete.
        // An unresolved fate also forbids a full WAL decode, even when an
        // earlier published offer is missing. Already-retained RAM prefixes
        // below its ceiling can still replay without guessing the frame fate.
        if window_entries.is_empty() && !blocked_by_inflight && !blocked_by_unresolved {
            let wal = repo.repo_wal().await?;
            let recovered = wal.recover().await?;
            self.recover_calls.fetch_add(1, Ordering::Relaxed);
            // Filter to entries strictly above the current watermark —
            // already-drained entries from the WAL (still inflight per
            // F6 truncation) must not be re-inserted into the window or
            // they leak across drain passes.
            let recovered: Vec<_> = recovered
                .into_iter()
                .filter(|e| e.commit_version > dur)
                .collect();
            self.seed_from_recover(recovered);
            // Retry the window scan once.
            window_entries.clear();
            orphan_repair::rescan_window_prefix(
                self,
                &gate,
                dur,
                vis,
                visible,
                &mut window_entries,
            );
            if window_entries.is_empty() {
                orphan_repair::settle_missing_evidence(self, repo, &gate).await?;
            }
        }

        if window_entries.is_empty() {
            return Ok(0);
        }

        // Warm missing registrations can be restored (remove_table/add_table).
        // Unlike cold recovery, do not discard an orphan on a transient absence.
        // Pin one resolved TableManager across history, postings and counters;
        // never combine an old per_table_mvcc with a replacement registration.
        // Lookup failure excludes the earliest missing orphan and everything
        // after it, but does not hold back its already-published prefix.
        let mut orphan_failed = false;
        let mut orphan_version = None;
        let mut orphan_tables = TFxMap::default();
        let mut replay_prefix = window_entries.len();
        'resolve_orphans: for (position, entry) in window_entries.iter().enumerate() {
            if !gate.is_orphaned_post_wal(entry.commit_version) {
                continue;
            }
            orphan_version = Some(entry.commit_version);
            self.orphan_replay_attempts_total
                .fetch_add(1, Ordering::Relaxed);
            for op in &entry.ops {
                let token = match op {
                    WalOpV2::Put {
                        table_id_interned, ..
                    }
                    | WalOpV2::Delete {
                        table_id_interned, ..
                    }
                    | WalOpV2::IndexPut {
                        table_id_interned, ..
                    }
                    | WalOpV2::IndexDel {
                        table_id_interned, ..
                    }
                    | WalOpV2::CounterDelta {
                        table_id_interned, ..
                    } => *table_id_interned,
                    _ => continue,
                };
                if token == 0 || orphan_tables.contains_key(&token) {
                    continue;
                }
                match repo.table_by_token(token).await {
                    Ok(Some(table)) => {
                        orphan_tables.insert(token, table);
                    }
                    Ok(None) | Err(_) => {
                        self.record_orphan_failure(
                            &mut orphan_failed,
                            format_args!(
                                "orphan {} table {token} unavailable",
                                entry.commit_version,
                            ),
                        );
                        log::warn!(
                            "drain_step: orphan {} table {token} unavailable; retry next pass",
                            entry.commit_version,
                        );
                        replay_prefix = position;
                        break 'resolve_orphans;
                    }
                }
            }
        }
        window_entries.truncate(replay_prefix);
        if window_entries.is_empty() {
            return Ok(0);
        }
        let wal = repo.repo_wal().await.inspect_err(|error| {
            if orphan_version.is_some() && !orphan_failed {
                self.record_orphan_failure(&mut orphan_failed, error);
            }
        })?;

        // ================================================================
        // Phase A: per-entry interner delta + accumulate data ops per table.
        // ================================================================

        // Per-table batch: table_id -> Vec<(commit_version, Vec<KvOp>)>,
        // preserving ascending-v order within each table for LWW.
        let mut table_batches: TFxMap<u64, Vec<(u64, Vec<KvOp>)>> = TFxMap::default();
        // Per-table batch of index postings (IndexPut/IndexDel), flattened
        // into one ascending-order `Vec<KvOp>` per table (NOT grouped by
        // commit_version like `table_batches` above — `info_store` is a
        // plain last-write-wins KV store, not a versioned history, so one
        // flat ordered Vec per table reproduces exactly what the old
        // immediate per-op `replay_v2_op` application would have done, as
        // long as ops are pushed in the same entry/op iteration order).
        // Only postings whose `table_id_interned != 0` (the common case —
        // `wal_ops_from_tx` never emits the `0` broadcast marker) are
        // batched here; broadcast postings still replay immediately below.
        let mut index_batches: TFxMap<u64, Vec<(u64, KvOp)>> = TFxMap::default();
        // Per-entry: which table_ids does this entry touch (for Phase C gating).
        let mut entry_tables: Vec<(u64 /* commit_version */, Vec<u64> /* table_ids */)> =
            Vec::with_capacity(window_entries.len());
        // Track Phase A failure — stop accumulating and drain nothing.
        let mut phase_a_failed = false;

        for entry in &window_entries {
            let v = entry.commit_version;

            // A4-recovery keystone: apply interner delta BEFORE data.
            if !entry.interner_delta.is_empty() {
                match apply_interner_delta(entry, repo).await {
                    Ok(()) => {}
                    Err(e) => {
                        log::warn!(
                            "drain_step: interner delta for tx {} commit_version {} \
                             failed: {e}; stopping this pass",
                            entry.txn_id,
                            v
                        );
                        if orphan_version == Some(v) {
                            self.record_orphan_failure(&mut orphan_failed, &e);
                        }
                        phase_a_failed = true;
                        break;
                    }
                }
            }

            // Route ops that are NOT batched through `replay_v2_op`
            // immediately: broadcast IndexPut/IndexDel (table_id_interned
            // == 0 — never emitted by `wal_ops_from_tx` today, a defensive
            // legacy fallback only), InternerOverlayMerge, and
            // CounterDelta. Per-table IndexPut/IndexDel (the common case)
            // are instead accumulated into `index_batches` below — this was
            // previously one awaited `info_store().set`/`remove` call PLUS
            // one `table_by_token` resolve PER POSTING; now it is ONE
            // resolve + ONE `Store::transact` per table per drain pass,
            // mirroring the existing Put/Delete data-op batching. Data ops
            // (Put/Delete) are handled by the second loop below, unchanged.
            let mut this_entry_index_tables: Vec<u64> = Vec::new();
            for op in &entry.ops {
                match op {
                    WalOpV2::Put { .. } | WalOpV2::Delete { .. } => {
                        // Data ops: handled below via batch accumulation.
                    }
                    WalOpV2::IndexPut {
                        table_id_interned,
                        key,
                        value,
                        ..
                    } if *table_id_interned != 0 => {
                        if self.index_applied_inline(v, *table_id_interned)
                            || self.posting_superseded(v, *table_id_interned, key)
                        {
                            continue;
                        }
                        this_entry_index_tables.push(*table_id_interned);
                        index_batches
                            .entry(*table_id_interned)
                            .or_default()
                            .push((v, KvOp::Set(key.clone().into(), value.clone())));
                    }
                    WalOpV2::IndexDel {
                        table_id_interned,
                        key,
                        ..
                    } if *table_id_interned != 0 => {
                        if self.index_applied_inline(v, *table_id_interned)
                            || self.posting_superseded(v, *table_id_interned, key)
                        {
                            continue;
                        }
                        this_entry_index_tables.push(*table_id_interned);
                        index_batches
                            .entry(*table_id_interned)
                            .or_default()
                            .push((v, KvOp::Remove(key.clone().into())));
                    }
                    WalOpV2::CounterDelta { .. } => {
                        // Orphan counters are best-effort in Phase C so a table
                        // lookup failure stops at this entry, not its prefix.
                    }
                    _ => {
                        if let Err(e) = crate::tx::recovery::replay_v2_op(op, repo).await {
                            log::warn!(
                                "drain_step: non-data op replay for tx {} \
                                 commit_version {} failed: {e}; stopping this pass",
                                entry.txn_id,
                                v
                            );
                            if orphan_version == Some(v) {
                                self.record_orphan_failure(&mut orphan_failed, &e);
                            }
                            phase_a_failed = true;
                            break;
                        }
                    }
                }
            }
            if phase_a_failed {
                break;
            }

            // Accumulate data ops (Put/Delete) grouped by table_id.
            let mut this_entry_tables: Vec<u64> = this_entry_index_tables;
            let mut by_table: TFxMap<u64, Vec<KvOp>> = TFxMap::default();
            for op in &entry.ops {
                let (table_id, kvop) = match op {
                    WalOpV2::Put {
                        table_id_interned,
                        rid,
                        body,
                    } => (
                        *table_id_interned,
                        KvOp::Set(RecordKey::from_slice(rid.as_bytes()), body.clone()),
                    ),
                    WalOpV2::Delete {
                        table_id_interned,
                        rid,
                    } => (
                        *table_id_interned,
                        KvOp::Remove(RecordKey::from_slice(rid.as_bytes())),
                    ),
                    _ => continue,
                };
                by_table.entry(table_id).or_default().push(kvop);
            }
            for (table_id, ops) in by_table {
                this_entry_tables.push(table_id);
                table_batches.entry(table_id).or_default().push((v, ops));
            }
            entry_tables.push((v, this_entry_tables));
        }

        if phase_a_failed {
            // Phase A failed on an entry — do not proceed to Phase B/C.
            // All entries remain inflight for the next pass/recovery.
            return Ok(0);
        }

        // ================================================================
        // Phase B: per-table ONE write_committed_batch_to_history.
        // ================================================================

        // Track which table_ids failed their transact.
        let mut failed_tables: shamir_collections::TFxSet<u64> =
            shamir_collections::TFxSet::default();

        for (table_id, pass) in &table_batches {
            let published_pass = pass
                .last()
                .filter(|(version, _)| gate.is_orphaned_post_wal(*version))
                .map_or(pass.as_slice(), |_| &pass[..pass.len() - 1]);
            // DEADLOCK FIX (same class as #589 / `cells` map commit `7a4abf62`,
            // H1+H2 commit `621776bd`): `read_sync`, NOT `read_async`. The
            // `per_table_mvcc` map is also touched SYNCHRONOUSLY — `read_sync`
            // (version_provider.rs, EVERY Serializable `validate_read_set`
            // commit), `get_sync` (commit.rs pessimistic-lock release;
            // rename-table) and `iter_sync` (flush_all_history; the F6a overlay
            // GC loop right below in this same file) — and EXCLUSIVELY by
            // `insert_sync` (table attach) / `remove_sync` (drop table).
            // `read_async`'s wait is lock-HANDOFF: saa grants the shared bucket
            // lock to the suspended reader TASK, which then holds it while
            // unpolled in tokio's run queue. A DDL exclusive writer
            // (attach/drop) racing sustained drain traffic can park every
            // worker behind that unpolled reader → whole-runtime deadlock.
            // `read_sync`'s bucket lock is held only by a RUNNING thread for a
            // few instructions (an `Arc::clone`), bounding every wait. The fn
            // stays `async`; this call no longer suspends.
            let mvcc = if let Some(table) = orphan_tables.get(table_id) {
                table.mvcc_store()
            } else {
                repo.per_table_mvcc()
                    .read_sync(table_id, |_, m| Arc::clone(m))
            };
            if let Some(mvcc) = mvcc {
                if let Err(e) = mvcc.write_committed_batch_to_history(published_pass).await {
                    log::warn!(
                        "drain_step: batch history write for table {} failed: {e}; \
                         entries touching this table will not be finalized",
                        table_id
                    );
                    self.record_orphan_table_failure(
                        &window_entries,
                        orphan_version,
                        *table_id,
                        &mut orphan_failed,
                        &e,
                    );
                    failed_tables.insert(*table_id);
                }
                continue;
            }

            // BUG FIX (group 1, 2026-08-14 review): `per_table_mvcc` has no
            // entry for this table_id. Phase A does NOT handle Put/Delete for
            // ANY table — it unconditionally skips them and accumulates them
            // here instead (see the loop above). This branch used to be a
            // silent no-op while Phase C finalized the entry anyway
            // (`mark_durable` + `wal.commit`), permanently discarding the
            // write. Two causes reach here: (a) a genuinely non-MVCC table
            // (no production path builds one today — see TASK_GROUPS.md
            // group 1 spot-check note 1), or (b) `RepoInstance::remove_table`
            // concurrently evicted the `per_table_mvcc` entry (DROP TABLE
            // racing an undrained commit) while this entry's ops were still
            // in the drain window.
            //
            // Mirror cold recovery instead (`recovery.rs`'s `replay_v2_op` /
            // `seed_version_cache_for_entry`): resolve the table by token —
            // if it (or an MvccStore attached to it in the meantime) still
            // exists, write straight to its raw `data_store` (idempotent:
            // re-applying an already-landed Set/Remove is a no-op), so warm
            // drain and cold recovery converge. If the token no longer
            // resolves at all, the table was genuinely dropped: its data is
            // moot (the same disposition `drop_table` gives an orphaned
            // `__data__` store), so skipping is correct — not a silent loss.
            match if let Some(table) = orphan_tables.get(table_id) {
                Ok(Some(table.clone()))
            } else {
                repo.table_by_token(*table_id).await
            } {
                Ok(Some(tbl)) => {
                    if let Some(mvcc) = tbl.mvcc_store() {
                        // Race: the table was attached to `per_table_mvcc`
                        // between the read_sync above and this lookup. Route
                        // through the same history write as the primary
                        // branch above.
                        if let Err(e) = mvcc.write_committed_batch_to_history(published_pass).await
                        {
                            log::warn!(
                                "drain_step: batch history write for table {} \
                                 (resolved via table_by_token) failed: {e}; \
                                 entries touching this table will not be \
                                 finalized",
                                table_id
                            );
                            self.record_orphan_table_failure(
                                &window_entries,
                                orphan_version,
                                *table_id,
                                &mut orphan_failed,
                                &e,
                            );
                            failed_tables.insert(*table_id);
                        }
                    } else {
                        // Genuinely unattached: no version log, write the
                        // raw data ops directly (best-effort — attempt every
                        // op, capture the first error, mirroring the
                        // first-err pattern used throughout this crate).
                        let mut first_err: Option<DbError> = None;
                        for (_v, ops) in pass {
                            for op in ops {
                                let res = match op {
                                    KvOp::Set(key, body) => {
                                        tbl.data_store().set(key.clone(), body.clone()).await
                                    }
                                    KvOp::Remove(key) => tbl.data_store().remove(key.clone()).await,
                                };
                                if let Err(e) = res {
                                    if first_err.is_none() {
                                        first_err = Some(e);
                                    }
                                }
                            }
                        }
                        if let Some(e) = first_err {
                            log::warn!(
                                "drain_step: unattached-table data write for \
                                 table {} failed: {e}; entries touching this \
                                 table will not be finalized",
                                table_id
                            );
                            self.record_orphan_table_failure(
                                &window_entries,
                                orphan_version,
                                *table_id,
                                &mut orphan_failed,
                                &e,
                            );
                            failed_tables.insert(*table_id);
                        }
                    }
                }
                Ok(None) => {
                    log::debug!(
                        "drain_step: table {} no longer resolves (dropped); \
                         data ops for this token are moot, skipping",
                        table_id
                    );
                }
                Err(e) => {
                    log::warn!(
                        "drain_step: table_by_token({}) failed while resolving \
                         an unattached-table data batch: {e}; entries touching \
                         this table will not be finalized",
                        table_id
                    );
                    self.record_orphan_table_failure(
                        &window_entries,
                        orphan_version,
                        *table_id,
                        &mut orphan_failed,
                        &e,
                    );
                    failed_tables.insert(*table_id);
                }
            }
        }

        // ================================================================
        // Phase B (index postings): per-table ONE `Store::transact` for
        // every table's accumulated IndexPut/IndexDel ops — was previously
        // one awaited `info_store().set`/`remove` PLUS one `table_by_token`
        // resolve PER POSTING (`recovery.rs`'s `replay_v2_op`); an
        // index-heavy drain window paid E*T awaits instead of T.
        // ================================================================

        for (table_id, ops) in index_batches {
            match if let Some(table) = orphan_tables.get(&table_id) {
                Ok(Some(table.clone()))
            } else {
                repo.table_by_token(table_id).await
            } {
                Ok(Some(tbl)) => {
                    // Exclude inline writers across check -> awaited transact ->
                    // record. Order: replay -> DDL admission -> raise intent ->
                    // drain writers -> unique_write_lock, one table at a time.
                    // No commit lock or map guard is held across this await.
                    // Commit's data-table prelock guards span Phase 5c and
                    // mark_index_applied, then materialize drops them without
                    // acquiring replaying. Index-only manually staged ops do
                    // not participate in that writer-drain protocol.
                    let posting_guards = if orphan_tables.contains_key(&table_id) {
                        Some(
                            tbl.begin_write_barrier(
                                crate::index::write_barrier_flags::SCHEMA_ACTIVATION,
                            )
                            .await,
                        )
                    } else {
                        None
                    };
                    // Recheck after table resolution: inline application may
                    // have advanced a posting since Phase A accumulated it.
                    let ops: Vec<_> = ops
                        .into_iter()
                        .filter(|(version, op)| {
                            let key = match op {
                                KvOp::Set(key, _) | KvOp::Remove(key) => {
                                    bytes::Bytes::copy_from_slice(key.as_ref())
                                }
                            };
                            !self.posting_superseded(*version, table_id, &key)
                        })
                        .collect();
                    let batch_len = ops.len();
                    // Test-only injection (see `FAIL_INDEX_BATCH_TABLE_ID`'s
                    // doc): simulate a persistent index-batch-write failure
                    // for one table so the "failure surfaces and blocks only
                    // that table" contract can be exercised deterministically
                    // — `InMemoryStore.transact` never fails on its own.
                    #[cfg(debug_assertions)]
                    let inject_fail = {
                        let armed = FAIL_INDEX_BATCH_TABLE_ID.load(Ordering::SeqCst);
                        armed != 0 && armed == table_id
                    };
                    #[cfg(not(debug_assertions))]
                    let inject_fail = false;

                    // The replay writes postings straight onto `info_store`,
                    // bypassing `IndexManager::apply_ops` and its posting-cache
                    // invalidation: remember the keys and drop their cached
                    // lists AFTER the write lands.
                    let touched_keys: Vec<bytes::Bytes> = ops
                        .iter()
                        .map(|(_, op)| match op {
                            KvOp::Set(k, _) | KvOp::Remove(k) => {
                                bytes::Bytes::copy_from_slice(k.as_ref())
                            }
                        })
                        .collect();

                    let write_result = if inject_fail {
                        Err(DbError::Internal(format!(
                            "injected index-batch failure for table {table_id} \
                             (test fault vector)"
                        )))
                    } else {
                        tbl.info_store()
                            .transact(ops.iter().map(|(_, op)| op.clone()).collect())
                            .await
                    };
                    if write_result.is_ok() {
                        for ((version, _), key) in ops.iter().zip(&touched_keys) {
                            self.record_posting_applied(*version, table_id, key.clone());
                        }
                        tbl.index_manager_ref()
                            .invalidate_posting_cache_for_keys(touched_keys.iter().map(|k| &k[..]));
                    }

                    if let Some((barrier, lock)) = posting_guards {
                        drop(barrier);
                        drop(lock);
                    }
                    if let Err(e) = write_result {
                        log::warn!(
                            "drain_step: batch index-posting write for table {} \
                             ({} posting op(s)) failed: {e}; entries touching \
                             this table will not be finalized",
                            table_id,
                            batch_len
                        );
                        self.record_orphan_table_failure(
                            &window_entries,
                            orphan_version,
                            table_id,
                            &mut orphan_failed,
                            &e,
                        );
                        failed_tables.insert(table_id);
                    }
                }
                Ok(None) => {
                    log::debug!(
                        "drain_step: table {} no longer resolves (dropped) while \
                         resolving a batched index-posting write; postings for \
                         this token are moot, skipping",
                        table_id
                    );
                }
                Err(e) => {
                    log::warn!(
                        "drain_step: table_by_token({}) failed while resolving a \
                         batched index-posting write: {e}; entries touching this \
                         table will not be finalized",
                        table_id
                    );
                    self.record_orphan_table_failure(
                        &window_entries,
                        orphan_version,
                        table_id,
                        &mut orphan_failed,
                        &e,
                    );
                    failed_tables.insert(table_id);
                }
            }
        }

        // ================================================================
        // Phase C: per-entry finalization (ascending v, contiguous).
        // ================================================================

        let mut drained = 0usize;

        'finalize: for (entry, (v, touched_tables)) in
            window_entries.iter().zip(entry_tables.iter())
        {
            // Contiguity: stop at the first entry that touches a failed table.
            // Entries above this version must not be finalized (the watermark
            // would jump over the gap).
            let any_failed = touched_tables.iter().any(|t| failed_tables.contains(t));
            if any_failed {
                log::warn!(
                    "drain_step: entry tx {} commit_version {} touches a table \
                     whose batch write failed; stopping finalization",
                    entry.txn_id,
                    v
                );
                break;
            }

            // D4 crash seam: data is durable in history but `mark_durable(v)`
            // has NOT yet run. Recovery re-replays idempotently.
            crate::tx::commit::maybe_crash("drain_replay", repo).await;

            // CRIT-2 (@sh round-2 finding): the A5 interner-hwm gate MUST be
            // checked and `pending_unsafe` populated BEFORE `mark_durable`
            // runs, not after. `interner_delta_safe_to_truncate` is a
            // genuine `.await` (over `repo.repo_interner().await?`) — if
            // `mark_durable(*v)` ran first, a CONCURRENT `settle_and_truncate`
            // call (historically the background and explicit replay passes
            // raced; replay is now serialized) could observe durability at v
            // with `pending_unsafe` NOT YET containing `v` — computing a
            // ceiling that includes `v` before this call ever gets to
            // protect it. Checking A5 and inserting into `pending_unsafe`
            // FIRST closes that window: by the time `durable_watermark`
            // advances to cover `v`, `pending_unsafe` already protects it if
            // unsafe.
            let delta_max_id = entry_interner_max_id(entry);
            let a5_safe = match interner_delta_safe_to_truncate(repo, delta_max_id).await {
                Ok(true) => true,
                Ok(false) => {
                    log::debug!(
                        "drain_step: tx {} commit_version {} drained to history but \
                         marker retained pending interner checkpoint (A5)",
                        entry.txn_id,
                        v
                    );
                    false
                }
                Err(e) => {
                    log::warn!(
                        "drain_step: A5 interner-hwm check for tx {} failed: {e}; \
                         conservatively retaining marker",
                        entry.txn_id
                    );
                    false
                }
            };
            if !a5_safe {
                // CRIT-2: record every A5-unsafe entry into the shared
                // `pending_unsafe` set — NOT just the first one — so
                // `settle_and_truncate`'s ceiling computation (`min(durable,
                // min_key(pending_unsafe) - 1)`) is always exhaustive over
                // every still-unpersisted interner delta, not just the
                // earliest. Inserted BEFORE `mark_durable` below (see the
                // comment above) so no concurrent truncation attempt can
                // ever observe `v` as durable without also seeing it as
                // protected.
                let _ = self
                    .pending_unsafe
                    .insert_sync(*v, delta_max_id.unwrap_or(0));
            }

            // Finalize published history writes and complete any orphan tail
            // below before advancing the contiguous durable watermark.
            //
            // NOTE (CRIT-2): `mark_durable` advancing here does NOT imply the
            // WAL segment holding this entry is safe to truncate — the A5
            // gate above (and `pending_unsafe`, populated before this call)
            // decides that. F6b uses `settle_and_truncate`'s ceiling, not
            // `durable_watermark` directly.
            if gate.is_orphaned_post_wal(*v) {
                for op in &entry.ops {
                    let WalOpV2::CounterDelta {
                        table_id_interned,
                        delta,
                    } = op
                    else {
                        continue;
                    };
                    if self.counter_applied(*v, *table_id_interned) {
                        continue;
                    }
                    let Some(tbl) = orphan_tables.get(table_id_interned) else {
                        break 'finalize;
                    };
                    if let Err(e) = tbl.counter().increment(*delta).await {
                        log::warn!(
                            "drain_step: orphan {v} counter for table \
                             {table_id_interned} failed: {e}; metric drift accepted"
                        );
                    }
                    // Failed metrics are settled too; never retry them forever.
                    self.mark_counter_applied(*v, *table_id_interned);
                }
                for (table_id, pass) in &table_batches {
                    let Some(orphan_pass) = pass.last().filter(|(version, _)| version == v) else {
                        continue;
                    };
                    let Some(tbl) = orphan_tables.get(table_id) else {
                        break 'finalize;
                    };
                    if let Some(mvcc) = tbl.mvcc_store() {
                        if let Err(e) = mvcc
                            .write_committed_batch_to_history(std::slice::from_ref(orphan_pass))
                            .await
                        {
                            self.record_orphan_failure(&mut orphan_failed, &e);
                            log::warn!(
                                "drain_step: orphan {v} history for table \
                                 {table_id} failed: {e}; stopping finalization"
                            );
                            break 'finalize;
                        }
                    }
                }
                if !orphan_repair::settle_event(self, repo, entry, &mut orphan_failed).await? {
                    break 'finalize;
                }
                gate.complete_orphaned_post_wal(*v);
                self.orphan_replay_consecutive_failures
                    .store(0, Ordering::Relaxed);
                self.orphan_replay_last_error.store(None);
                #[cfg(test)]
                let inject_marker_failure = crate::tx::commit::TEST_POST_WAL_PRE_PUBLISH_HOOK
                    .get()
                    .is_some_and(|hook| {
                        hook.repo_name == repo.name()
                            && hook.fail_orphan_markers.swap(false, Ordering::AcqRel)
                    });
                #[cfg(not(test))]
                let inject_marker_failure = false;
                let marker_result = if inject_marker_failure {
                    Err(DbError::Internal("injected orphan marker failure".into()))
                } else {
                    crate::tx::commit_phases::persist_markers(repo, &gate, *v).await
                };
                if let Err(e) = marker_result {
                    log::warn!(
                        "drain_step: orphan {v} marker persist failed: {e}; \
                         next commit rewrites monotone markers"
                    );
                }
                gate.forget_orphaned_post_wal(*v);
            }
            gate.mark_durable(*v);
            drained += 1;

            // Truncate the inflight marker now that both mark_durable and
            // the pending_unsafe bookkeeping above are settled for this
            // entry.
            if a5_safe {
                if let Err(e) = wal.commit(entry.txn_id).await {
                    log::warn!(
                        "drain_step: wal.commit(tx {}) failed: {e}; \
                         marker left inflight (data already durable)",
                        entry.txn_id
                    );
                }
            }

            // D2 P1d-2c crash seam: `v` is now fully durable in history and
            // truncation has been attempted. Recovery re-applies idempotently.
            crate::tx::commit::maybe_crash("phase7", repo).await;

            // Op #2 Stage 3: remove the finalized entry from the window so
            // memory does not grow without bound. PT 1: keep the atomic
            // depth mirror in lock-step with the actual tree.
            if self.window.remove_sync(v) {
                self.window_depth.fetch_sub(1, Ordering::Relaxed);
            }
            self.clear_index_applied(entry);
            for op in &entry.ops {
                if let WalOpV2::CounterDelta {
                    table_id_interned, ..
                } = op
                {
                    let _ = self.counter_applied.remove_sync(&(*v, *table_id_interned));
                }
            }
        }

        self.prune_index_applied(gate.durable_watermark());

        if drained > 0 {
            self.drained_total
                .fetch_add(drained as u64, Ordering::Relaxed);

            // D2 P1e — overlay GC. The durable watermark advanced (each
            // `mark_durable` above moved it; read the post-pass value once).
            // Every overlay entry with `version <= durable_watermark` is now
            // durable in `history`, so its overlay copy is redundant and is
            // dropped across ALL per-table overlays. This bounds the overlay to
            // the still-undrained `(durable_watermark, last_committed]` window
            // instead of letting it grow without limit. Lock-free: each
            // `gc_overlay_to` is a B+-tree sweep with no `.await` and no shared
            // mutex (see `MvccStore::gc_overlay_to`).
            //
            // CRIT-2 (#436): overlay GC uses `durable_watermark` (read
            // visibility) — the overlay copy is redundant once the value is in
            // `history`, which is independent of WAL-segment truncation. Only
            // F6b below must use the interner-safe ceiling.
            let durable = gate.durable_watermark();
            repo.per_table_mvcc().iter_sync(|_, mvcc| {
                mvcc.gc_overlay_to(durable);
                true
            });

            // F6b — WAL truncation. `settle_and_truncate` computes the
            // interner-safe ceiling (see `pending_unsafe`'s field doc) and
            // performs the has_truncatable/flush/truncate_below/crash-seam
            // sequence.
            self.settle_and_truncate(repo).await?;
        }
        Ok(drained)
    }

    /// CRIT-2 (#436): re-check every version in `pending_unsafe` against the
    /// CURRENT interner hwm (dropping any that are now confirmed safe), then
    /// compute the truncation ceiling as `min(durable_watermark,
    /// min_key(pending_unsafe) - 1)` and attempt F6b truncation up to it.
    ///
    /// Called both at the end of `drain_step`'s normal per-entry pass AND
    /// from the `dur >= vis` early-return path — the interner can catch up
    /// without any new commit (e.g. a background `InternerManager::persist`
    /// tick), so truncation must be independently re-attempted even when
    /// there is nothing new to replay; otherwise a pending-unsafe entry
    /// would wedge truncation forever once `durable_watermark` reaches
    /// `visibility`.
    ///
    /// Replay callers serialize on `replaying`; this helper is only called
    /// under that guard and never acquires it again. The ceiling is recomputed
    /// from `pending_unsafe` and the current durable watermark on every pass.
    async fn settle_and_truncate(&self, repo: &RepoInstance) -> DbResult<()> {
        // S5c-X PR5b: land repair-pending (channel-full deferred) journal
        // events BEFORE computing the truncation ceiling — an admission here
        // lifts its clamp below, and a still-pending version holds the WAL
        // segment retained until its repaired event is durably checkpointed.
        crate::tx::clean_horizon::repair_pending_drain::drain_repair_pending(repo).await?;
        let snapshot: Vec<(u64, u64)> = {
            let guard = scc::Guard::new();
            self.pending_unsafe
                .iter(&guard)
                .map(|(k, v)| (*k, *v))
                .collect()
        };
        for (v, delta_max_id_raw) in snapshot {
            let delta_max_id = if delta_max_id_raw == 0 {
                None
            } else {
                Some(delta_max_id_raw)
            };
            if matches!(
                interner_delta_safe_to_truncate(repo, delta_max_id).await,
                Ok(true)
            ) {
                log::debug!(
                    "drain_step: pending-unsafe commit_version {v} is now A5-safe; \
                     removing from the pending set"
                );
                let _ = self.pending_unsafe.remove_sync(&v);
            }
            // Ok(false)/Err: leave it in `pending_unsafe` — still bounds the
            // ceiling below `v`.
        }

        let gate = repo.tx_gate().await?;
        let durable = gate.durable_watermark();
        let min_pending: Option<u64> = {
            let guard = scc::Guard::new();
            self.pending_unsafe.iter(&guard).next().map(|(k, _)| *k)
        };
        let ceiling = match min_pending {
            Some(min_v) => min_v.saturating_sub(1).min(durable),
            None => durable,
        };

        // S5c-X PR5b (I2): never let the J-checkpoint or `truncate_below`
        // cross a repair-pending version — the WAL segment holding the still
        // deferred event stays retained until the repaired event is durably
        // checkpointed (`checkpoint_before_truncate` is FIFO after any
        // admitted repaired event, so J never passes an unflushed one).
        let ceiling = match repo.changefeed().await?.feed.repair_pending_min() {
            Some(min_repair) => ceiling.min(min_repair.saturating_sub(1)),
            None => ceiling,
        };

        // F6b — every record with `commit_version <= ceiling` is BOTH
        // durable in `history` AND has its interner delta durably persisted
        // (or has no delta at all), so the sealed WAL segments holding only
        // such records are reclaimable. `has_truncatable` is CHEAP (a
        // lock-held scan of the short sealed list, no I/O) and is USUALLY
        // false: segments are large (`WAL_SEGMENT_MAX_BYTES`), so a sealed
        // segment crosses the ceiling only at a segment boundary, not on
        // every drain pass.
        //
        // ORDER (I1/I2): history-flush BEFORE truncate — `mark_durable`'s
        // write may still sit in the page cache. Before unlinking a sealed
        // segment we `fsync` `history` (narrow seam — `flush_all_history`,
        // NOT `flush_buffers`, which would re-enter `drain_all` and
        // recurse) so a power-loss after the unlink cannot lose the data
        // (I2). Then delete the segments.
        let settled = gate.completion().watermark();
        let checkpoint_ceiling = ceiling.min(settled);
        let wal = repo.repo_wal().await?;
        let _retention_action =
            crate::tx::clean_horizon::retention::enforce_retention(repo, checkpoint_ceiling)
                .await?;
        if wal.has_truncatable(checkpoint_ceiling) {
            // PR2 S5c-X crash seam: BEFORE the clean-horizon checkpoint. A
            // HARD crash here leaves every sealed segment on disk — nothing
            // checkpointed, nothing unlinked.
            crate::tx::commit::maybe_crash("pre_checkpoint", repo).await;
            // PR2 S5c-X: checkpoint the changefeed journal through the
            // ceiling BEFORE any unlink (I2). On failure the counter is
            // bumped inside; skip truncation this pass — the next pass
            // retries.
            if !crate::tx::clean_horizon::checkpoint::checkpoint_before_truncate(repo).await {
                log::warn!(
                    "drain_step: clean-horizon checkpoint failed at ceiling {ceiling}; \
                     truncation deferred to the next pass"
                );
                return Ok(());
            }
            // PR2 S5c-X crash seam: AFTER the checkpoint, BEFORE the
            // history-flush + unlink — recovery must observe the durable J.
            crate::tx::commit::maybe_crash("post_checkpoint", repo).await;
            // F6c crash seam: BEFORE history-flush + unlink. A HARD crash
            // here leaves EVERY sealed segment on disk (nothing unlinked
            // yet) — recovery replays all of them idempotently and loses
            // nothing (the data is also already in `history`).
            crate::tx::commit::maybe_crash("pre_truncate", repo).await;
            repo.flush_all_history().await?;
            let removed = wal.truncate_below(checkpoint_ceiling).await?;
            #[cfg(test)]
            test_seam::record(repo, "truncate");
            // Recheck actual retained bytes after unlink; clear admission refusal only below cap.
            let _retention_action =
                crate::tx::clean_horizon::retention::enforce_retention(repo, checkpoint_ceiling)
                    .await?;
            // F6c crash seam: AFTER a successful truncate. The unlinked
            // segments are durable in `history` (flushed above, I2), so
            // recovery from the survivors is correct and complete.
            crate::tx::commit::maybe_crash("post_truncate", repo).await;
            if removed > 0 {
                log::debug!(
                    "drain_step: truncated {removed} sealed WAL segment(s) below \
                     ceiling {ceiling} (durable {durable}, min_pending_unsafe={min_pending:?})"
                );
            }
        }
        Ok(())
    }

    /// Drain repeatedly until a pass drains nothing. Used for graceful
    /// shutdown and tests to flush the replayable inflight tail fully.
    /// Waits for an already-running replay pass, but stops at an active commit
    /// or a data-relevant replay failure; a later call retries that tail.
    /// Shutdown callers must quiesce commits first.
    ///
    /// Returns the total number of versions drained across all passes.
    pub async fn drain_all(&self, repo: &RepoInstance) -> DbResult<usize> {
        let mut total = 0usize;
        loop {
            let n = self.drain_step(repo).await?;
            if n == 0 {
                break;
            }
            total += n;
        }
        Ok(total)
    }

    /// Spawn the background drain loop as a leak-free task (mirrors
    /// `WalGroupCommit::spawn_background_fsync` / `MemBufferStore`'s flusher).
    ///
    /// ## Lifecycle (the hard part)
    ///
    /// [`RepoInstance`] is `Clone`-of-`Arc`s (every field is `Arc`-shared;
    /// there is NO canonical `Arc<RepoInstance>` to take a `Weak` from). A
    /// task holding an owned `RepoInstance` clone would therefore bump every
    /// inner `Arc` and keep the repo alive forever — a leak for ephemeral
    /// repos (tests create/drop many).
    ///
    /// The fix: `repo` here is a BACKGROUND clone whose `live` token is `None`
    /// (`RepoInstance::clone_for_background`), so it does NOT count toward the
    /// repo's liveness. `live` is a `Weak<()>` of the repo's shared liveness
    /// `Arc<()>`, held STRONGLY only by the real (foreground) repo clones. The
    /// loop parks on the wake/interval, then checks `live.upgrade()`: once the
    /// last foreground clone drops, the strong count hits zero, the upgrade
    /// fails, the loop breaks, and the owned background clone is dropped —
    /// releasing every inner `Arc`. No leak, no cycle (the background clone's
    /// strong ref to the `Drainer` is severed when the loop exits), no
    /// deadlock (the loop only `.await`s I/O + the notify).
    ///
    /// Driven by [`wake`](Self::wake) (commit-path notify) with an `interval`
    /// backstop so a missed wakeup still makes progress. The single-owner
    /// contract (exactly one task per repo) is the caller's responsibility
    /// (spawn once behind the repo's `OnceCell<Arc<Drainer>>`).
    pub fn spawn(self: &Arc<Self>, repo: RepoInstance, live: Weak<()>, interval: Duration) {
        #[cfg(any(test, feature = "test-util"))]
        if crate::tx::commit::TEST_POST_WAL_PRE_PUBLISH_HOOK
            .get()
            .is_some_and(|hook| hook.repo_name == repo.name())
        {
            return;
        }
        let drainer = Arc::downgrade(self);
        tokio::spawn(async move {
            // Op #2 Stage 1: seed the drainer window from WAL recovery on
            // cold start so the window contains all inflight entries before
            // the first drain_step. Best-effort — if the WAL is unreachable
            // (shouldn't happen on a healthy repo) the loop still runs and
            // drain_step falls back to wal.recover() as before.
            if let Some(this) = drainer.upgrade() {
                match repo.repo_wal().await {
                    Ok(wal) => match wal.recover().await {
                        Ok(entries) => this.seed_from_recover(entries),
                        Err(e) => {
                            log::warn!(
                                "drainer spawn: wal.recover() for seed failed: {e}; \
                                 window starts empty"
                            );
                        }
                    },
                    Err(e) => {
                        log::warn!(
                            "drainer spawn: repo_wal() for seed failed: {e}; \
                             window starts empty"
                        );
                    }
                }
            }
            loop {
                // Exit promptly once every foreground repo clone has dropped.
                if live.upgrade().is_none() {
                    break;
                }
                // Park on the next wake OR the interval backstop. Upgrade the
                // drainer just to reach its `notify`; drop it before the work
                // so a quiescent system can release it.
                match drainer.upgrade() {
                    Some(this) => {
                        tokio::select! {
                            _ = this.notify.notified() => {}
                            _ = tokio::time::sleep(interval) => {}
                        }
                    }
                    None => break,
                }
                // Re-check liveness after the park (the last clone may have
                // dropped while we slept) so we never drain a dead repo.
                if live.upgrade().is_none() {
                    break;
                }
                if let Some(this) = drainer.upgrade() {
                    // F-68 (#895) cluster D / task #124 diagnostic
                    // instrumentation: timestamped "tick started"/"tick
                    // finished" pair around this background loop's core
                    // work, per the brief's request to rule out (or
                    // implicate) a concurrently-running reaper/GC task
                    // during `rename_populated_survives_cold_restart`'s
                    // cold-restart cycle. NOT a behavior change — logging
                    // only, no new await point.
                    let tick_started = std::time::Instant::now();
                    log::debug!("drainer background loop: tick started");
                    if let Err(e) = this.drain_all(&repo).await {
                        log::warn!("drainer background loop: drain_all failed: {e}");
                    }
                    log::debug!(
                        "drainer background loop: tick finished after {:?}",
                        tick_started.elapsed()
                    );
                } else {
                    break;
                }
            }
        });
    }
}

/// A4-recovery keystone: apply an entry's interner delta BEFORE data ops.
/// Extracted from `replay_v2_entry` (recovery.rs) for use in the drainer's
/// Phase A. The interner is per-REPO (Stage I); the first u64 of each
/// triple (scope constant) is ignored.
async fn apply_interner_delta(entry: &shamir_wal::WalEntryV2, repo: &RepoInstance) -> DbResult<()> {
    let repo_interner = repo.repo_interner().await?;
    let interner = repo_interner.get().await?;
    for (_scope, name, id) in &entry.interner_delta {
        interner.touch_with_id(name, *id).map_err(|e| {
            DbError::Internal(format!(
                "drain interner delta for tx {} failed: {}",
                entry.txn_id, e
            ))
        })?;
    }
    Ok(())
}
