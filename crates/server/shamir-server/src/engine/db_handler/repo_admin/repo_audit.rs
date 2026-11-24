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

/// Admission class of a detached audit job.
///
/// `Security` covers cancellation refusals, operator cancels and shape-stat
/// resets — events an operator must be able to see even under read load.
/// `Observability` covers successful typed-read audits: high volume,
/// best-effort telemetry that must never crowd a security event out of the
/// queue.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AuditEventClass {
    Security,
    Observability,
}

impl AuditEventClass {
    const fn label(self) -> &'static str {
        match self {
            Self::Security => "security",
            Self::Observability => "observability",
        }
    }
}

/// One principal's live admission counts. `total` is bounded by
/// [`CANCEL_AUDIT_PRINCIPAL_SHARE`]; `observability` is the subset of that
/// total charged to [`AuditEventClass::Observability`] and is bounded by
/// [`CANCEL_AUDIT_OBSERVABILITY_PRINCIPAL_SHARE`].
#[derive(Clone, Copy)]
struct PrincipalHeld {
    total: usize,
    observability: usize,
}

impl PrincipalHeld {
    /// `true` when `class` may not take one more slot of this principal's share.
    fn is_full(&self, class: AuditEventClass) -> bool {
        self.total >= CANCEL_AUDIT_PRINCIPAL_SHARE
            || (class == AuditEventClass::Observability
                && self.observability >= CANCEL_AUDIT_OBSERVABILITY_PRINCIPAL_SHARE)
    }
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
/// Global slots `Observability` jobs may occupy — three quarters of
/// [`CANCEL_AUDIT_CAPACITY`] (192 of 256). Successful typed-read audits are
/// best-effort telemetry, so they are confined to this part of the cap and
/// the remaining quarter (64 slots) is always available to `Security`
/// events. Without the split, enough busy readers could fill all 256 slots
/// and drop a cancellation refusal. `Security` keeps using the full
/// [`CANCEL_AUDIT_CAPACITY`].
pub(crate) const CANCEL_AUDIT_OBSERVABILITY_CAPACITY: usize = CANCEL_AUDIT_CAPACITY / 4 * 3;
/// Per-principal slots `Observability` jobs may occupy — three quarters of
/// [`CANCEL_AUDIT_PRINCIPAL_SHARE`] (24 of 32). A principal's own read audits
/// may fill at most 24 slots, so the remaining 8 are always available to that
/// same principal's `Security` events: 32 pending read jobs must not make the
/// principal's next audit-worthy cancellation drop. `Security` keeps using
/// the full [`CANCEL_AUDIT_PRINCIPAL_SHARE`].
pub(crate) const CANCEL_AUDIT_OBSERVABILITY_PRINCIPAL_SHARE: usize =
    CANCEL_AUDIT_PRINCIPAL_SHARE / 4 * 3;
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
    principals: scc::HashMap<String, PrincipalHeld, THasher>,
    /// Live `Observability` jobs (queued + running) across every principal.
    /// Bounded by [`CANCEL_AUDIT_OBSERVABILITY_CAPACITY`]; `Security` jobs
    /// never touch it, so the rest of the global cap is theirs alone.
    observability: AtomicUsize,
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
    /// Test seam (drop accounting): when set, the shared detached-audit clock
    /// lookup fails without touching the system clock, so a test can drive the
    /// "clock unavailable" branch deterministically.
    #[cfg(test)]
    pub(crate) fail_clock: std::sync::atomic::AtomicBool,
}

#[path = "activity/serving/refusal_admission.rs"]
mod refusal_admission;

impl CancelRefusalAudit {
    /// Release one principal slot of `class` and retire the entry when it
    /// reaches zero.
    ///
    /// The decrement and the removal both run inside the name's bucket lock, so
    /// a reservation arriving in between is never lost: it either increments
    /// before the removal (the removal then sees a non-zero count and keeps the
    /// entry) or inserts a fresh entry afterwards.
    fn release_share(&self, principal: &str, class: AuditEventClass) {
        self.principals.update_sync(principal, |_, held| {
            held.total = held.total.saturating_sub(1);
            if class == AuditEventClass::Observability {
                held.observability = held.observability.saturating_sub(1);
            }
        });
        self.principals
            .remove_if_sync(principal, |held| held.total == 0);
    }

    /// Release one global observability slot; a no-op for `Security` jobs,
    /// which never charged one.
    fn release_observability(&self, class: AuditEventClass) {
        if class == AuditEventClass::Observability {
            self.observability.fetch_sub(1, Ordering::AcqRel);
        }
    }

    fn record_drop(&self, class: AuditEventClass) -> usize {
        let dropped = self.dropped.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        tracing::trace!(class = class.label(), dropped, "audit job dropped");
        dropped
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
        self.try_reserve("test-held-refusal", AuditEventClass::Security)
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
            .read_sync(principal, |_, held| held.total)
            .unwrap_or(0)
    }

    /// Test-only: ONE real reservation through the production admission path.
    #[cfg(test)]
    pub(crate) fn try_reserve_for_test(
        self: &Arc<Self>,
        principal: &str,
    ) -> Option<HeldReservations> {
        self.try_reserve(principal, AuditEventClass::Security)
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
                    self.try_reserve(principal, AuditEventClass::Security)
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
    /// The class the slot was admitted under; `Observability` slots also hold
    /// one global observability slot that [`Self::drop`] must release.
    class: AuditEventClass,
    appended: bool,
}

impl Drop for CancelAuditSlot {
    fn drop(&mut self) {
        if !self.appended {
            self.queue.record_drop(self.class);
        }
        self.queue.release_observability(self.class);
        self.queue.release_share(&self.principal, self.class);
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
                    class = self.slot.class.label(),
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
    let slot = queue.try_reserve(&session.username, AuditEventClass::Security)?;
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

// Report a detached audit job discarded before execution (e.g. during runtime
// teardown), so a refused spawn is visible and its slot is accounted.
struct PendingDetachedAudit {
    dispatcher: tracing::Dispatch,
    completed: bool,
    // Field drop order: release the writer before signalling idle.
    audit: Arc<AuditChainWriter>,
    slot: CancelAuditSlot,
    /// The operation-specific message logged when the job never ran.
    dropped_message: &'static str,
}

impl PendingDetachedAudit {
    fn complete(mut self, appended: bool) {
        self.completed = true;
        self.slot.appended = appended;
    }
}

impl Drop for PendingDetachedAudit {
    fn drop(&mut self) {
        if !self.completed {
            tracing::dispatcher::with_default(&self.dispatcher, || {
                tracing::error!(class = self.slot.class.label(), "{}", self.dropped_message);
            });
        }
    }
}

/// One ready-to-spawn detached audit append. The operation-specific pieces
/// (label, encoded details, the two error messages) are supplied by the
/// caller; everything else — the bounded admission/share reservation, the
/// clock, the session-prefix fields and the blocking append — is shared.
struct DetachedAuditJob<'a> {
    queue: &'a Arc<CancelRefusalAudit>,
    audit: &'a Arc<AuditChainWriter>,
    session: &'a Session,
    operation: &'static str,
    details: Vec<u8>,
    class: AuditEventClass,
    dropped_message: &'static str,
    rejected_message: &'static str,
}

/// Clock gate shared by the detached audit enqueues.
///
/// A clock failure drops the job BEFORE any reservation, so it must record the
/// drop itself (for `class`) — otherwise the queue's counter would miss a
/// dropped job. The test-only `fail_clock` seam makes this branch
/// deterministic without touching the system clock.
fn detached_audit_now(queue: &CancelRefusalAudit, class: AuditEventClass) -> Option<u64> {
    #[cfg(test)]
    let clock_failure = queue.fail_clock.load(Ordering::Acquire);
    #[cfg(not(test))]
    let clock_failure = false;
    if clock_failure {
        let dropped = queue.record_drop(class);
        tracing::error!(
            class = class.label(),
            dropped,
            "audit append skipped: clock unavailable"
        );
        return None;
    }
    match UnixNanos::try_now() {
        Ok(now) => Some(now.as_u64()),
        Err(error) => {
            let dropped = queue.record_drop(class);
            tracing::error!(
                %error,
                class = class.label(),
                dropped,
                "audit append skipped: clock unavailable"
            );
            None
        }
    }
}

/// Shared body of every best-effort, detached `ok` audit append: reserve one
/// bounded share slot in `class`, spawn a tracked blocking job and append
/// under the caller's dispatcher. A job that cannot be built (clock
/// unavailable, share or observability quota exhausted) never spawns and
/// accounts as a drop exactly like the cancel enqueues.
fn spawn_detached_audit(job: DetachedAuditJob<'_>) -> Option<JoinHandle<()>> {
    let DetachedAuditJob {
        queue,
        audit,
        session,
        operation,
        details,
        class,
        dropped_message,
        rejected_message,
    } = job;
    let now_ns = detached_audit_now(queue, class)?;
    // Reserve after the encode/clock gates, exactly like the cancel enqueue:
    // a job that cannot be built never consumes a share slot.
    let slot = queue.try_reserve(&session.username, class)?;
    let mut session_prefix = [0u8; 8];
    session_prefix.copy_from_slice(&session.session_id[..8]);
    let transport = transport_name(session.transport_kind);
    let username = session.username.clone();
    let pending = PendingDetachedAudit {
        dispatcher: tracing::dispatcher::get_default(Clone::clone),
        completed: false,
        audit: Arc::clone(audit),
        slot,
        dropped_message,
    };
    queue.spawn_tracked(move || {
        let guard = pending;
        let appended = tracing::dispatcher::with_default(&guard.dispatcher, || {
            if let Err(error) = guard.audit.append(
                operation,
                transport,
                username,
                "",
                session_prefix,
                "ok",
                details,
                now_ns,
            ) {
                tracing::error!(%error, "{}", rejected_message);
                return false;
            }
            true
        });
        guard.complete(appended);
    })
}

/// Best-effort, detached audit append for a successful shape-statistics
/// reset. Admission, per-principal share, drain accounting and the blocking
/// append are the SAME machinery (and the same bounded queue) the operator
/// cancels use, so a reset can never block the reply and can never grow an
/// unbounded job list.
///
/// Operation label `reset_shape_stats` names the reset; the status word is
/// the fixed `ok` (the wipe has no partial outcome) and the details carry
/// the repository name only. Admitted as [`AuditEventClass::Security`]: it is
/// an operator-visible admin event, so it keeps the full cap and share.
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
    spawn_detached_audit(DetachedAuditJob {
        queue,
        audit,
        session,
        operation: "reset_shape_stats",
        details,
        class: AuditEventClass::Security,
        dropped_message: "shape reset audit job dropped before append completed",
        rejected_message: "audit append rejected shape reset",
    })
}

/// Payload of one successful TYPED READ (`status` / `get_record`): the scope
/// NAMES only.
///
/// A read can return record values or carry a caller-supplied key/filter, and
/// NONE of that may enter the audit chain. The operation label, the db name
/// and (when the read is scoped to one table) the repository and table names
/// are the whole payload — no value, key, filter or literal.
#[derive(serde::Serialize)]
struct TypedReadAuditDetails<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    db: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repo: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    table: Option<&'a str>,
}

/// Best-effort, detached audit append for a successful typed read. Uses the
/// same bounded queue, admission/share accounting, clock and session-prefix
/// fields as [`enqueue_shape_reset_audit`]; only the operation label and the
/// names-only payload differ.
///
/// `operation` is the stable label (`status` / `get_record`), `db` is the read
/// scope's database name (the server-wide `status` passes an empty name, which
/// is omitted from the payload), and `repo_table` is `Some` for a table-scoped
/// read. The status word is the fixed
/// `ok`: this is called only on the success path, so a refusal is never
/// audited. The payload carries names only — never a record value, key, filter
/// or caller literal. Admitted as [`AuditEventClass::Observability`]: it is
/// high-volume, best-effort telemetry, confined to the reserved observability
/// part of the cap and share.
pub(super) fn enqueue_typed_read_audit(
    queue: &Arc<CancelRefusalAudit>,
    audit: &Arc<AuditChainWriter>,
    session: &Session,
    operation: &'static str,
    db: &str,
    repo_table: Option<(&str, &str)>,
) -> Option<JoinHandle<()>> {
    let db = (!db.is_empty()).then_some(db);
    let (repo, table) = match repo_table {
        Some((repo, table)) => (Some(repo), Some(table)),
        None => (None, None),
    };
    let details = match rmp_serde::to_vec_named(&TypedReadAuditDetails { db, repo, table }) {
        Ok(details) => details,
        Err(error) => {
            // No reservation was taken, so this path must record the dropped
            // observability job itself.
            let dropped = queue.record_drop(AuditEventClass::Observability);
            tracing::error!(
                %error,
                class = AuditEventClass::Observability.label(),
                dropped,
                "audit append skipped: typed read encoding failed"
            );
            return None;
        }
    };
    spawn_detached_audit(DetachedAuditJob {
        queue,
        audit,
        session,
        operation,
        details,
        class: AuditEventClass::Observability,
        dropped_message: "typed read audit job dropped before append completed",
        rejected_message: "audit append rejected typed read",
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
