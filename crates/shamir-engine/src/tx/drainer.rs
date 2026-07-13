//! Repo-level background drainer — generalized inflight-WAL recovery run
//! as a continuous loop instead of only once on open (D2 P1d-2a).
//!
//! ## Why this exists
//!
//! P1d-2 moves the expensive `history.transact` (the version-log DATA
//! write) OFF the commit ack-path into a background task. The §8 refinement
//! of `docs/perf/d2-p1d2-subplan.md` observes that this background work is
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

/// Default high-watermark for the drain window. When `window.len() >=` this
/// value, `offer` drops the entry (backpressure). The gap-reseed path in
/// `drain_step` recovers dropped entries from the WAL on the next pass.
const DEFAULT_WINDOW_HIGH_WATERMARK: usize = 64 * 1024;

use scc::TreeIndex;
use shamir_collections::TFxMap;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{KvOp, RecordKey};
use shamir_wal::{WalEntryV2, WalOpV2};
use tokio::sync::Notify;

use crate::repo::RepoInstance;
use crate::tx::materialize::interner_delta_safe_to_truncate;

/// Repo-level single-owner drainer.
///
/// Holds a [`Notify`] so a producer (the commit path, in P1d-2b) can wake
/// the drain loop on every commit, plus a monotonic counter of versions
/// drained so far (telemetry / tests). There is no CAS leader election:
/// exactly one drain task per repo owns the loop, and the only shared
/// state is the WAL inflight tail (read via `wal.recover()`) and the gate's
/// durable watermark (lock-free atomics) — no `Mutex` on the drain path.
pub struct Drainer {
    /// Woken on every commit (P1d-2b) so the loop drains promptly without
    /// busy-polling. The interval in [`spawn`](Self::spawn) is a backstop.
    notify: Notify,
    /// Total versions drained across all `drain_step` calls — telemetry and
    /// a hook for tests to assert progress without reading the gate.
    drained_total: AtomicU64,
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
            window: TreeIndex::new(),
            window_high_watermark: AtomicUsize::new(DEFAULT_WINDOW_HIGH_WATERMARK),
            offer_dropped_total: AtomicU64::new(0),
            recover_calls: AtomicU64::new(0),
            window_depth: AtomicUsize::new(0),
            pending_unsafe: TreeIndex::new(),
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
        let hw = self.window_high_watermark.load(Ordering::Relaxed);
        if self.window_depth.load(Ordering::Relaxed) >= hw {
            // Backpressure: drop, recovery is via drain_step's gap-reseed.
            self.offer_dropped_total.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if self.window.insert(entry.commit_version, entry).is_ok() {
            self.window_depth.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Seed the window from a cold-start `wal.recover()` result.
    /// Called ONCE during Drainer::spawn (or RepoInstance::drainer init).
    pub fn seed_from_recover(&self, entries: Vec<WalEntryV2>) {
        for e in entries {
            let v = e.commit_version;
            if self.window.insert(v, Arc::new(e)).is_ok() {
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
        let guard = scc::ebr::Guard::new();
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
        if self.window.remove(&version) {
            self.window_depth.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Test-only: read the approximate window depth (atomic mirror).
    #[cfg(test)]
    pub fn window_depth(&self) -> usize {
        self.window_depth.load(Ordering::Relaxed)
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
        let gate = repo.tx_gate().await?;
        let vis = gate.last_committed();
        let dur = gate.durable_watermark();
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
        {
            let guard = scc::ebr::Guard::new();
            let mut expected = dur + 1;
            for (k, v) in self.window.range(expected..=vis, &guard) {
                if *k != expected {
                    break; // gap — reseed below
                }
                window_entries.push(Arc::clone(v));
                expected += 1;
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
        if window_entries.is_empty() {
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
            let guard2 = scc::ebr::Guard::new();
            let mut expected2 = dur + 1;
            for (k, v) in self.window.range(expected2..=vis, &guard2) {
                if *k != expected2 {
                    break;
                }
                window_entries.push(Arc::clone(v));
                expected2 += 1;
            }
        }

        if window_entries.is_empty() {
            return Ok(0);
        }

        let wal = repo.repo_wal().await?;

        // ================================================================
        // Phase A: per-entry interner delta + accumulate data ops per table.
        // ================================================================

        // Per-table batch: table_id -> Vec<(commit_version, Vec<KvOp>)>,
        // preserving ascending-v order within each table for LWW.
        let mut table_batches: TFxMap<u64, Vec<(u64, Vec<KvOp>)>> = TFxMap::default();
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
                        phase_a_failed = true;
                        break;
                    }
                }
            }

            // Replay non-MVCC ops (IndexPut/IndexDel/InternerOverlayMerge/
            // CounterDelta) — these go through `replay_v2_op` which handles
            // non-data routing. Data ops (Put/Delete) for MVCC tables are
            // accumulated below instead of going through replay_v2_op.
            for op in &entry.ops {
                match op {
                    WalOpV2::Put { .. } | WalOpV2::Delete { .. } => {
                        // Data ops: handled below via batch accumulation.
                    }
                    _ => {
                        if let Err(e) = crate::tx::recovery::replay_v2_op(op, repo).await {
                            log::warn!(
                                "drain_step: non-data op replay for tx {} \
                                 commit_version {} failed: {e}; stopping this pass",
                                entry.txn_id,
                                v
                            );
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
            let mut this_entry_tables: Vec<u64> = Vec::new();
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
            if let Some(mvcc) = repo
                .per_table_mvcc()
                .read_async(table_id, |_, m| std::sync::Arc::clone(m))
                .await
            {
                if let Err(e) = mvcc.write_committed_batch_to_history(pass).await {
                    log::warn!(
                        "drain_step: batch history write for table {} failed: {e}; \
                         entries touching this table will not be finalized",
                        table_id
                    );
                    failed_tables.insert(*table_id);
                }
            }
            // No MvccStore for this table_id — the table is unattached (system/
            // test); data ops were already handled by replay_v2_op in Phase A
            // (which skips Put/Delete for MVCC tables). Nothing to do here.
        }

        // ================================================================
        // Phase C: per-entry finalization (ascending v, contiguous).
        // ================================================================

        let mut drained = 0usize;

        for (entry, (v, touched_tables)) in window_entries.iter().zip(entry_tables.iter()) {
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
            // call (the ambient background drainer racing this explicit
            // call, both sharing one `Drainer` via `repo.drainer()`, no
            // mutex between them) could observe `durable_watermark == v`
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
                let _ = self.pending_unsafe.insert(*v, delta_max_id.unwrap_or(0));
            }

            // The value is now durable in history -> advance the durable
            // watermark (contiguous; safe to call redundantly).
            //
            // NOTE (CRIT-2): `mark_durable` advancing here does NOT imply the
            // WAL segment holding this entry is safe to truncate — the A5
            // gate above (and `pending_unsafe`, populated before this call)
            // decides that. F6b uses `settle_and_truncate`'s ceiling, not
            // `durable_watermark` directly.
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
            if self.window.remove(v) {
                self.window_depth.fetch_sub(1, Ordering::Relaxed);
            }
        }

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
            repo.per_table_mvcc().scan(|_, mvcc| {
                mvcc.gc_overlay_to(durable);
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
    /// Safe under concurrent callers (the ambient background loop AND any
    /// explicit `drain_all` share one `Drainer` via `repo.drainer()`, with
    /// no mutex between them): `pending_unsafe` is the single authoritative,
    /// lock-free set both converge on, and the ceiling is ALWAYS recomputed
    /// fresh from it plus the current `durable_watermark` — never cached
    /// from a per-call snapshot that a concurrent unsafe entry could poison.
    /// A version is removed from `pending_unsafe` ONLY after an explicit
    /// positive re-check (never optimistically), so two racing callers each
    /// converge on a ceiling that is conservative-or-equal, never unsafe.
    async fn settle_and_truncate(&self, repo: &RepoInstance) -> DbResult<()> {
        let snapshot: Vec<(u64, u64)> = {
            let guard = scc::ebr::Guard::new();
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
                let _ = self.pending_unsafe.remove(&v);
            }
            // Ok(false)/Err: leave it in `pending_unsafe` — still bounds the
            // ceiling below `v`.
        }

        let gate = repo.tx_gate().await?;
        let durable = gate.durable_watermark();
        let min_pending: Option<u64> = {
            let guard = scc::ebr::Guard::new();
            self.pending_unsafe.iter(&guard).next().map(|(k, _)| *k)
        };
        let ceiling = match min_pending {
            Some(min_v) => min_v.saturating_sub(1).min(durable),
            None => durable,
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
        let wal = repo.repo_wal().await?;
        if wal.has_truncatable(ceiling) {
            // F6c crash seam: BEFORE history-flush + unlink. A HARD crash
            // here leaves EVERY sealed segment on disk (nothing unlinked
            // yet) — recovery replays all of them idempotently and loses
            // nothing (the data is also already in `history`).
            crate::tx::commit::maybe_crash("pre_truncate", repo).await;
            repo.flush_all_history().await?;
            let removed = wal.truncate_below(ceiling).await?;
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
    /// shutdown and tests to flush the inflight tail fully.
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
                    if let Err(e) = this.drain_all(&repo).await {
                        log::warn!("drainer background loop: drain_all failed: {e}");
                    }
                } else {
                    break;
                }
            }
        });
    }
}

/// Project a WAL entry's `interner_delta` (`Vec<(scope, name, id)>`) into the
/// single max-id shape the A5 gate consumes (`Option<u64>`). Mirrors
/// `materialize`'s `interner_delta_max_id` capture, sourced here from the
/// durable WAL entry rather than the in-memory `TxContext`.
///
/// Stage I: the interner is per-REPO, so every triple's `id` shares one
/// id-namespace — we just take the max across the whole delta. The first
/// `u64` (the scope constant) is ignored.
fn entry_interner_max_id(entry: &shamir_wal::WalEntryV2) -> Option<u64> {
    entry
        .interner_delta
        .iter()
        .map(|(_scope, _name, id)| *id)
        .max()
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
