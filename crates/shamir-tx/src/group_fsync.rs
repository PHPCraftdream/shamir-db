//! Lock-free group-commit fsync primitive (D1b scaffold, additive).
//!
//! [`GroupFsync`] coalesces many concurrent durable appends into a single
//! `set_many` + `flush()` window: one appender wins leadership (a single
//! `AtomicBool` CAS), drains the pending queue in batches and fsyncs once
//! per window; the rest park until their physical entry is durable. The
//! amortised cost is one fsync per *window*, not per append.
//!
//! ## Why per-entry waiters, not a generation counter
//!
//! The obvious design ties completion to a `watch<u64>` generation /
//! epoch counter: the leader bumps `flushed_gen` after each fsync, and an
//! appender waits for `flushed_gen >= my_gen`. That forces the appender to
//! *predict* which future flush will cover its entry — and the prediction
//! is racy. An entry pushed just before a `mem::take` is drained into the
//! *current* window, yet the appender may have already snapshotted a later
//! `cur_gen`; it then ends up **durable but waiting on a generation that
//! never arrives** (the leader released after the window the entry was
//! actually in). That is a genuine liveness gap, not a theoretical one.
//!
//! Per-entry waiters sidestep the prediction entirely: completion is tied
//! to the *physical* entry. The leader that drains a `(key, value, waiter)`
//! tuple is, by construction, the task that calls `waiter.complete(..)`
//! after the fsync that made that exact tuple durable. An appender is woken
//! IFF its entry was popped into a batch AND `flush()` returned. The cost is
//! one `Arc<Waiter>` per append — negligible beside an fsync.
//!
//! ## Correctness + liveness argument
//!
//! Leadership release happens under the **same** `pending` lock as `push`
//! (see [`GroupFsync::lead_until_drained`]): the leader observes the queue
//! empty and stores `flushing = false` while still holding the lock. A push
//! is therefore serialised against that observation — a late push is either
//! (a) seen by the current leader and drained in a further window, or
//! (b) ordered strictly after the `flushing = false` store, in which case
//! that pusher's own CAS succeeds and it becomes the next leader. No entry
//! can be enqueued yet never drained — no stranding.
//!
//! PURELY ADDITIVE: not wired into `RepoWalManager` or the commit path
//! (that is D1c/D1d). Marked `#[allow(dead_code)]` while unwired so clippy
//! stays green.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{RecordKey, Store};
use tokio::sync::{Mutex, Notify};

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

type Pending = (RecordKey, Bytes, Arc<Waiter>);

#[allow(dead_code)]
pub struct GroupFsync {
    store: Arc<dyn Store>,
    // Sanctioned tokio::sync::Mutex (CLAUDE.md): guards a tiny O(1)
    // push / mem::take critical section, NEVER held across the
    // flush().await below. Contention model: one push per concurrent
    // committer + one take per flush window — nanoseconds under lock.
    pending: Mutex<Vec<Pending>>,
    flushing: AtomicBool,
}

#[allow(dead_code)]
impl GroupFsync {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self {
            store,
            pending: Mutex::new(Vec::new()),
            flushing: AtomicBool::new(false),
        }
    }

    pub async fn append_and_await(&self, key: RecordKey, value: Bytes) -> DbResult<()> {
        let waiter = Arc::new(Waiter::new());
        {
            let mut p = self.pending.lock().await;
            p.push((key, value, Arc::clone(&waiter)));
        }
        if self
            .flushing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.lead_until_drained().await;
        }
        // Park until durable. enable()-before-check closes the Notify
        // subscribe-window race.
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
            Err(DbError::Storage("group fsync flush failed".into()))
        }
    }

    // Caller holds leadership (flushing == true). Flush batches until the
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
            let mut items = Vec::with_capacity(batch.len());
            let mut waiters = Vec::with_capacity(batch.len());
            for (k, v, w) in batch {
                items.push((k, v));
                waiters.push(w);
            }
            let set_res = self.store.set_many(items).await;
            let result = match set_res {
                Ok(_) => self.store.flush().await,
                Err(e) => Err(e),
            };
            let ok = result.is_ok();
            for w in waiters {
                w.complete(ok);
            }
            if let Err(e) = result {
                log::error!("group fsync flush failed: {e}");
            }
        }
    }
}
