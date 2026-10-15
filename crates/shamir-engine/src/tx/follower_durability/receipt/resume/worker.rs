//! Repository-owned bounded receipt worker (S5c-X PR6b-2, round 3 — review
//! findings 2/3/4). ONE lazily-spawned worker task per repository replaces
//! the per-call `tokio::spawn` in `apply_batch_receipt`: callers submit
//! bounded jobs (queue capacity 4, `try_send`) and await only a oneshot
//! reply, so CALLER cancellation can no longer drop the pipeline mid-flight
//! — the worker retains ownership and runs the job to completion or to a
//! durable pause point.
//!
//! PLACEMENT NOTE: this lives in `resume/` (not `receipt/`) because the
//! `receipt/` folder is at the 7-entry ratchet limit; `resume/` has room.
//! All repo-facing orchestration (`impl RepoInstance` blocks) lives here so
//! `repo_instance.rs` only carries the lazy field.
//!
//! Guard ownership across errors (finding 2): a pipeline that fails while it
//! still holds fresh chunk `VersionGuard`s moves them into
//! [`ReceiptWorker::parked_guards`] instead of dropping them — a dropped
//! guard marks its version `Aborted` and ADVANCES the watermarks, which
//! would let the repo serve over an unpublished version. Parked guards are
//! released ONLY by (a) a later successful publication on this repo (the
//! versions are then ≤ the watermark, so their eventual `Aborted` marks are
//! no-ops), or (b) worker shutdown under QUARANTINE: the gate is set
//! `requires_reopen` and the guards dropped (after reopen the `Aborted`
//! marks are acceptable — the repo is fail-stopped until then).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::Store;
use shamir_tx::VersionGuard;
use tokio::sync::{mpsc, oneshot, Mutex, Notify};

use super::super::apply::run_receipt_pipeline;
use crate::repo::RepoInstance;
use crate::tx::{ApplyOutcome, ApplyPolicy, ReplicatedEvent};

/// Bounded worker queue: the admission mutex inside the pipeline serializes
/// applies (one at a time), so the queue only absorbs concurrent submitters.
const RECEIPT_WORKER_QUEUE: usize = 4;

/// One unit of receipt-pipeline work, owned by the worker task. `repo` is an
/// owned clone: the job outlives any caller that drops its reply receiver.
pub(crate) struct Job {
    repo: RepoInstance,
    info: Arc<dyn Store>,
    events: Vec<ReplicatedEvent>,
    bookmark: u64,
    policy: ApplyPolicy,
    reply: oneshot::Sender<DbResult<ApplyOutcome>>,
    /// Repo-side worker handle, threaded into `run_receipt_pipeline` for
    /// guard park/retire — the loop itself holds no `Arc<ReceiptWorker>`.
    worker: Arc<ReceiptWorker>,
}

/// Loop-only state, held as `Arc<LoopState>` by BOTH the spawned loop and
/// [`ReceiptWorker`]: the loop holds NO `Arc<ReceiptWorker>`, so when the
/// repo's `OnceCell` (which owns the worker and thus the queue sender)
/// drops, the sender drops, `recv()` returns `None`, and the loop exits —
/// the worker task can no longer keep its own queue alive (reviewer P1 #4).
struct LoopState {
    /// Jobs currently running (0..1 in practice — the admission mutex
    /// serializes). Drives the deterministic `wait_idle` test seam.
    in_flight: AtomicUsize,
    /// Notified on every 1→0 `in_flight` transition.
    idle: Notify,
}

impl LoopState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            in_flight: AtomicUsize::new(0),
            idle: Notify::new(),
        })
    }
}

pub(crate) struct ReceiptWorker {
    /// `Some` while the worker is accepting submissions; `None` after
    /// shutdown (dropping the last sender closes the queue and ends the loop).
    sender: Mutex<Option<mpsc::Sender<Job>>>,
    closed: AtomicBool,
    handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Fresh chunk guards parked across pipeline errors — see the module
    /// doc. NEVER dropped while the repo keeps serving.
    parked_guards: Mutex<Vec<VersionGuard>>,
    /// Shared with the spawned loop — see [`LoopState`].
    state: Arc<LoopState>,
}

impl ReceiptWorker {
    fn new() -> (Arc<Self>, mpsc::Receiver<Job>) {
        let (sender, receiver) = mpsc::channel(RECEIPT_WORKER_QUEUE);
        (
            Arc::new(Self {
                sender: Mutex::new(Some(sender)),
                closed: AtomicBool::new(false),
                handle: Mutex::new(None),
                parked_guards: Mutex::new(Vec::new()),
                state: LoopState::new(),
            }),
            receiver,
        )
    }

    /// A worker for a repo that never initialized one (or was already shut
    /// down at install time): closed, sender-less, NO task spawned. Every
    /// later `submit` fails with the typed closed error.
    pub(crate) fn new_closed() -> Arc<Self> {
        Arc::new(Self {
            sender: Mutex::new(None),
            closed: AtomicBool::new(true),
            handle: Mutex::new(None),
            parked_guards: Mutex::new(Vec::new()),
            state: LoopState::new(),
        })
    }

    /// Submit one job and await its reply. `try_send` keeps the queue
    /// bounded: a full queue or a shut-down worker is a typed error, never a
    /// spawn. Only the REPLY is awaited — dropping this future at the await
    /// point drops the receiver, and the worker KEEPS RUNNING the job.
    ///
    /// The `Arc<Self>` receiver lets the job carry a worker handle into the
    /// spawned loop (`run_receipt_pipeline` parks/retires guards through it)
    /// WITHOUT the loop structurally owning one: the Arc lives only inside
    /// the dequeued job and drops with it (reviewer P1 #4).
    pub(crate) async fn submit(
        self: &Arc<Self>,
        repo: RepoInstance,
        info: Arc<dyn Store>,
        events: Vec<ReplicatedEvent>,
        bookmark: u64,
        policy: ApplyPolicy,
    ) -> DbResult<ApplyOutcome> {
        if self.closed.load(Ordering::Acquire) {
            return Err(DbError::Internal(format!(
                "receipt worker for repo {} shut down",
                repo.name()
            )));
        }
        let (tx, rx) = oneshot::channel();
        let job = Job {
            repo,
            info,
            events,
            bookmark,
            policy,
            reply: tx,
            worker: Arc::clone(self),
        };
        let rejected = {
            let sender = self.sender.lock().await;
            match sender.as_ref() {
                Some(sender) => sender.try_send(job).err(),
                None => Some(mpsc::error::TrySendError::Closed(job)),
            }
        };
        match rejected {
            Some(mpsc::error::TrySendError::Full(_)) => {
                return Err(DbError::Internal(
                    "receipt worker queue full (capacity 4); retry the pull".into(),
                ));
            }
            Some(mpsc::error::TrySendError::Closed(_)) => {
                return Err(DbError::Internal("receipt worker shut down".into()));
            }
            None => {}
        }
        // The worker owns the job: a dropped caller only drops `rx`.
        rx.await
            .map_err(|_| DbError::Internal("receipt worker dropped the reply".into()))?
    }

    /// Finding 2: take ownership of a failed chunk's still-fresh guards.
    /// Their Drop marks Aborted + advances the watermarks, so they must NOT
    /// drop while the repo keeps serving — they wait here, parked.
    pub(crate) async fn park_guards(&self, guards: Vec<VersionGuard>) {
        if guards.is_empty() {
            return;
        }
        log::warn!(
            "receipt worker: parking {} unpublished version guards across a \
             failed apply",
            guards.len()
        );
        self.parked_guards.lock().await.extend(guards);
    }

    /// Release path (a): a chunk just PUBLISHED (apply_publication ran
    /// `mark_durable`), so every parked version is ≤ the watermark and the
    /// parked guards' Aborted marks are no-ops. Drain after every successful
    /// publication.
    pub(crate) async fn retire_parked_guards(&self) {
        self.parked_guards.lock().await.clear();
    }

    #[cfg(test)]
    pub(crate) async fn parked_guard_count(&self) -> usize {
        self.parked_guards.lock().await.len()
    }

    /// Deterministic test wait until no job is in flight. Cancel-safe
    /// arm-then-check Notify pattern (mirrors `CommitTailBarrier::wait_zero`).
    #[cfg(test)]
    pub(crate) async fn wait_idle(&self) {
        loop {
            let notified = self.state.idle.notified();
            tokio::pin!(notified);
            // Arm before checking: the final decrement may race this waiter.
            notified.as_mut().enable();
            if self.state.in_flight.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }

    /// Close the queue and wait (bounded) for the in-flight job WITHOUT ever
    /// detaching it: the `JoinHandle` STAYS stored, so on timeout the task is
    /// still owned and a later shutdown attempt can await it again. Returns
    /// `Err` only on timeout (the caller decides the quarantine).
    async fn shutdown_core(&self, timeout: Duration) -> DbResult<()> {
        self.closed.store(true, Ordering::Release);
        // Dropping the sender closes the channel; the loop drains queued
        // jobs (`recv` returns the rest) before it exits.
        drop(self.sender.lock().await.take());
        let mut handle = self.handle.lock().await;
        match handle.as_mut() {
            // Await UNDER the handle mutex: on timeout the handle is left in
            // place — never dropped, never detached.
            Some(join) => {
                let joined = tokio::time::timeout(timeout, &mut *join).await;
                match joined {
                    // Timeout: the job is still running — the caller
                    // quarantines (requires_reopen) and reports the error.
                    Err(_elapsed) => Err(DbError::Internal(
                        "receipt worker shutdown timed out; job still running".into(),
                    )),
                    // Completed: surface a panicked job as a typed error
                    // (reviewer P1 #3) instead of swallowing the JoinError.
                    Ok(Err(join_error)) => Err(DbError::Internal(format!(
                        "receipt worker task failed: {join_error}"
                    ))),
                    Ok(Ok(())) => Ok(()),
                }
            }
            None => Ok(()),
        }
    }
}

/// The single lifecycle spawn of this repository's receipt worker: drain the
/// queue, one job at a time. The `commit_tail` guard makes
/// `quiesce_commits`/`shutdown_clean` wait for in-flight receipt work. A
/// panicked job ends the loop; later submitters observe the closed queue as
/// a typed `Internal` error (same visibility as the old per-call
/// `JoinHandle` join error, without an orphan spawn per call).
async fn worker_loop(state: Arc<LoopState>, mut receiver: mpsc::Receiver<Job>) {
    while let Some(job) = receiver.recv().await {
        state.in_flight.fetch_add(1, Ordering::AcqRel);
        let _tail = job.repo.commit_tail().enter();
        // The worker handle comes from the JOB (repo side) — the loop itself
        // holds no `Arc<ReceiptWorker>`, so dropping the repo's OnceCell
        // closes the queue and ends this loop.
        let result = run_receipt_pipeline(
            job.repo,
            job.info,
            job.events,
            job.bookmark,
            job.policy,
            &job.worker,
        )
        .await;
        let _ = job.reply.send(result);
        if state.in_flight.fetch_sub(1, Ordering::AcqRel) == 1 {
            state.idle.notify_waiters();
        }
    }
}

impl RepoInstance {
    /// Lazy accessor for the repository-owned receipt worker — same
    /// `OnceCell` pattern as `tx_gate()`: the worker task is spawned on the
    /// FIRST submission and lives until shutdown or repo drop.
    pub(crate) async fn receipt_worker(&self) -> DbResult<Arc<ReceiptWorker>> {
        let worker = self
            .receipt_worker
            .get_or_try_init(|| async {
                let (worker, receiver) = ReceiptWorker::new();
                let handle = tokio::spawn(worker_loop(Arc::clone(&worker.state), receiver));
                *worker.handle.lock().await = Some(handle);
                // E0283: multiple `From<_> for DbError` impls leave the
                // closure's `E` ambiguous — pin it explicitly.
                Ok::<Arc<ReceiptWorker>, DbError>(worker)
            })
            .await?;
        // A closed worker (installed by `shutdown_receipt_worker` before any
        // submission) is returned AS-IS: `submit` errors on the `closed`
        // flag, and would also error on the absent sender.
        Ok(Arc::clone(worker))
    }

    /// Idle count of parked guards, for tests and diagnostics. `None` when
    /// the worker was never initialized.
    #[cfg(test)]
    pub(crate) async fn receipt_worker_parked_guards(&self) -> Option<usize> {
        match self.receipt_worker.get() {
            Some(worker) => Some(worker.parked_guard_count().await),
            None => None,
        }
    }

    /// Deterministic test wait: no receipt job is in flight on this repo's
    /// worker.
    #[cfg(test)]
    pub(crate) async fn receipt_worker_wait_idle(&self) {
        self.receipt_worker().await.unwrap().wait_idle().await;
    }

    /// Shutdown the receipt worker: close the queue, wait (bounded) for the
    /// in-flight job — the JoinHandle is NEVER removed, so on timeout the
    /// task stays owned (a later shutdown attempt can await it again; it is
    /// never detached) — then QUARANTINE any parked guards: set
    /// `requires_reopen` and drop them (after reopen the Aborted marks are
    /// acceptable; the repo is fail-stopped until then). A repo that never
    /// initialized a worker gets a CLOSED worker installed instead, so any
    /// later `receipt_worker()`/submit observes the typed closed error.
    /// Idempotent.
    ///
    /// Shutdown contract (reviewer P1 #3): on success the loop has EXITED —
    /// every queued job was drained through the pipeline (each taking
    /// `commit_tail`, so a subsequent `quiesce_commits` observes them) and
    /// the in-flight job reached publication or a durable pause. On error
    /// the parked guards are STILL quarantined (that is the safe durable
    /// pause) and the timeout/join error is PROPAGATED to the caller —
    /// `shutdown_clean` must not report a clean shutdown with a live job.
    pub(crate) async fn shutdown_receipt_worker(&self, timeout: Duration) -> DbResult<()> {
        let worker = match self.receipt_worker.get() {
            Some(worker) => Arc::clone(worker),
            None => {
                // Never-initialized path: install the CLOSED worker. If the
                // `set` loses to a concurrent initializer (that initializer
                // is now the stored worker), fall through and shut THAT one
                // down instead of silently returning with a live worker.
                let closed = ReceiptWorker::new_closed();
                match self.receipt_worker.set(closed) {
                    Ok(()) => return Ok(()),
                    Err(_winner) => self.receipt_worker().await?,
                }
            }
        };
        let shutdown = worker.shutdown_core(timeout).await;
        if let Err(error) = &shutdown {
            log::error!(
                "receipt worker shutdown for repo {}: {error}; the task \
                 remains owned and parked guards are quarantined",
                self.name()
            );
        }
        // Quarantine runs on BOTH paths: it is the safe durable pause.
        let parked = worker.parked_guards.lock().await.len();
        if parked > 0 {
            log::warn!(
                "receipt worker shutdown: quarantining {parked} parked \
                 version guards (requires_reopen)"
            );
            if let Ok(gate) = self.tx_gate().await {
                gate.set_requires_reopen();
            }
            worker.parked_guards.lock().await.clear();
        }
        shutdown
    }
}
