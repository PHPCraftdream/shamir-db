//! Bounded command admission for the `CachedStore` FIFO owner channel
//! (RV-STORAGE-E02C8A80).
//!
//! The owner channel used to be an UNBOUNDED `mpsc`, so under
//! `WriteMode::Async` every value-bearing command was admitted without
//! limit behind a stalled owner and the queued historical payloads kept
//! memory alive without bound. Admission is now bounded TWICE:
//!
//! * a COUNT bound, [`COMMAND_CHANNEL_CAPACITY`] commands, and
//! * a retained-BYTE bound, [`QUEUED_PAYLOAD_BUDGET_BYTES`].
//!
//! The EXACT byte bound: the charged bytes of QUEUED commands (≤
//! [`QUEUED_PAYLOAD_BUDGET_BYTES`]), plus at most ONE oversized command
//! whose reservation is taken only from an exactly-zero counter.
//! Commands WAITING for budget are held by their callers and are NOT
//! charged — that is caller memory, not queue memory. The reservation
//! guard travels INSIDE each queued command and refunds on ANY drop
//! (owner dequeue, owner abort/panic, receiver drop), so the counter can
//! never be stranded. KEY bytes count toward charges; retries never
//! re-charge; cancellation refunds.
//!
//! A count bound ([`COMMAND_CHANNEL_CAPACITY`] commands) applies on top.
//! When the queue is full the submitting task cooperatively `yield_now`s
//! and retries — saturation pends the CALLER task, never the executor
//! thread (the same pattern the Fjall write worker's `submit` uses).

use super::owner_health::storage_err;
use super::Command;
#[cfg(test)]
use super::{CachedStore, RecordKey};
use crate::error::{DbError, DbResult};
use bytes::Bytes;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
#[cfg(test)]
use tokio::sync::oneshot;
use tokio::task::yield_now;

/// Hard bound on commands queued for the FIFO owner task. Value-bearing
/// `Set` commands dominate memory retention; 1024 queued commands is far
/// above any legitimate caller concurrency while capping worst-case
/// retention behind a stalled owner.
pub(crate) const COMMAND_CHANNEL_CAPACITY: usize = 1024;

/// Hard bound on the total payload bytes retained by commands queued for
/// the FIFO owner task (RV-STORAGE-E02C8A80 round 2). The count bound
/// above is NOT a memory bound (1024 × arbitrary values is unbounded
/// memory), so value-bearing payloads are additionally charged against
/// this byte budget; see the module docs for the exact bound.
pub(crate) const QUEUED_PAYLOAD_BUDGET_BYTES: usize = 64 * 1024 * 1024;

/// Retained payload bytes this command would add to the owner queue.
/// KEY bytes count toward charges (they are retained alongside the
/// values until the owner dequeues the command).
pub(crate) fn queued_payload_bytes(cmd: &Command) -> usize {
    match cmd {
        Command::Insert { value, .. } => value.len(),
        Command::Set { key, value, .. } => key.len() + value.len(),
        Command::Transact { ops, .. } => ops
            .iter()
            .map(|op| match op {
                crate::types::KvOp::Set(k, v) => k.len() + v.len(),
                crate::types::KvOp::Remove(k) => k.len(),
            })
            .sum(),
        Command::Fill { key, value, .. } => key.len() + value.as_ref().map_or(0, Bytes::len),
        Command::Remove { key, .. } => key.len(),
        Command::Reload { .. } | Command::Flush { .. } | Command::FlushDrain { .. } => 0,
    }
}

/// A charge-once reservation against the retained-byte budget
/// (RV-STORAGE-E02C8A80 round 4).
///
/// The reservation OWNS its share of the budget counter (an `Arc` clone)
/// and refunds `bytes` unconditionally on `Drop` — on owner dequeue, on
/// owner abort/panic, on cancellation of the `admit` future, on a
/// refused command, and on receiver drop. Because the reservation
/// travels inside the queued [`QueuedCommand`], the budget can never be
/// stranded by any death of the owner. A full-channel retry does NOT
/// re-charge: the same reservation persists across `try_send` attempts.
pub(crate) struct Reservation {
    counter: Arc<AtomicUsize>,
    bytes: usize,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.bytes > 0 {
            self.counter.fetch_sub(self.bytes, Ordering::Relaxed);
        }
    }
}

/// A command queued for the owner TOGETHER WITH its byte-budget
/// reservation: the guard refunds automatically on ANY drop — owner
/// dequeue, owner abort/panic, receiver drop — so the shared counter can
/// never be stranded by queued commands nobody dequeues.
pub(crate) struct QueuedCommand {
    cmd: Command,
    _reservation: Reservation,
}

impl QueuedCommand {
    /// Split into the command (to execute) and the reservation (dropped
    /// immediately by the owner at dequeue, releasing the charge).
    pub(crate) fn into_parts(self) -> (Command, Reservation) {
        (self.cmd, self._reservation)
    }
}

/// ONE CAS-based reservation attempt against the byte budget. Returns
/// `None` when not reservable (the caller yields and retries); no charge
/// is left behind on refusal.
///
/// * `size == 0` → trivially reservable with a zero-byte reservation.
/// * OVERSIZED (`size > QUEUED_PAYLOAD_BUDGET_BYTES`) → succeeds only via
///   `compare_exchange(0, size)`: the counter must be EXACTLY zero. This
///   IS the oversized-exclusivity mechanism — only one oversized
///   reservation can exist at a time, and the counter reads 0 again only
///   after the owner dequeues the command. There is NO `tx.capacity()`
///   check anywhere (the old capacity-then-send check was a racy
///   check-then-act two concurrent oversized producers could both pass).
/// * normal → CAS loop reserving `cur + size` while `cur + size` stays
///   within the budget; `None` if the budget is exhausted.
fn try_reserve(counter: &Arc<AtomicUsize>, size: usize, oversized: bool) -> Option<Reservation> {
    let make = |bytes: usize| Reservation {
        counter: Arc::clone(counter),
        bytes,
    };
    if size == 0 {
        return Some(make(0));
    }
    if oversized {
        counter
            .compare_exchange(0, size, Ordering::AcqRel, Ordering::Acquire)
            .ok()?;
        return Some(make(size));
    }
    let mut cur = counter.load(Ordering::Relaxed);
    loop {
        if cur + size > QUEUED_PAYLOAD_BUDGET_BYTES {
            return None;
        }
        match counter.compare_exchange_weak(cur, cur + size, Ordering::AcqRel, Ordering::Relaxed) {
            Ok(_) => return Some(make(size)),
            Err(now) => cur = now,
        }
    }
}

/// Test oracle for [`admit`]: how many full-channel retries occurred.
#[cfg(test)]
static FULL_CHANNEL_RETRIES: AtomicUsize = AtomicUsize::new(0);

/// Test oracle for [`admit`]: how often a reservation failed because the
/// byte budget was exhausted (the caller is stuck WAITING for budget).
#[cfg(test)]
static BUDGET_WAITS: AtomicUsize = AtomicUsize::new(0);

/// Zero the full-channel retry oracle (test setup only).
#[cfg(test)]
pub(crate) fn reset_retry_count_for_tests() {
    FULL_CHANNEL_RETRIES.store(0, Ordering::Relaxed);
}

/// Full-channel retries observed since the last reset (test oracle).
#[cfg(test)]
pub(crate) fn retry_count_for_tests() -> usize {
    FULL_CHANNEL_RETRIES.load(Ordering::Relaxed)
}

/// Budget waits observed since process start (test oracle).
#[cfg(test)]
pub(crate) fn budget_waits_for_tests() -> usize {
    BUDGET_WAITS.load(Ordering::Relaxed)
}

/// Admit `cmd` into the doubly-bounded owner channel: reserve ONCE (the
/// reservation persists across retries, so a full channel never
/// re-charges), then retry `try_send` cooperatively (`yield_now`) while
/// the queue refuses admission, so a saturated owner pends this TASK
/// instead of growing the queue. The reservation travels INSIDE the
/// queued [`QueuedCommand`] and refunds on ANY drop — dequeue, owner
/// abort/panic, cancellation, refusal, receiver drop — so the counter
/// cannot be stranded. Every iteration re-checks the sticky owner-health
/// watch and channel closure, so a caller stuck waiting for budget is
/// unblocked by owner death. Channel-closed maps onto the same
/// diagnostic the previous unbounded `send` produced.
pub(crate) async fn admit(
    tx: &mpsc::Sender<QueuedCommand>,
    queued_bytes: &Arc<AtomicUsize>,
    health: &tokio::sync::watch::Receiver<Option<String>>,
    cmd: Command,
) -> DbResult<()> {
    let size = queued_payload_bytes(&cmd);
    let oversized = size > QUEUED_PAYLOAD_BUDGET_BYTES;
    let mut cmd = cmd;
    let mut reservation: Option<Reservation> = None;
    loop {
        // Owner-health FIRST every iteration: a caller stuck waiting for
        // budget must observe a terminal owner failure.
        if let Some(msg) = health.borrow().clone() {
            return Err(storage_err(msg));
        }
        if tx.is_closed() {
            return Err(DbError::Storage(
                "cache owner command channel closed".to_string(),
            ));
        }
        if reservation.is_none() {
            match try_reserve(queued_bytes, size, oversized) {
                Some(res) => reservation = Some(res),
                None => {
                    #[cfg(test)]
                    BUDGET_WAITS.fetch_add(1, Ordering::Relaxed);
                    yield_now().await;
                    continue;
                }
            }
        }
        let queued = QueuedCommand {
            cmd,
            _reservation: reservation.take().expect("reservation held"),
        };
        match tx.try_send(queued) {
            Ok(()) => return Ok(()), // the charge now rides inside the queue
            Err(mpsc::error::TrySendError::Full(returned)) => {
                // Recover command AND reservation — the retry must NOT
                // re-charge.
                let QueuedCommand {
                    cmd: c,
                    _reservation: r,
                } = returned;
                cmd = c;
                reservation = Some(r);
                #[cfg(test)]
                FULL_CHANNEL_RETRIES.fetch_add(1, Ordering::Relaxed);
                yield_now().await;
            }
            Err(mpsc::error::TrySendError::Closed(returned)) => {
                drop(returned); // refunds the charge
                return Err(DbError::Storage(
                    "cache owner command channel closed".to_string(),
                ));
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn try_admit_for_tests(
    tx: &mpsc::Sender<QueuedCommand>,
    queued_bytes: &Arc<AtomicUsize>,
    cmd: Command,
) -> bool {
    let size = queued_payload_bytes(&cmd);
    let oversized = size > QUEUED_PAYLOAD_BUDGET_BYTES;
    let Some(reservation) = try_reserve(queued_bytes, size, oversized) else {
        return false; // not reservable: no charge left behind
    };
    let queued = QueuedCommand {
        cmd,
        _reservation: reservation,
    };
    match tx.try_send(queued) {
        Ok(()) => true, // the charge rides inside the queue
        Err(mpsc::error::TrySendError::Full(_)) | Err(mpsc::error::TrySendError::Closed(_)) => {
            false
        } // drop refunds
    }
}

#[cfg(test)]
impl CachedStore {
    /// RV-STORAGE-E02C8A80 oracle helper: enqueue filler commands until
    /// the channel refuses admission (or a 4x sanity cap is reached) and
    /// return how many were admitted. Goes through the REAL admission
    /// path ([`try_admit_for_tests`]); `Flush` carries no payload, so the
    /// count bound is the binding constraint and exactly
    /// [`COMMAND_CHANNEL_CAPACITY`] fillers fit. Against the old UNBOUNDED
    /// channel the cap is always reached.
    pub(crate) fn try_admit_set_for_tests(&self, key: RecordKey, value: Bytes) -> bool {
        let (reply, _) = oneshot::channel();
        try_admit_for_tests(
            &self.tx,
            &self.queued_bytes,
            Command::Set { key, value, reply },
        )
    }

    #[cfg(test)]
    pub(crate) fn fill_command_channel_for_tests(&self) -> usize {
        let mut admitted = 0;
        while admitted < 4 * COMMAND_CHANNEL_CAPACITY {
            let cmd = Command::Flush {
                reply: oneshot::channel().0,
            };
            if !try_admit_for_tests(&self.tx, &self.queued_bytes, cmd) {
                break;
            }
            admitted += 1;
        }
        admitted
    }
}
