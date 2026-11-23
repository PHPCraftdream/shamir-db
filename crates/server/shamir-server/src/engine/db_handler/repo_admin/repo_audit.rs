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
struct CancelAuditDetails<'a> {
    target: u64,
    outcome: &'a CancelOutcome,
}

// The low bits count queued and running jobs; the high bits close admission
// and latch the first rejected submission of each saturation episode.
const CANCEL_AUDIT_CAPACITY: usize = 256;
const CLOSED: usize = 1 << (usize::BITS - 1);
const SATURATED: usize = CLOSED >> 1;
const COUNT_MASK: usize = !(CLOSED | SATURATED);
const DRAIN_BUDGET: Duration = Duration::from_secs(3);

#[derive(Default)]
pub(crate) struct CancelRefusalAudit {
    state: AtomicUsize,
    dropped: AtomicUsize,
    idle: Notify,
}

impl CancelRefusalAudit {
    fn try_reserve(self: &Arc<Self>) -> Option<CancelAuditSlot> {
        let mut state = self.state.load(Ordering::Acquire);
        loop {
            if state & CLOSED != 0 {
                let dropped = self.record_drop();
                tracing::debug!(dropped, "cancel refusal audit dropped: admission closed");
                return None;
            }
            let full = state & COUNT_MASK == CANCEL_AUDIT_CAPACITY;
            if full && state & SATURATED != 0 {
                self.record_drop();
                return None;
            }
            let next = if full { state | SATURATED } else { state + 1 };
            match self
                .state
                .compare_exchange_weak(state, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) if full => {
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

    pub(crate) async fn drain(&self) {
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
        if drained.is_err() {
            let pending = self.state.load(Ordering::Acquire) & COUNT_MASK;
            tracing::warn!(pending, "cancel refusal audit drain timed out");
        }
        let dropped = self.dropped.load(Ordering::Relaxed);
        if dropped != 0 {
            tracing::debug!(dropped, "cancel refusal audit drops so far");
        }
    }
}

struct CancelAuditSlot {
    queue: Arc<CancelRefusalAudit>,
    appended: bool,
}

impl Drop for CancelAuditSlot {
    fn drop(&mut self) {
        if !self.appended {
            self.queue.record_drop();
        }
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
    let slot = queue.try_reserve()?;
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
    Some(tokio::task::spawn_blocking(move || {
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
    }))
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
