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
//! ## Scope of P1d-2a (additive, NOT wired)
//!
//! This is a SCAFFOLD like the P1a overlay: the [`Drainer`] is defined and
//! its [`spawn`](Drainer::spawn) helper exists, but it is NOT started from
//! the commit path. The live commit path still writes history inline
//! (materialize Phase 5a) and truncates inline (post_publish_cleanup
//! Phase 7). Running `drain_step` over already-drained, already-truncated
//! state is a no-op (replay is idempotent; `wal.commit` of an absent marker
//! is OK). The cutover that makes the drainer the SOLE history writer is
//! P1d-2b.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use shamir_storage::error::DbResult;
use shamir_types::types::common::THasher;
use tokio::sync::Notify;

use crate::repo::RepoInstance;
use crate::tx::materialize::interner_delta_safe_to_truncate;
use crate::tx::recovery::replay_v2_entry;

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
        }
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

    /// cancel-safe: NO — multi-step state mutation per entry (replay →
    /// mark_durable → wal.commit). Cancellation mid-loop leaves the durable
    /// watermark partially advanced; replay is idempotent so a re-run
    /// converges, but the step is not atomic. Mirrors `recover_inflight_v2`.
    ///
    /// Drain one pass: replay every inflight WAL entry whose `commit_version`
    /// is in the window `durable_watermark < commit_version <=
    /// last_committed` into history, advance the durable watermark, and
    /// truncate the marker (A5-gated). Returns the number of versions drained
    /// this pass.
    ///
    /// This is the generalized body of
    /// [`recover_inflight_v2`](crate::tx::recovery::recover_inflight_v2):
    /// same replay → mark → truncate sequence, but windowed to the
    /// not-yet-durable visible prefix instead of every inflight entry.
    pub async fn drain_step(&self, repo: &RepoInstance) -> DbResult<usize> {
        let gate = repo.tx_gate().await?;
        let vis = gate.last_committed();
        let dur = gate.durable_watermark();
        // Nothing visible is un-durable → nothing to drain.
        if dur >= vis {
            return Ok(0);
        }

        let wal = repo.repo_wal().await?;
        let mut entries = wal.recover().await?;
        // Ascending commit_version: matches recovery (HIGH-5) so last-write-
        // wins ops resolve to the correct final value, AND lets us stop at
        // the first replay failure without skipping a hole in the durable
        // prefix (contiguity).
        entries.sort_by_key(|e| e.commit_version);

        let mut drained = 0usize;
        for entry in &entries {
            let v = entry.commit_version;
            // Below the durable prefix: already drained on a prior pass (or
            // by inline materialize). Above visibility: not yet committed —
            // not ours to drain.
            if v <= dur || v > vis {
                continue;
            }

            // 1) Replay into history (idempotent, last-write-wins). On error
            //    leave the entry inflight: do NOT mark_durable, do NOT
            //    wal.commit, and STOP — a later version must not jump over a
            //    hole in the durable prefix (the contiguous watermark would
            //    not advance past the gap anyway, but we also must not
            //    truncate a higher entry whose lower neighbour is undrained).
            if let Err(e) = replay_v2_entry(entry, repo).await {
                log::warn!(
                    "drain_step: replay of tx {} commit_version {} failed: {e}; \
                     leaving inflight for recovery, stopping this pass",
                    entry.txn_id,
                    v
                );
                break;
            }

            // D4 crash seam: `replay_v2_entry` wrote this entry's ops into
            // `history` (the value is durable) but `mark_durable(v)` below has
            // NOT yet run — the durable watermark does not cover `v` and the WAL
            // entry is still inflight. A HARD crash HERE proves the drain is not
            // atomic but recovery is convergent: `recover_inflight_v2` re-replays
            // the still-inflight entry idempotently (last-write-wins), the data is
            // unchanged, and the durable watermark re-converges to visibility.
            // Zero cost in release builds (see `maybe_crash`).
            crate::tx::commit::maybe_crash("drain_replay", repo).await;

            // 2) The value is now durable in history → advance the durable
            //    watermark (contiguous; safe to call redundantly).
            gate.mark_durable(v);
            drained += 1;

            // 3) A5 interner-hwm gate, then truncate the inflight marker.
            //    `wal.commit` is currently a no-op (markers live in the
            //    segment until F6), but the gate is preserved so the cutover
            //    in P1d-2b inherits it: only truncate when every interner id
            //    in this entry's delta is durably persisted. If not covered,
            //    leave the marker inflight — the data is already durable
            //    (mark_durable above), so this is NOT a durability deferral,
            //    only a truncation deferral (a later checkpoint advances the
            //    hwm and a future pass truncates).
            let delta_max_ids = entry_interner_max_ids(entry);
            match interner_delta_safe_to_truncate(repo, &delta_max_ids).await {
                Ok(true) => {
                    if let Err(e) = wal.commit(entry.txn_id).await {
                        log::warn!(
                            "drain_step: wal.commit(tx {}) failed: {e}; \
                             marker left inflight (data already durable)",
                            entry.txn_id
                        );
                    }
                }
                Ok(false) => {
                    log::debug!(
                        "drain_step: tx {} commit_version {} drained to history but \
                         marker retained pending interner checkpoint (A5)",
                        entry.txn_id,
                        v
                    );
                }
                Err(e) => {
                    log::warn!(
                        "drain_step: A5 interner-hwm check for tx {} failed: {e}; \
                         conservatively retaining marker",
                        entry.txn_id
                    );
                }
            }

            // D2 P1d-2c crash seam: `v` is now fully durable in history and
            // truncation has been attempted — the post-cutover equivalent of
            // the old inline "phase7" (the ack-path no longer reaches it after
            // the cutover). A HARD crash here leaves the data durable in
            // history with the WAL entry still replayable (no-op truncation
            // until F6), so recovery re-applies idempotently. Zero cost in
            // release builds (see `maybe_crash`).
            crate::tx::commit::maybe_crash("phase7", repo).await;
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
            let durable = gate.durable_watermark();
            repo.per_table_mvcc().scan(|_, mvcc| {
                mvcc.gc_overlay_to(durable);
            });

            // F6b — WAL truncation. Every record with `commit_version <=
            // durable` is now durable in `history`, so the sealed WAL
            // segments holding only such records are reclaimable. The
            // `has_truncatable` gate is CHEAP (a lock-held scan of the short
            // sealed list, no I/O) and is USUALLY false: segments are large
            // (`WAL_SEGMENT_MAX_BYTES`), so a sealed segment crosses the
            // watermark only at a segment boundary, not on every drain pass.
            //
            // ORDER (I1/I2): history-flush BEFORE truncate. The drainer
            // already advanced `durable` only after replaying each entry into
            // `history` (`mark_durable`), but that write may sit in the page
            // cache. Before unlinking a sealed segment we `fsync` `history`
            // (narrow seam — `flush_all_history`, NOT `flush_buffers`, which
            // would re-enter `drain_all` and recurse) so a power-loss after
            // the unlink cannot lose the data (I2). Then delete the segments.
            if wal.has_truncatable(durable) {
                // F6c crash seam: BEFORE history-flush + unlink. A HARD crash
                // here leaves EVERY sealed segment on disk (nothing unlinked
                // yet) — recovery replays all of them idempotently and loses
                // nothing (the data is also already in `history`).
                crate::tx::commit::maybe_crash("pre_truncate", repo).await;
                repo.flush_all_history().await?;
                let removed = wal.truncate_below(durable).await?;
                // F6c crash seam: AFTER a successful truncate. The unlinked
                // segments are durable in `history` (flushed above, I2), so
                // recovery from the survivors is correct and complete.
                crate::tx::commit::maybe_crash("post_truncate", repo).await;
                if removed > 0 {
                    log::debug!(
                        "drain_step: truncated {} sealed WAL segment(s) below durable {}",
                        removed,
                        durable
                    );
                }
            }
        }
        Ok(drained)
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

/// Project a WAL entry's `interner_delta` (`Vec<(token, name, id)>`) into
/// the per-table max-id shape the A5 gate consumes (`Vec<(token, max_id)>`).
/// Mirrors `materialize`'s `interner_delta_max_ids` capture, sourced here
/// from the durable WAL entry rather than the in-memory `TxContext`.
fn entry_interner_max_ids(entry: &shamir_wal::WalEntryV2) -> Vec<(u64, u64)> {
    let mut by_token: HashMap<u64, u64, THasher> = HashMap::default();
    for (token, _name, id) in &entry.interner_delta {
        let e = by_token.entry(*token).or_insert(0);
        if *id > *e {
            *e = *id;
        }
    }
    by_token.into_iter().collect()
}
