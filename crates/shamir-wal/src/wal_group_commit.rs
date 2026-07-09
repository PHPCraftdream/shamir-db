//! Two-tier group-commit coordinator over a [`WalSink`] (W2, additive).
//!
//! [`WalGroupCommit`] coalesces many concurrent appends through one
//! rotating leader: a single `AtomicBool` CAS elects the leader, which
//! drains the pending queue and issues ONE
//! [`append_batch`](WalSink::append_batch) (`write()` → OS page cache,
//! level 2) and at most ONE [`sync`](WalSink::sync) (`fsync`, level 3)
//! per window. The rest park until their physical entry reaches the
//! requested durability tier.
//!
//! ## Durability tiers
//!
//! - [`WalDurability::Buffered`] → waiter acks after the batched `write()`
//!   (level 2): survives a process crash, lost only on power loss before a
//!   later `sync`.
//! - [`WalDurability::Synced`] → waiter acks after the batched `fsync()`
//!   (level 3): survives power loss.
//!
//! A window issues an `fsync` IFF it contains at least one `Synced`
//! waiter; a window of only `Buffered` appends needs no `fsync` at all.
//!
//! ## Correctness + liveness
//!
//! This reuses the verified leader/follower structure originally proved in
//! `shamir-tx`'s `group_fsync.rs` (D1b). That file was REMOVED in `e9e48c9`
//! ("remove dead GroupFsync — superseded by file-WAL WalGroupCommit"); the
//! verbatim liveness proof it carried lives on in git at
//! `f62fe18:crates/shamir-tx/src/group_fsync.rs`, and the L1/L2/L3 arguments
//! it established are reproduced below and in
//! `docs/perf/capstone-subplan.md` §1:
//!   - **L1 (no stranded committer):** leadership is released under the SAME
//!     `pending` lock as `push` (in [`WalGroupCommit::lead_until_drained`]) —
//!     a late push is either seen by the current leader or wins leadership
//!     itself, so no entry is ever stranded;
//!   - **L2 (no lost wakeup):** the `enable()`-before-check `Notify` park
//!     loop closes the subscribe-window race;
//!   - **L3 (circuit-breaker):** on a write/sync error the leader releases
//!     leadership and returns, so the next append elects a fresh leader and
//!     no task spins on a dead segment;
//!   - completion is tied to the PHYSICAL entry: the leader that drains a
//!     `(payload, tier, waiter)` tuple is the task that calls
//!     `waiter.complete(..)` once that exact tuple has reached its tier.
//!
//! A single-writer-task replacement for the rotating leader (this `pending`
//! Mutex → bounded MPSC, the `flushing` CAS deleted) is designed in
//! `docs/perf/capstone-subplan.md`. It was PROTOTYPED and REVERTED: a
//! permanent writer task mandates a per-append cross-task `oneshot`
//! round-trip that suspends the executor on every append, whereas this
//! rotating leader drains the in-RAM `Mem` sink synchronously within one
//! poll. That regressed mem N=1 latency ~+22% (the subplan §0 GO/NO-GO
//! criterion) and broke an atomicity property the engine commit path relies
//! on (the non-yielding Mem append keeps a commit atomic on a current-thread
//! runtime; the mandatory yield let concurrent SSI committers all validate
//! before any published). The subtraction is therefore DEFERRED, not
//! rejected — see the subplan §5/§5-bis.
//!
//! ## Out of scope for W2
//!
//! The background fsync timer — which bounds the power-loss window for
//! `Buffered` entries on a quiet system or at shutdown by flushing the
//! page cache to disk on an interval — is NOT part of this primitive. It
//! belongs to the integration layer (W3). This coordinator only fsyncs
//! when a window carries a `Synced` waiter.
//!
//! PURELY ADDITIVE: not wired into `RepoWalManager` or the commit path
//! (that is W3/W4). Marked `#[allow(dead_code)]` while unwired.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use shamir_storage::error::{DbError, DbResult};
use tokio::sync::{Mutex, Notify};

use crate::wal_sink::WalSink;

/// Durability tier for a single WAL append.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WalDurability {
    /// Ack after `write()` to the OS page cache (level 2): survives a
    /// process crash, lost only on power loss before a later `sync`.
    Buffered,
    /// Ack after `fsync` (level 3): survives power loss.
    Synced,
}

struct Waiter {
    done: AtomicBool,
    ok: AtomicBool,
    notify: Notify,
}

impl Waiter {
    fn new() -> Self {
        Self {
            done: AtomicBool::new(false),
            ok: AtomicBool::new(false),
            notify: Notify::new(),
        }
    }
    fn complete(&self, ok: bool) {
        self.ok.store(ok, Ordering::Release);
        self.done.store(true, Ordering::Release);
        self.notify.notify_one();
    }
}

type Pending = (
    Vec<u8>,
    u64, /* commit_version */
    WalDurability,
    Arc<Waiter>,
);

/// Lock-free-leader group commit over a [`WalSink`]. See the module-level
/// correctness argument (L1/L2/L3 — the structure originally proved in the
/// now-removed `group_fsync.rs`, preserved at `f62fe18`).
#[allow(dead_code)]
pub struct WalGroupCommit {
    sink: Arc<WalSink>,
    // Sanctioned tokio::sync::Mutex (CLAUDE.md "Banned in hot paths"):
    // guards a tiny O(1) push / mem::take critical section, NEVER held
    // across `append_batch`/`sync` .await. Contention model: one push per
    // concurrent committer + one `mem::take` per drain window — sub-µs
    // under lock.
    //
    // SINGLE-WRITER BY CONSTRUCTION + MEASURED non-bottleneck. The rotating
    // leader (CAS on `flushing`) makes exactly one task drain at a time, so
    // this lock simulates a single writer out of many equal committers. The
    // WAL-append bench (`benches/wal_append.rs`, baseline `2e3bd51`) confirms
    // it is not the ceiling: mem-sink append SCALES 4.4× with concurrency
    // (20.5K→91.2K, N=1→64) — a lock-bound path would plateau/regress;
    // group-commit coalescing makes each added committer CHEAPER.
    //
    // A single-writer-task replacement (this `Mutex` → bounded MPSC, leader
    // CAS deleted) is designed in `docs/perf/capstone-subplan.md`. It was
    // PROTOTYPED and REVERTED (subplan §5/§5-bis): the permanent writer task
    // mandates a per-append cross-task `oneshot` round-trip that suspends the
    // executor on every append, whereas this leader drains the in-RAM `Mem`
    // sink synchronously within one poll. Measured cost: mem N=1 latency
    // ~+22% (the subplan §0 GO/NO-GO criterion) plus a broken commit-path
    // atomicity property (the non-yielding Mem append keeps a commit atomic
    // on a current-thread runtime; the mandatory yield let concurrent SSI
    // committers all validate before any published). Deferred, not rejected.
    pending: Mutex<Vec<Pending>>,
    flushing: AtomicBool,
    // Count of fsyncs this coordinator issued (for the batching test).
    fsync_count: AtomicU64,
    // RF1: true when a Buffered append completed but no fsync has run since.
    dirty_since_sync: AtomicBool,
}

#[allow(dead_code)]
impl WalGroupCommit {
    pub fn new(sink: Arc<WalSink>) -> Self {
        Self {
            sink,
            pending: Mutex::new(Vec::new()),
            flushing: AtomicBool::new(false),
            fsync_count: AtomicU64::new(0),
            dirty_since_sync: AtomicBool::new(false),
        }
    }

    /// Append one encoded payload at the given tier; returns once the
    /// entry has reached the requested durability level. `commit_version`
    /// is the MVCC version stamped on the entry — the leader folds the
    /// window's max into the sink's `max_committed` watermark (F6).
    pub async fn append(
        &self,
        payload: Vec<u8>,
        commit_version: u64,
        durability: WalDurability,
    ) -> DbResult<()> {
        let waiter = Arc::new(Waiter::new());
        {
            let mut p = self.pending.lock().await;
            p.push((payload, commit_version, durability, Arc::clone(&waiter)));
        }
        if self
            .flushing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.lead_until_drained().await;
        }
        // Park until the entry reaches its tier. enable()-before-check
        // closes the Notify subscribe-window race.
        loop {
            let notified = waiter.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if waiter.done.load(Ordering::Acquire) {
                break;
            }
            notified.await;
        }
        if waiter.ok.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(DbError::Storage("wal group commit failed".into()))
        }
    }

    /// Batched append — all payloads land in ONE leader window (one
    /// `append_batch` call to the sink), so the batch is atomic at the
    /// segment level: either every payload is written in a single `write()`
    /// syscall (and reaches the requested tier together) or, on a write/sync
    /// failure, the segment is quarantined (§1.3) and every caller gets the
    /// SAME error — no partial-write resurrection on recovery (audit §1.6).
    ///
    /// `entries` is `(payload, commit_version)` pairs; `durability` applies to
    /// the whole batch. The window's `max_version` (folded into the sink's
    /// watermark) is the max of the entries' `commit_version`s.
    ///
    /// Atomicity argument: all entries are pushed to `pending` under ONE lock
    /// acquisition. The leader that drains the queue observes the full batch
    /// in one `mem::take` and issues a single `sink.append_batch(payloads, …)`
    /// — one `write()`, so a partial write (ENOSPC mid-frame) quarantines the
    /// segment and rolls back ALL frames in the batch (the §1.3 rollback
    /// truncates to the pre-batch offset). No entry survives a partial write,
    /// so recovery never replays a subset of a "failed" batch.
    pub async fn append_many(
        &self,
        entries: Vec<(Vec<u8>, u64)>,
        durability: WalDurability,
    ) -> DbResult<()> {
        if entries.is_empty() {
            return Ok(());
        }
        // One shared waiter for the whole batch. The leader completes it
        // (possibly multiple times — idempotent) with the single batch
        // outcome; every caller parks on the same waiter and observes the
        // same Ok/Err.
        let waiter = Arc::new(Waiter::new());
        {
            let mut p = self.pending.lock().await;
            for (payload, commit_version) in entries {
                p.push((payload, commit_version, durability, Arc::clone(&waiter)));
            }
        }
        if self
            .flushing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.lead_until_drained().await;
        }
        loop {
            let notified = waiter.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if waiter.done.load(Ordering::Acquire) {
                break;
            }
            notified.await;
        }
        if waiter.ok.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(DbError::Storage("wal group commit batch failed".into()))
        }
    }

    // Caller holds leadership (flushing == true). Drain windows until the
    // queue is observed empty, then release leadership ATOMICALLY with the
    // empty observation (under the pending lock) so a concurrent pusher is
    // either seen by us or wins leadership itself — never stranded.
    async fn lead_until_drained(&self) {
        loop {
            let batch = {
                let mut p = self.pending.lock().await;
                if p.is_empty() {
                    self.flushing.store(false, Ordering::Release);
                    return;
                }
                std::mem::take(&mut *p)
            };
            // Split without cloning payloads.
            let mut payloads = Vec::with_capacity(batch.len());
            let mut metas = Vec::with_capacity(batch.len());
            let mut batch_max_version = 0u64;
            for (p, version, tier, w) in batch {
                payloads.push(p);
                batch_max_version = batch_max_version.max(version);
                metas.push((tier, w));
            }
            // One write() for the whole window (level 2). The window's max
            // commit_version is the watermark folded into the sink (F6).
            let write_ok = self
                .sink
                .append_batch(payloads, batch_max_version)
                .await
                .is_ok();
            // Wake Buffered waiters immediately — level 2 reached. (write()
            // success means bytes are in the OS page cache regardless of a
            // later fsync outcome.)
            let has_buffered = metas.iter().any(|(t, _)| *t == WalDurability::Buffered);
            if write_ok && has_buffered {
                self.dirty_since_sync.store(true, Ordering::Release);
            }
            for (tier, w) in &metas {
                if *tier == WalDurability::Buffered {
                    w.complete(write_ok);
                }
            }
            // One fsync for the window iff any Synced waiter is present.
            let needs_fsync = metas.iter().any(|(t, _)| *t == WalDurability::Synced);
            let sync_ok = if write_ok && needs_fsync {
                self.fsync_count.fetch_add(1, Ordering::Relaxed);
                let ok = self.sink.sync().await.is_ok();
                if ok {
                    self.dirty_since_sync.store(false, Ordering::Release);
                }
                ok
            } else {
                write_ok
            };
            for (tier, w) in &metas {
                if *tier == WalDurability::Synced {
                    w.complete(write_ok && sync_ok);
                }
            }
            // Circuit breaker: on write or sync error, release leadership
            // and return so the next append elects a fresh leader. Without
            // this, a dead segment causes infinite spin.
            if !write_ok || (needs_fsync && !sync_ok) {
                self.flushing.store(false, Ordering::Release);
                return;
            }
        }
    }

    /// Replay all `WalEntryV2` records persisted in the underlying sink.
    pub async fn replay(&self) -> DbResult<Vec<crate::wal_entry_v2::WalEntryV2>> {
        self.sink.replay().await
    }

    /// F6b: delegate WAL truncation to the sink — reclaim every record fully
    /// durable in history (`commit_version` in `(0, durable]`). Returns the
    /// count reclaimed. The caller (drainer) is responsible for flushing
    /// history before this (I2).
    pub async fn truncate_below(&self, durable: u64) -> DbResult<usize> {
        self.sink.truncate_below(durable).await
    }

    /// F6b: cheap probe gating the drainer's history-flush + truncate so it
    /// fires only on a segment/frame boundary, never per-commit (I2).
    pub fn has_truncatable(&self, durable: u64) -> bool {
        self.sink.has_truncatable(durable)
    }

    /// Force a durable `fsync` of the sink (level 2 → level 3). Used by the
    /// `synced` durability tier at batch granularity.
    pub async fn sync_now(&self) -> DbResult<()> {
        let res = self.sink.sync().await;
        if res.is_ok() {
            self.dirty_since_sync.store(false, Ordering::Release);
        }
        self.fsync_count.fetch_add(1, Ordering::Relaxed);
        res
    }

    /// Atomically read and clear the dirty flag. Returns `true` if a
    /// Buffered append happened since the last fsync.
    pub fn take_dirty(&self) -> bool {
        self.dirty_since_sync.swap(false, Ordering::AcqRel)
    }

    /// Spawn a background task that fsyncs the WAL every `interval` IFF a
    /// Buffered append happened since the last sync, bounding the power-loss
    /// window for level-2 (Buffered) commits. Weak-ref lifecycle: the task
    /// exits when the last `Arc<WalGroupCommit>` is dropped (no leak —
    /// mirrors MemBufferStore's flusher). No-op sinks make `sync_now()` cheap.
    ///
    /// **Audit §1.5:** `take_dirty()` clears the flag BEFORE attempting the
    /// fsync. If the fsync fails, restoring the flag (`store(true)`) ensures
    /// the next tick retries instead of leaving the dirty state lost with no
    /// further attempt until a new append arrives — on a quiescent system
    /// that is an unbounded data-at-risk window. The fsync error is logged
    /// loudly (it was previously swallowed by `let _ =`). Repeated fsync
    /// failures cause the sink to quarantine the segment (see §1.3 — the
    /// `WalSegment::sync` poison path forces the leader to rotate), so this
    /// loop converges: either the fsync eventually succeeds (clearing the
    /// flag) or every append starts failing fast on a poisoned segment.
    pub fn spawn_background_fsync(self: &Arc<Self>, interval: Duration) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                match weak.upgrade() {
                    Some(g) => {
                        if g.take_dirty() {
                            if let Err(e) = g.sync_now().await {
                                // Restore the dirty flag so the next tick
                                // retries the fsync instead of silently
                                // abandoning the unflushed Buffered entries.
                                g.dirty_since_sync.store(true, Ordering::Release);
                                log::error!(
                                    "WalGroupCommit background fsync failed: {e}; \
                                     dirty flag restored — will retry next tick"
                                );
                            }
                        }
                    }
                    None => break,
                }
            }
        });
    }

    #[cfg(test)]
    pub(crate) fn fsync_count(&self) -> u64 {
        self.fsync_count.load(Ordering::Relaxed)
    }

    /// Test-only: is the dirty flag currently set? Used by the §1.5
    /// regression test to assert the background fsync restores it after a
    /// failed sync (rather than silently losing it).
    #[cfg(test)]
    pub(crate) fn is_dirty(&self) -> bool {
        self.dirty_since_sync.load(Ordering::Acquire)
    }

    /// Test-only: set the dirty flag directly (simulates a Buffered append
    /// landing without going through the full append path).
    #[cfg(test)]
    pub(crate) fn set_dirty(&self) {
        self.dirty_since_sync.store(true, Ordering::Release);
    }
}
