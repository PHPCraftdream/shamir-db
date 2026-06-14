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
//! This reuses the verified leader/follower structure from
//! `shamir-tx`'s `group_fsync.rs` (D1b) verbatim:
//!   - leadership is released under the SAME `pending` lock as `push`
//!     (in [`WalGroupCommit::lead_until_drained`]) — a late push is either
//!     seen by the current leader or wins leadership itself, so no entry
//!     is ever stranded;
//!   - the `enable()`-before-check `Notify` park loop closes the
//!     subscribe-window race (no lost wakeup);
//!   - completion is tied to the PHYSICAL entry: the leader that drains a
//!     `(payload, tier, waiter)` tuple is the task that calls
//!     `waiter.complete(..)` once that exact tuple has reached its tier.
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

type Pending = (Vec<u8>, WalDurability, Arc<Waiter>);

/// Lock-free-leader group commit over a [`WalSink`]. See module-level
/// correctness argument (and `group_fsync.rs`, D1b — identical structure).
#[allow(dead_code)]
pub struct WalGroupCommit {
    sink: Arc<WalSink>,
    // Sanctioned tokio::sync::Mutex (CLAUDE.md): O(1) push / mem::take only,
    // NEVER held across append_batch/sync .await. One push per concurrent
    // committer + one take per window.
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
    /// entry has reached the requested durability level.
    pub async fn append(&self, payload: Vec<u8>, durability: WalDurability) -> DbResult<()> {
        let waiter = Arc::new(Waiter::new());
        {
            let mut p = self.pending.lock().await;
            p.push((payload, durability, Arc::clone(&waiter)));
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
            for (p, tier, w) in batch {
                payloads.push(p);
                metas.push((tier, w));
            }
            // One write() for the whole window (level 2).
            let write_ok = self.sink.append_batch(payloads).await.is_ok();
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
    pub fn spawn_background_fsync(self: &Arc<Self>, interval: Duration) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                match weak.upgrade() {
                    Some(g) => {
                        if g.take_dirty() {
                            let _ = g.sync_now().await;
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
}
