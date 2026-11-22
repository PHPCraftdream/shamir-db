use std::hash::BuildHasher;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

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
struct CancelRefusalDetails<'a> {
    target: RequestExecId,
    outcome: &'a CancelOutcome,
}

// The low bits count queued and running jobs; the high bits close admission
// and latch the first rejected submission of each saturation episode.
const CANCEL_AUDIT_CAPACITY: usize = 256;
/// Share of the queue one principal may hold (queued + running jobs). An
/// eighth of the global cap, so at least eight principals keep their whole
/// share while the global cap binds and no single principal can starve the
/// rest. Enforced per hash bucket of the principal name: a collision only
/// tightens a bucket's share, never loosens it, and the accounting stays a
/// fixed array instead of an unbounded per-name map on a non-waiting path.
pub(crate) const CANCEL_AUDIT_PRINCIPAL_SHARE: usize = 32;
/// Buckets for the per-principal share (`CANCEL_AUDIT_PRINCIPAL_SHARE * 32`
/// exceeds the global cap, so buckets never bind before the share does).
const CANCEL_AUDIT_PRINCIPAL_BUCKETS: usize = 32;
const CLOSED: usize = 1 << (usize::BITS - 1);
const SATURATED: usize = CLOSED >> 1;
const COUNT_MASK: usize = !(CLOSED | SATURATED);
const DRAIN_BUDGET: Duration = Duration::from_secs(3);

#[derive(Default)]
pub(crate) struct CancelRefusalAudit {
    state: AtomicUsize,
    dropped: AtomicUsize,
    idle: Notify,
    /// Queued + running jobs per principal bucket. Only ever touched by the
    /// non-waiting CAS in `try_reserve` and the slot's `Drop`, so the
    /// contention model is "one CAS pair per refusal" with no lock.
    principals: [AtomicUsize; CANCEL_AUDIT_PRINCIPAL_BUCKETS],
    /// Abort handles of accepted-but-unfinished audit jobs. Bounded by
    /// `CANCEL_AUDIT_CAPACITY` live slots (finished entries are pruned on
    /// the next refusal), taken once per accepted refusal for one bounded
    /// push plus a prune of finished handles, and once at teardown; it is
    /// never held across an `.await`. The client-visible cancel REPLY is
    /// produced before any of this and does not wait on the audit.
    unfinished: std::sync::Mutex<Vec<tokio::task::AbortHandle>>,
}

impl CancelRefusalAudit {
    fn try_reserve(self: &Arc<Self>, principal: &str) -> Option<CancelAuditSlot> {
        let bucket = principal_bucket(principal);
        let mut held = self.principals[bucket].load(Ordering::Acquire);
        loop {
            if held >= CANCEL_AUDIT_PRINCIPAL_SHARE {
                let dropped = self.record_drop();
                tracing::warn!(
                    share = CANCEL_AUDIT_PRINCIPAL_SHARE,
                    dropped,
                    "cancel refusal audit principal share full; dropping event"
                );
                return None;
            }
            match self.principals[bucket].compare_exchange_weak(
                held,
                held + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(current) => held = current,
            }
        }
        let mut state = self.state.load(Ordering::Acquire);
        loop {
            if state & CLOSED != 0 {
                self.principals[bucket].fetch_sub(1, Ordering::AcqRel);
                let dropped = self.record_drop();
                tracing::debug!(dropped, "cancel refusal audit dropped: admission closed");
                return None;
            }
            let full = state & COUNT_MASK == CANCEL_AUDIT_CAPACITY;
            if full && state & SATURATED != 0 {
                self.principals[bucket].fetch_sub(1, Ordering::AcqRel);
                self.record_drop();
                return None;
            }
            let next = if full { state | SATURATED } else { state + 1 };
            match self
                .state
                .compare_exchange_weak(state, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) if full => {
                    self.principals[bucket].fetch_sub(1, Ordering::AcqRel);
                    let dropped = self.record_drop();
                    tracing::warn!(
                        capacity = CANCEL_AUDIT_CAPACITY,
                        dropped,
                        "cancel refusal audit queue full; dropping events"
                    );
                    return None;
                }
                Ok(_) => {
                    return Some(CancelAuditSlot {
                        queue: Arc::clone(self),
                        bucket,
                        appended: false,
                    });
                }
                Err(current) => state = current,
            }
        }
    }

    fn record_drop(&self) -> usize {
        self.dropped.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
    }

    pub(crate) fn close(&self) {
        self.state.fetch_or(CLOSED, Ordering::AcqRel);
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

    /// Register one accepted job's abort handle, pruning finished ones.
    ///
    /// Invariant: `close()` is always called before `discard_queued()`, and
    /// the closed-bit test and the push happen under the same mutex the
    /// discard sweep drains, so a job is either swept or aborted — it is
    /// never registered after the sweep.
    fn track_or_abort(&self, handle: tokio::task::AbortHandle) {
        let mut handles = match self.unfinished.lock() {
            Ok(handles) => handles,
            Err(_) => {
                // Poisoned: the queue is unusable, so never leave the job
                // running past teardown.
                handle.abort();
                return;
            }
        };
        if self.state.load(Ordering::Acquire) & CLOSED != 0 {
            // `close()` already ran, so the discard sweep either saw nothing or
            // has already passed this job: abort it here instead of tracking it.
            handle.abort();
            return;
        }
        handles.retain(|queued| !queued.is_finished());
        handles.push(handle);
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

struct CancelAuditSlot {
    queue: Arc<CancelRefusalAudit>,
    bucket: usize,
    appended: bool,
}

impl Drop for CancelAuditSlot {
    fn drop(&mut self) {
        if !self.appended {
            self.queue.record_drop();
        }
        self.queue.principals[self.bucket].fetch_sub(1, Ordering::AcqRel);
        // Releasing any slot necessarily makes the queue non-full. Clear
        // the saturation latch in the same transition as the decrement.
        let mut state = self.queue.state.load(Ordering::Acquire);
        loop {
            let next = (state - 1) & !SATURATED;
            match self.queue.state.compare_exchange_weak(
                state,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    if next == CLOSED {
                        self.queue.idle.notify_waiters();
                    }
                    break;
                }
                Err(current) => state = current,
            }
        }
    }
}

// Report a job discarded before execution (e.g. during runtime teardown).
struct PendingCancelAudit {
    target: RequestExecId,
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
                    target_id = self.target.0,
                    outcome = self.result,
                    "cancel refusal audit job dropped before append completed"
                );
            });
        }
    }
}

pub(super) fn enqueue_cancel_refusal_audit(
    queue: &Arc<CancelRefusalAudit>,
    audit: &Arc<AuditChainWriter>,
    session: &Session,
    target: RequestExecId,
    outcome: &CancelOutcome,
) -> Option<JoinHandle<()>> {
    let result = match outcome {
        CancelOutcome::AlreadyCommitting => "already_committing",
        CancelOutcome::NotCancellable => "not_cancellable",
        CancelOutcome::Cancelled | CancelOutcome::AlreadyFinished => return None,
    };
    let slot = queue.try_reserve(&session.username)?;
    let details = match rmp_serde::to_vec_named(&CancelRefusalDetails { target, outcome }) {
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
    let job = tokio::task::spawn_blocking(move || {
        let guard = pending;
        let appended = tracing::dispatcher::with_default(&guard.dispatcher, || {
            if let Err(error) = guard.audit.append(
                "cancel_request",
                transport,
                username,
                "",
                session_prefix,
                guard.result,
                details,
                now_ns,
            ) {
                tracing::error!(%error, target_id = guard.target.0, outcome = guard.result,
                    "audit append rejected cancel refusal");
                return false;
            }
            true
        });
        guard.complete(appended);
    });
    queue.track_or_abort(job.abort_handle());
    Some(job)
}

/// Map a principal name to its share bucket. `THasher` is the workspace's
/// deterministic, unseeded hasher; a collision only makes the share tighter.
fn principal_bucket(principal: &str) -> usize {
    let hashed = shamir_collections::THasher::default().hash_one(principal);
    (hashed as usize) % CANCEL_AUDIT_PRINCIPAL_BUCKETS
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
