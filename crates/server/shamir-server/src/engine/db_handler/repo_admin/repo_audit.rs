use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use shamir_collections::THasher;
use shamir_connect::common::time::UnixNanos;
use shamir_connect::common::types::TransportKind;
use shamir_connect::server::audit_chain::AuditChainWriter;
use shamir_connect::server::session::Session;
use shamir_db::engine::repo::DurabilityPolicy;
use shamir_db::RepoAdminObserver;
use shamir_query_types::admin::activity::{CancelOutcome, RequestExecId};
use shamir_query_types::durability::{ReplicationServe, RetentionOverflow, WalCommitTier};
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

fn transport_name(kind: TransportKind) -> &'static str {
    match kind {
        TransportKind::Tcp => "tcp",
        TransportKind::WebSocket => "ws",
        TransportKind::Unix => "unix",
    }
}

struct RepoAuditObserver {
    audit: Arc<AuditChainWriter>,
    username: String,
    transport: &'static str,
    session_prefix: [u8; 8],
}

impl RepoAdminObserver for RepoAuditObserver {
    fn repository_durability_changed(
        &self,
        db_name: &str,
        repo: &str,
        effective: &DurabilityPolicy,
    ) {
        let mut policy = new_map();
        policy.insert(
            "wal_commit".into(),
            QueryValue::Str(
                match effective.effective_wal_commit() {
                    WalCommitTier::Buffered => "buffered",
                    WalCommitTier::Synced => "synced",
                }
                .into(),
            ),
        );
        policy.insert(
            "wal_retention_max_bytes".into(),
            QueryValue::from(effective.wal_retention_max_bytes()),
        );
        policy.insert(
            "on_wal_retention_exceeded".into(),
            QueryValue::Str(
                match effective.on_wal_retention_exceeded() {
                    RetentionOverflow::RefuseWrites => "refuse_writes",
                    RetentionOverflow::DeclareGap => "declare_gap",
                }
                .into(),
            ),
        );
        policy.insert(
            "replication_serve".into(),
            QueryValue::Str(
                match effective.replication_serve() {
                    ReplicationServe::Durable => "durable",
                    ReplicationServe::CompletedReseed => "completed_reseed",
                }
                .into(),
            ),
        );
        let mut details = new_map();
        details.insert("db".into(), QueryValue::Str(db_name.to_owned()));
        details.insert("repo".into(), QueryValue::Str(repo.to_owned()));
        details.insert("durability".into(), QueryValue::Map(policy));
        if let Ok(bytes) = rmp_serde::to_vec_named(&QueryValue::Map(details)) {
            // RV-CONNECT-387EAA05: never fabricate a timestamp; on clock
            // failure log an error and skip this append (fail closed).
            let now_ns = match UnixNanos::try_now() {
                Ok(now) => now.as_u64(),
                Err(error) => {
                    tracing::error!(%error, "audit append skipped: clock unavailable");
                    return;
                }
            };
            if let Err(error) = self.audit.append(
                "alter_repo",
                self.transport,
                self.username.clone(),
                "",
                self.session_prefix,
                "ok",
                bytes,
                now_ns,
            ) {
                tracing::error!(%error, "audit append rejected repository event");
            }
        }
    }
}

#[derive(serde::Serialize)]
struct CancelAuditDetails<'a> {
    target: u64,
    outcome: &'a CancelOutcome,
}

// The low bits count queued and running jobs; the high bits close admission
// and latch the first rejected submission of each saturation episode.
const CANCEL_AUDIT_CAPACITY: usize = 256;
/// Share of the queue one principal may hold (queued + running jobs). An
/// eighth of the global cap, so at least eight principals keep their whole
/// share while the global cap binds and no single principal can starve the
/// rest. Charged per principal NAME — one counter per name, never shared
/// between two names — so one principal reaching its share can never deny
/// another while global slots are free.
pub(crate) const CANCEL_AUDIT_PRINCIPAL_SHARE: usize = 32;
const CLOSED: usize = 1 << (usize::BITS - 1);
const SATURATED: usize = CLOSED >> 1;
const COUNT_MASK: usize = !(CLOSED | SATURATED);
const DRAIN_BUDGET: Duration = Duration::from_secs(3);
const ADMISSION_RETRY_LIMIT: usize = 4;

#[derive(Default)]
pub(crate) struct CancelRefusalAudit {
    state: AtomicUsize,
    dropped: AtomicUsize,
    idle: Notify,
    /// Queued + running jobs per principal NAME. A name's entry is inserted
    /// with its first refusal and removed when its count returns to zero. The
    /// global slot is reserved inside the SAME bucket-locked decision that
    /// increments the count, and only when the share still has room (see
    /// [`CancelRefusalAudit::reserve_share_and_global`]), so every non-zero
    /// entry implies a held slot: at most `CANCEL_AUDIT_CAPACITY` entries with a
    /// non-zero count, plus zero entries whose removal raced a reservation
    /// (retired by that name's next release).
    ///
    /// Contention: one `update_sync`/`insert_sync` per refusal and one per
    /// release on the scc bucket of that name — the map's own bucket lock for
    /// the increment/decrement only, never across an `.await`, never inside the
    /// audit append. No `std` mutex is added to the reply path.
    principals: scc::HashMap<String, usize, THasher>,
    /// Abort handles of accepted-but-unfinished audit jobs. Bounded by
    /// `CANCEL_AUDIT_CAPACITY` live slots (finished entries are pruned on
    /// the next refusal), taken once per accepted refusal for one bounded
    /// push plus a prune of finished handles, and once at teardown; it is
    /// never held across an `.await`. It IS on the reply path: the client
    /// reply is built after the synchronous enqueue, whose cost is one
    /// principal-counter update, one `spawn_blocking` and this short
    /// push+prune. None of that waits for the audit writer, its persistence
    /// gate or a flush.
    unfinished: std::sync::Mutex<Vec<tokio::task::AbortHandle>>,
    /// Test seam (ordered-shutdown close/discard race): called after the share
    /// slot is reserved and before the job is spawned, so a test can close
    /// admission and sweep the queue inside that exact window.
    #[cfg(test)]
    pub(crate) pause_before_spawn: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// Outcome of the single bucket-locked admission decision. Refusal accounting
/// and logging are performed only after the bucket callback returns.
enum ShareAdmission {
    Reserved,
    ShareFull,
    GlobalRefused(GlobalRefusal),
}

#[derive(Clone, Copy)]
enum GlobalRefusal {
    Closed,
    Saturated,
    Full,
}

impl CancelRefusalAudit {
    /// Reserve one slot of `principal`'s share AND one global slot as ONE
    /// bounded, non-waiting decision, or refuse.
    ///
    /// The share test, the global reservation and the increment run under the
    /// SAME bucket lock (see [`Self::reserve_share_and_global`]), so the global
    /// slot is taken only when the share still has room: a doomed request from
    /// a principal already at its share never consumes — and never refuses
    /// another principal — a global slot. `reserve_global_slot` is a lock-free
    /// CAS loop, so it takes no second lock and cannot block inside the closure.
    ///
    /// A principal entry therefore always implies a reserved slot, so the map
    /// can never hold more live entries than `CANCEL_AUDIT_CAPACITY` (the bound
    /// its doc states).
    fn try_reserve(self: &Arc<Self>, principal: &str) -> Option<CancelAuditSlot> {
        // READ-ONLY pre-check: an already-capped principal is refused without
        // reaching the map update at all. It no longer decides anything -- the
        // same share test runs under the bucket lock below -- it only spares
        // the saturated case one state CAS and one entry lookup.
        if self.share_is_full(principal) {
            self.refuse_share_full();
            return None;
        }
        #[cfg(test)]
        fire_share_decision_probe();
        if !self.reserve_share_and_global(principal) {
            return None;
        }
        Some(CancelAuditSlot {
            queue: Arc::clone(self),
            principal: principal.to_owned(),
            appended: false,
        })
    }

    /// Read-only share test: creates no entry and takes no slot.
    fn share_is_full(&self, principal: &str) -> bool {
        self.principals
            .read_sync(principal, |_, held| *held >= CANCEL_AUDIT_PRINCIPAL_SHARE)
            .unwrap_or(false)
    }

    /// Take one slot of `principal`'s share AND one global slot in ONE
    /// bucket-locked decision, or refuse without waiting.
    ///
    /// The share test, the global CAS and the increment run inside the same
    /// `update_sync`, so no global slot is ever held by a request that the
    /// share check then refuses: the slot another principal needs is never
    /// consumed by a doomed reservation, not even for an instant. Exact per
    /// NAME: two principals never share a counter and the count cannot drift
    /// from the live slots.
    fn reserve_share_and_global(&self, principal: &str) -> bool {
        for _ in 0..ADMISSION_RETRY_LIMIT {
            match self.principals.update_sync(principal, |_, held| {
                if *held >= CANCEL_AUDIT_PRINCIPAL_SHARE {
                    return ShareAdmission::ShareFull;
                }
                match self.reserve_global_slot() {
                    Ok(()) => {
                        *held += 1;
                        ShareAdmission::Reserved
                    }
                    Err(reason) => ShareAdmission::GlobalRefused(reason),
                }
            }) {
                Some(ShareAdmission::Reserved) => return true,
                Some(ShareAdmission::ShareFull) => {
                    self.refuse_share_full();
                    return false;
                }
                Some(ShareAdmission::GlobalRefused(reason)) => {
                    // First-touch failures leave a zero-count placeholder. Retire
                    // it only if no concurrent admission has made it live.
                    self.principals.remove_if_sync(principal, |held| *held == 0);
                    self.refuse_global(reason);
                    return false;
                }
                None => {
                    // Publish an empty entry first. The subsequent bucket-locked
                    // update is the sole admission decision for both paths.
                    let _ = self.principals.insert_sync(principal.to_owned(), 0);
                }
            }
        }
        self.principals.remove_if_sync(principal, |held| *held == 0);
        self.refuse_retry_limit();
        false
    }

    fn refuse_retry_limit(&self) {
        let dropped = self.record_drop();
        tracing::warn!(
            retries = ADMISSION_RETRY_LIMIT,
            dropped,
            "cancel refusal audit admission retry limit reached; dropping event"
        );
    }

    fn refuse_global(&self, reason: GlobalRefusal) {
        match reason {
            GlobalRefusal::Closed => {
                let dropped = self.record_drop();
                tracing::debug!(dropped, "cancel refusal audit dropped: admission closed");
            }
            GlobalRefusal::Full => {
                let dropped = self.record_drop();
                tracing::warn!(
                    capacity = CANCEL_AUDIT_CAPACITY,
                    dropped,
                    "cancel refusal audit queue full; dropping events"
                );
            }
            GlobalRefusal::Saturated => {
                self.record_drop();
            }
        }
    }

    /// Record one refused submission: exactly one drop and one warn line, the
    /// same accounting for the read-only pre-check and for the decision.
    fn refuse_share_full(&self) {
        let dropped = self.record_drop();
        tracing::warn!(
            share = CANCEL_AUDIT_PRINCIPAL_SHARE,
            dropped,
            "cancel refusal audit principal share full; dropping event"
        );
    }

    /// Take one GLOBAL slot, or refuse: admission closed, capacity reached, or
    /// an earlier rejection in the same saturation episode already latched it.
    /// The count grows only here.
    fn reserve_global_slot(&self) -> Result<(), GlobalRefusal> {
        let mut state = self.state.load(Ordering::Acquire);
        loop {
            if state & CLOSED != 0 {
                return Err(GlobalRefusal::Closed);
            }
            let full = state & COUNT_MASK == CANCEL_AUDIT_CAPACITY;
            if full && state & SATURATED != 0 {
                return Err(GlobalRefusal::Saturated);
            }
            let next = if full { state | SATURATED } else { state + 1 };
            match self
                .state
                .compare_exchange_weak(state, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) if full => return Err(GlobalRefusal::Full),
                Ok(_) => return Ok(()),
                Err(current) => state = current,
            }
        }
    }

    /// Give one GLOBAL slot back, clearing the saturation latch in the same
    /// transition. Every path that reserved a slot but produced no
    /// [`CancelAuditSlot`] calls this; the slot's own `Drop` does too.
    fn release_global_slot(&self) {
        let mut state = self.state.load(Ordering::Acquire);
        loop {
            let next = (state - 1) & !SATURATED;
            match self
                .state
                .compare_exchange_weak(state, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => {
                    if next == CLOSED {
                        self.idle.notify_waiters();
                    }
                    return;
                }
                Err(current) => state = current,
            }
        }
    }

    /// Release one principal slot and retire the entry when it reaches zero.
    ///
    /// The decrement and the removal both run inside the name's bucket lock, so
    /// a reservation arriving in between is never lost: it either increments
    /// before the removal (the removal then sees a non-zero count and keeps the
    /// entry) or inserts a fresh entry afterwards.
    fn release_share(&self, principal: &str) {
        self.principals.update_sync(principal, |_, held| {
            *held = held.saturating_sub(1);
        });
        self.principals.remove_if_sync(principal, |held| *held == 0);
    }

    fn record_drop(&self) -> usize {
        self.dropped.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
    }

    /// Drops recorded so far — the test observable for the refusal accounting
    /// (a refused reservation, a closed queue and a discarded job all count).
    #[cfg(test)]
    pub(crate) fn dropped_count(&self) -> usize {
        self.dropped.load(Ordering::Relaxed)
    }

    pub(crate) fn close(&self) {
        // ONE decision with `spawn_tracked`: both take this mutex, so `close()`
        // cannot return while a spawn decision is in flight. After it returns
        // no job can still be spawned, and the handle of every already spawned
        // job sits in the list `discard_queued` drains. Setting the bit without
        // the mutex left exactly that window open.
        let _decision = self.lock_close_decision();
        self.state.fetch_or(CLOSED, Ordering::AcqRel);
    }

    /// Acquire the spawn-decision mutex; the test rendezvous observes an actual
    /// failed try-lock before this call blocks on that same mutex.
    fn lock_close_decision(
        &self,
    ) -> std::sync::LockResult<std::sync::MutexGuard<'_, Vec<tokio::task::AbortHandle>>> {
        #[cfg(test)]
        if matches!(
            self.unfinished.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ) {
            fire_close_decision_probe();
        }
        self.unfinished.lock()
    }

    /// Test-only hook fired when `close()` observes the spawn-decision mutex
    /// already held, before it waits to acquire it.
    #[cfg(test)]
    pub(crate) fn arm_close_decision_probe(
        probe: impl FnOnce() + 'static,
    ) -> CloseDecisionProbeGuard {
        CLOSE_DECISION_PROBE.with(|slot| *slot.borrow_mut() = Some(Box::new(probe)));
        CloseDecisionProbeGuard
    }

    /// Abort audit jobs that have not started yet.
    ///
    /// A job that never ran drops its [`CancelAuditSlot`], so it is counted
    /// exactly like a refused submission and its `Arc<AuditChainWriter>`
    /// reference — which would otherwise keep the writer, the appender and
    /// its fjall store alive past the release timeout — is released now.
    /// A job already blocked inside the writer's persistence gate keeps
    /// running: a timeout stops waiting, it does not cancel I/O.
    pub(crate) fn discard_queued(&self) {
        let aborted = self
            .unfinished
            .lock()
            .map(|mut handles| {
                let count = handles.len();
                for handle in handles.drain(..) {
                    handle.abort();
                }
                count
            })
            .unwrap_or(0);
        if aborted != 0 {
            tracing::warn!(aborted, "cancel refusal audit discarded queued jobs");
        }
    }

    /// Test-only hook fired BETWEEN the admission check and the spawn, on the
    /// caller's thread. A close-race test parks there to prove that `close()`
    /// cannot return while a spawn decision is in flight.
    #[cfg(test)]
    pub(crate) fn arm_spawn_window_probe(probe: impl FnOnce() + 'static) -> SpawnWindowProbeGuard {
        SPAWN_WINDOW_PROBE.with(|slot| *slot.borrow_mut() = Some(Box::new(probe)));
        SpawnWindowProbeGuard
    }

    /// Test-only hook fired BETWEEN the read-only share pre-check and the
    /// bucket-locked admission decision, on the caller's thread and holding no
    /// lock. A fairness test parks a doomed reservation there to prove it holds
    /// no global slot while another principal is admitted.
    #[cfg(test)]
    pub(crate) fn arm_share_decision_probe(
        probe: impl FnOnce() + 'static,
    ) -> ShareDecisionProbeGuard {
        SHARE_DECISION_PROBE.with(|slot| *slot.borrow_mut() = Some(Box::new(probe)));
        ShareDecisionProbeGuard
    }

    /// Spawn one accepted job and register its abort handle as ONE decision
    /// with [`Self::close`].
    ///
    /// The closed-bit test, the `spawn_blocking` and the push all happen under
    /// the mutex [`Self::discard_queued`] drains, so after `close()` returned no
    /// job can still be spawned: the sweep either sees the handle (and aborts
    /// it before the blocking pool starts it) or the job is never spawned at
    /// all. That ordering is the whole point — `abort` cannot stop a blocking
    /// job that already started, so the decision has to be taken BEFORE the
    /// spawn, never after it. A refused spawn drops `job`, whose slot counts
    /// the refusal as a dropped event exactly like a refused reservation.
    fn spawn_tracked(&self, job: impl FnOnce() + Send + 'static) -> Option<JoinHandle<()>> {
        let mut handles = match self.unfinished.lock() {
            Ok(handles) => handles,
            Err(_) => return None,
        };
        if self.state.load(Ordering::Acquire) & CLOSED != 0 {
            return None;
        }
        #[cfg(test)]
        fire_spawn_window_probe();
        let handle = tokio::task::spawn_blocking(job);
        handles.retain(|queued| !queued.is_finished());
        handles.push(handle.abort_handle());
        Some(handle)
    }

    /// Test-only: hold ONE global admission slot open exactly as an admitted
    /// refusal append does between its reservation and its job's completion.
    /// A teardown test uses this to keep [`Self::drain`] pending (and let its
    /// own budget expire) without driving a real refusal. Dropping the guard
    /// releases the slot like a finished job, returning the queue to idle.
    #[cfg(test)]
    pub(crate) fn hold_admission_slot_for_test(self: &Arc<Self>) -> impl Drop {
        self.try_reserve("test-held-refusal")
            .expect("refusal admission must be open")
    }

    /// Test-only: the admission width, so a test can size a fill without
    /// restating the constant.
    #[cfg(test)]
    pub(crate) fn global_capacity_for_test() -> usize {
        CANCEL_AUDIT_CAPACITY
    }

    /// Test-only: global slots currently reserved, read from the same counter
    /// the admission CAS loop maintains.
    #[cfg(test)]
    pub(crate) fn global_held_for_test(&self) -> usize {
        self.state.load(Ordering::Acquire) & COUNT_MASK
    }

    /// Test-only: `principal`'s live share count, read from the same map the
    /// admission decision updates.
    #[cfg(test)]
    pub(crate) fn principal_held_for_test(&self, principal: &str) -> usize {
        self.principals
            .read_sync(principal, |_, held| *held)
            .unwrap_or(0)
    }

    /// Test-only: ONE real reservation through the production admission path.
    #[cfg(test)]
    pub(crate) fn try_reserve_for_test(
        self: &Arc<Self>,
        principal: &str,
    ) -> Option<HeldReservations> {
        self.try_reserve(principal)
            .map(|slot| HeldReservations(vec![slot]))
    }

    /// Test-only: `count` real reservations charged to `principal`, kept alive
    /// by the returned slots. They go through [`Self::try_reserve`], so the
    /// global table and the principal counter are filled by the SAME counters
    /// production uses.
    #[cfg(test)]
    pub(crate) fn seed_reservations_for_test(
        self: &Arc<Self>,
        principal: &str,
        count: usize,
    ) -> HeldReservations {
        HeldReservations(
            (0..count)
                .map(|_| {
                    self.try_reserve(principal)
                        .expect("seeded reservation must be admitted")
                })
                .collect(),
        )
    }

    pub(crate) async fn drain(&self) -> bool {
        let drained = tokio::time::timeout(DRAIN_BUDGET, async {
            // Cancel-safe: no jobs are consumed by waiting.
            loop {
                let notified = self.idle.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if self.state.load(Ordering::Acquire) & COUNT_MASK == 0 {
                    break;
                }
                notified.await;
            }
        })
        .await;
        let complete = drained.is_ok();
        if !complete {
            let pending = self.state.load(Ordering::Acquire) & COUNT_MASK;
            tracing::warn!(pending, "cancel refusal audit drain timed out");
        }
        let dropped = self.dropped.load(Ordering::Relaxed);
        if dropped != 0 {
            tracing::debug!(dropped, "cancel refusal audit drops so far");
        }
        complete
    }
}

#[cfg(test)]
thread_local! {
    static SPAWN_WINDOW_PROBE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static SHARE_DECISION_PROBE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static CLOSE_DECISION_PROBE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Clears an unfired share-decision probe; a probe that fires removes itself.
#[cfg(test)]
pub(crate) struct ShareDecisionProbeGuard;

#[cfg(test)]
impl Drop for ShareDecisionProbeGuard {
    fn drop(&mut self) {
        SHARE_DECISION_PROBE.with(|slot| *slot.borrow_mut() = None);
    }
}

/// Fire and consume the current thread's share-decision probe, if any. Fired
/// between the read-only share pre-check and the bucket-locked decision, so a
/// parked probe holds NO lock and NO global slot.
#[cfg(test)]
fn fire_share_decision_probe() {
    let probe = SHARE_DECISION_PROBE.with(|slot| slot.borrow_mut().take());
    if let Some(probe) = probe {
        probe();
    }
}

#[cfg(test)]
pub(crate) struct CloseDecisionProbeGuard;

#[cfg(test)]
impl Drop for CloseDecisionProbeGuard {
    fn drop(&mut self) {
        CLOSE_DECISION_PROBE.with(|slot| *slot.borrow_mut() = None);
    }
}

#[cfg(test)]
fn fire_close_decision_probe() {
    let probe = CLOSE_DECISION_PROBE.with(|slot| slot.borrow_mut().take());
    if let Some(probe) = probe {
        probe();
    }
}

/// Clears an unfired spawn-window probe; a probe that fires removes itself.
#[cfg(test)]
pub(crate) struct SpawnWindowProbeGuard;

#[cfg(test)]
impl Drop for SpawnWindowProbeGuard {
    fn drop(&mut self) {
        SPAWN_WINDOW_PROBE.with(|slot| *slot.borrow_mut() = None);
    }
}

/// Fire and consume the current thread's spawn-window probe, if any.
#[cfg(test)]
fn fire_spawn_window_probe() {
    let probe = SPAWN_WINDOW_PROBE.with(|slot| slot.borrow_mut().take());
    if let Some(probe) = probe {
        probe();
    }
}

/// Test-only holder for reservations seeded through the production admission
/// path. Its inner `Vec` sits in a PRIVATE field, so the `pub(crate)` helpers
/// above can return live slots without naming the private [`CancelAuditSlot`].
#[cfg(test)]
pub(crate) struct HeldReservations(Vec<CancelAuditSlot>);

#[cfg(test)]
impl HeldReservations {
    /// Take over every slot of `other`, keeping all of them alive.
    pub(crate) fn append(&mut self, mut other: Self) {
        self.0.append(&mut other.0);
    }
}

struct CancelAuditSlot {
    queue: Arc<CancelRefusalAudit>,
    /// The NAME this slot is charged to — never a hash bucket, so two
    /// principals can never share one counter.
    principal: String,
    appended: bool,
}

impl Drop for CancelAuditSlot {
    fn drop(&mut self) {
        if !self.appended {
            self.queue.record_drop();
        }
        self.queue.release_share(&self.principal);
        // Releasing any slot necessarily makes the queue non-full; the shared
        // helper clears the saturation latch in the same transition.
        self.queue.release_global_slot();
    }
}

// Report a job discarded before execution (e.g. during runtime teardown).
struct PendingCancelAudit {
    target: u64,
    result: &'static str,
    dispatcher: tracing::Dispatch,
    completed: bool,
    // Field drop order: release the writer before signalling idle.
    audit: Arc<AuditChainWriter>,
    slot: CancelAuditSlot,
}

impl PendingCancelAudit {
    fn complete(mut self, appended: bool) {
        self.completed = true;
        self.slot.appended = appended;
    }
}

impl Drop for PendingCancelAudit {
    fn drop(&mut self) {
        if !self.completed {
            tracing::dispatcher::with_default(&self.dispatcher, || {
                tracing::error!(
                    target_id = self.target,
                    outcome = self.result,
                    "cancel refusal audit job dropped before append completed"
                );
            });
        }
    }
}

/// Stable audit status/label for one cancellation outcome. Shared by both
/// enqueue entry points so a refusal's status word never drifts from the
/// operator ops'.
fn cancel_outcome_label(outcome: &CancelOutcome) -> &'static str {
    match outcome {
        CancelOutcome::Cancelled => "cancelled",
        CancelOutcome::AlreadyFinished => "already_finished",
        CancelOutcome::AlreadyCommitting => "already_committing",
        CancelOutcome::NotCancellable => "not_cancellable",
    }
}

/// Best-effort, detached audit append for an OPERATOR-driven cancellation
/// (`cancel_tx` / `cancel_cursor_admin`): refusals AND successful cross-user
/// cancels are audited. The details payload carries only the target id and
/// the outcome. Admission, drain accounting and the blocking append are the
/// same machinery as [`enqueue_cancel_refusal_audit`].
pub(super) fn enqueue_operator_cancel_audit(
    queue: &Arc<CancelRefusalAudit>,
    audit: &Arc<AuditChainWriter>,
    session: &Session,
    operation: &'static str,
    target: u64,
    outcome: &CancelOutcome,
) -> Option<JoinHandle<()>> {
    let result = cancel_outcome_label(outcome);
    let slot = queue.try_reserve(&session.username)?;
    let details = match rmp_serde::to_vec_named(&CancelAuditDetails { target, outcome }) {
        Ok(details) => details,
        Err(error) => {
            tracing::error!(%error, "audit append skipped: cancel refusal encoding failed");
            return None;
        }
    };
    let now_ns = match UnixNanos::try_now() {
        Ok(now) => now.as_u64(),
        Err(error) => {
            tracing::error!(%error, "audit append skipped: clock unavailable");
            return None;
        }
    };
    let mut session_prefix = [0u8; 8];
    session_prefix.copy_from_slice(&session.session_id[..8]);
    let transport = transport_name(session.transport_kind);
    let username = session.username.clone();
    let pending = PendingCancelAudit {
        target,
        result,
        dispatcher: tracing::dispatcher::get_default(Clone::clone),
        completed: false,
        audit: Arc::clone(audit),
        slot,
    };
    #[cfg(test)]
    if let Some(pause) = queue.pause_before_spawn.as_ref() {
        pause();
    }
    queue.spawn_tracked(move || {
        let guard = pending;
        let appended = tracing::dispatcher::with_default(&guard.dispatcher, || {
            if let Err(error) = guard.audit.append(
                operation,
                transport,
                username,
                "",
                session_prefix,
                guard.result,
                details,
                now_ns,
            ) {
                tracing::error!(%error, target_id = guard.target, outcome = guard.result,
                    "audit append rejected cancel refusal");
                return false;
            }
            true
        });
        guard.complete(appended);
    })
}

/// Refusal-only audit for `cancel_request`: appends only for the outcomes
/// that refuse the cancel (`already_committing` / `not_cancellable`), never
/// for a successful or already-finished one. Same queue, capability and
/// drain accounting as [`enqueue_operator_cancel_audit`].
pub(super) fn enqueue_cancel_refusal_audit(
    queue: &Arc<CancelRefusalAudit>,
    audit: &Arc<AuditChainWriter>,
    session: &Session,
    target: RequestExecId,
    outcome: &CancelOutcome,
) -> Option<JoinHandle<()>> {
    match outcome {
        CancelOutcome::AlreadyCommitting | CancelOutcome::NotCancellable => {}
        CancelOutcome::Cancelled | CancelOutcome::AlreadyFinished => return None,
    }
    enqueue_operator_cancel_audit(queue, audit, session, "cancel_request", target.0, outcome)
}

/// Payload of one shape-statistics reset (`reset_shape_stats`): the
/// repository name ONLY.
///
/// The reset destroys node-local telemetry; there is no per-shape, counter
/// or user data to record, and none may enter the audit chain. The repo
/// name is the operator-visible target of the admin operation, never
/// sensitive (the same name already appears in `alter_repo` payloads).
#[derive(serde::Serialize)]
struct ShapeResetAuditDetails<'a> {
    repo: &'a str,
}

// Report a shape-reset job discarded before execution (e.g. during runtime
// teardown), so a refused spawn is visible and its slot is accounted.
struct PendingShapeResetAudit {
    dispatcher: tracing::Dispatch,
    completed: bool,
    // Field drop order: release the writer before signalling idle.
    audit: Arc<AuditChainWriter>,
    slot: CancelAuditSlot,
}

impl PendingShapeResetAudit {
    fn complete(mut self, appended: bool) {
        self.completed = true;
        self.slot.appended = appended;
    }
}

impl Drop for PendingShapeResetAudit {
    fn drop(&mut self) {
        if !self.completed {
            tracing::dispatcher::with_default(&self.dispatcher, || {
                tracing::error!("shape reset audit job dropped before append completed");
            });
        }
    }
}

/// Best-effort, detached audit append for a successful shape-statistics
/// reset. Admission, per-principal share, drain accounting and the blocking
/// append are the SAME machinery (and the same bounded queue) the operator
/// cancels use, so a reset can never block the reply and can never grow an
/// unbounded job list.
///
/// Operation label `reset_shape_stats` names the reset; the status word is
/// the fixed `ok` (the wipe has no partial outcome) and the details carry
/// the repository name only.
pub(super) fn enqueue_shape_reset_audit(
    queue: &Arc<CancelRefusalAudit>,
    audit: &Arc<AuditChainWriter>,
    session: &Session,
    repo: &str,
) -> Option<JoinHandle<()>> {
    let details = match rmp_serde::to_vec_named(&ShapeResetAuditDetails { repo }) {
        Ok(details) => details,
        Err(error) => {
            tracing::error!(%error, "audit append skipped: shape reset encoding failed");
            return None;
        }
    };
    let now_ns = match UnixNanos::try_now() {
        Ok(now) => now.as_u64(),
        Err(error) => {
            tracing::error!(%error, "audit append skipped: clock unavailable");
            return None;
        }
    };
    // Reserve after the encode/clock gates, exactly like the cancel enqueue:
    // a job that cannot be built never consumes a share slot.
    let slot = queue.try_reserve(&session.username)?;
    let mut session_prefix = [0u8; 8];
    session_prefix.copy_from_slice(&session.session_id[..8]);
    let transport = transport_name(session.transport_kind);
    let username = session.username.clone();
    let pending = PendingShapeResetAudit {
        dispatcher: tracing::dispatcher::get_default(Clone::clone),
        completed: false,
        audit: Arc::clone(audit),
        slot,
    };
    queue.spawn_tracked(move || {
        let guard = pending;
        let appended = tracing::dispatcher::with_default(&guard.dispatcher, || {
            if let Err(error) = guard.audit.append(
                "reset_shape_stats",
                transport,
                username,
                "",
                session_prefix,
                "ok",
                details,
                now_ns,
            ) {
                tracing::error!(%error, "audit append rejected shape reset");
                return false;
            }
            true
        });
        guard.complete(appended);
    })
}

pub(super) fn repo_audit_observer(
    audit: Arc<AuditChainWriter>,
    session: &Session,
) -> Arc<dyn RepoAdminObserver> {
    let mut session_prefix = [0u8; 8];
    session_prefix.copy_from_slice(&session.session_id[..8]);
    Arc::new(RepoAuditObserver {
        audit,
        username: session.username.clone(),
        transport: transport_name(session.transport_kind),
        session_prefix,
    })
}
