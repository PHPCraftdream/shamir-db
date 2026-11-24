//! CancelRequest looks up ownership without taking a registry writer lock.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use shamir_connect::server::session::Session;
use shamir_query_types::admin::activity::{CancelOutcome, RequestExecId};
use shamir_query_types::wire::{CursorId, DbResponse};
use shamir_sync::durability::tx_cancel::TxParkedState;
use shamir_tx::{CancelState, TxContext};
use shamir_types::access::{Action, ResourcePath};

use super::requests::RequestEntry;

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::engine::db_handler) enum CancelPauseTarget {
    Transaction,
    Cursor,
}

impl ShamirDbHandler {
    #[cfg(test)]
    pub(in crate::engine::db_handler) fn with_cancel_authorized_pause(
        mut self,
        target: CancelPauseTarget,
        id: u64,
        reached: Arc<tokio::sync::Barrier>,
        resume: Arc<tokio::sync::Barrier>,
    ) -> Self {
        self.cancel_authorized_pause = Some((target, id, reached, resume));
        self
    }

    #[cfg(test)]
    pub(in crate::engine::db_handler) async fn pause_cancel_after_authorization(
        &self,
        target: CancelPauseTarget,
        id: u64,
    ) {
        if let Some((configured_target, configured_id, reached, resume)) =
            &self.cancel_authorized_pause
        {
            if *configured_target == target && *configured_id == id {
                reached.wait().await;
                resume.wait().await;
            }
        }
    }
}

use crate::engine::db_handler::handler::{session_actor, ShamirDbHandler};
use crate::engine::db_handler::repo_audit::{
    enqueue_cancel_refusal_audit, enqueue_operator_cancel_audit,
};
use crate::tx_registry::InteractiveTx;

pub(super) fn cancel_entry(entry: &RequestEntry) -> CancelOutcome {
    if !entry.cancellable {
        return match entry.cancel.state() {
            CancelState::Done | CancelState::Cancelled => CancelOutcome::AlreadyFinished,
            CancelState::Running | CancelState::Committing => CancelOutcome::NotCancellable,
        };
    }
    match entry.cancel.try_cancel() {
        Ok(()) => CancelOutcome::Cancelled,
        Err(CancelState::Committing) => CancelOutcome::AlreadyCommitting,
        Err(CancelState::Done | CancelState::Cancelled) => CancelOutcome::AlreadyFinished,
        Err(CancelState::Running) => unreachable!("running state cannot refuse cancellation"),
    }
}

/// Per-server cancellation outcome counters, one fixed slot per
/// `CancelOutcome`. Index order is shared with
/// `CANCEL_RESULT_LABELS` in the activity export
/// (`shamir_cancellations_total{result=...}`): 0 `cancelled`,
/// 1 `already_finished`, 2 `already_committing`, 3 `not_cancellable`.
pub(in crate::engine::db_handler) struct CancelCounters {
    results: [AtomicU64; 4],
}

impl CancelCounters {
    pub(in crate::engine::db_handler) fn new() -> Self {
        Self {
            results: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    pub(in crate::engine::db_handler) fn record(&self, outcome: CancelOutcome) {
        let index = match outcome {
            CancelOutcome::Cancelled => 0,
            CancelOutcome::AlreadyFinished => 1,
            CancelOutcome::AlreadyCommitting => 2,
            CancelOutcome::NotCancellable => 3,
        };
        self.results[index].fetch_add(1, Ordering::Relaxed);
    }

    pub(in crate::engine::db_handler) fn snapshot(&self) -> [u64; 4] {
        std::array::from_fn(|index| self.results[index].load(Ordering::Relaxed))
    }
}

impl ShamirDbHandler {
    /// Called on an authenticated control connection. Looking up the row
    /// reveals ownership internally, but no outcome is read or returned
    /// for another user's row before the ActivityView permission check.
    pub(in crate::engine::db_handler) async fn cancel_request(
        &self,
        session: &Session,
        target: RequestExecId,
    ) -> DbResponse {
        let Some(entry) = self.activity_requests.get(target.0) else {
            return self.cancel_reply(CancelOutcome::AlreadyFinished);
        };
        if entry.user_id != session.user_id
            && self
                .db
                .authorize_access(
                    &session_actor(session),
                    &ResourcePath::ActivityView,
                    Action::Manage,
                )
                .await
                .is_err()
        {
            return DbResponse::Error {
                code: "permission_denied".to_owned(),
                message:
                    "cancelling another user's request requires ActivityView manage permission"
                        .to_owned(),
            };
        }

        let outcome = cancel_entry(&entry);
        // Lookup has released its epoch guard. The state decision above does
        // not depend on audit I/O.
        //
        // The reply is built AFTER this synchronous enqueue, so its real cost
        // is the enqueue's cost: one per-principal counter update, one
        // `spawn_blocking` and one short push+prune of the queue's handle list.
        // None of that waits for the audit writer, its persistence gate or a
        // flush — a refusal that cannot be admitted is dropped and counted
        // instead of blocking the reply.
        if let Some(audit) = &self.audit {
            // The job is detached: the append runs on a blocking thread and
            // this reply never awaits it.
            drop(enqueue_cancel_refusal_audit(
                &self.cancel_refusal_audit,
                audit,
                session,
                target,
                &outcome,
            ));
        }
        self.cancel_reply(outcome)
    }

    /// The single construction point for a cancel reply, so every produced
    /// outcome is counted exactly once — no cancel path may build
    /// `DbResponse::CancelOutcome` directly.
    fn cancel_reply(&self, outcome: CancelOutcome) -> DbResponse {
        self.cancel_counters.record(outcome);
        DbResponse::CancelOutcome { outcome }
    }

    async fn may_manage_other_users_work(&self, session: &Session) -> bool {
        self.db
            .authorize_access(
                &session_actor(session),
                &ResourcePath::ActivityView,
                Action::Manage,
            )
            .await
            .is_ok()
    }

    /// `target` is only ever a fixed literal (`"transaction"` or `"cursor"`),
    /// so the message stays free of caller-supplied text.
    fn cancel_permission_denied(target: &'static str) -> DbResponse {
        DbResponse::Error {
            code: "permission_denied".to_owned(),
            message: format!(
                "cancelling another user's {target} requires ActivityView manage permission"
            ),
        }
    }

    /// Best-effort, detached audit for an operator-driven cancel: appends for
    /// every refusal outcome AND for every successful cross-user cancel —
    /// the same-user non-refusal case is the client's own normal path and is
    /// deliberately not audited.
    fn audit_operator_cancel(
        &self,
        session: &Session,
        operation: &'static str,
        target: u64,
        target_user_id: [u8; 16],
        outcome: &CancelOutcome,
    ) {
        let Some(audit) = &self.audit else { return };
        // Audited: both refusals, and a SUCCESSFUL cancel of another user's
        // work. `already_finished` is neither (nothing was refused or ended).
        let audited = match outcome {
            CancelOutcome::AlreadyCommitting | CancelOutcome::NotCancellable => true,
            CancelOutcome::Cancelled => target_user_id != session.user_id,
            CancelOutcome::AlreadyFinished => false,
        };
        if !audited {
            return;
        }
        // Detach by design: the reply must not wait for the writer.
        drop(enqueue_operator_cancel_audit(
            &self.cancel_refusal_audit,
            audit,
            session,
            operation,
            target,
            outcome,
        ));
    }

    /// Cancel an OPEN interactive transaction named by the activity-view
    /// handle. Own user (any of that user's sessions) needs authentication
    /// only; another user's transaction needs `ActivityView` + `Manage`. An
    /// unknown handle answers `already_finished` before the permission check.
    ///
    /// The decision is made ONCE, against the authoritative `open` map: the
    /// owner comes from the parked transaction, and the removal re-checks the
    /// handle's identity (`Arc::ptr_eq`) under the `open` entry's own shard
    /// lock, so a handle re-minted for ANOTHER transaction between the
    /// authorization and the removal answers `already_finished` and leaves
    /// the replacement alone. That shard lock is the only lock this cancel
    /// takes — no commit gate, no commit mutex — and no `.await` runs while
    /// it is held.
    ///
    /// A transaction with a request in flight is refused with
    /// `not_cancellable` (its context lock is only ever attempted, never
    /// awaited); a momentary hold by the commit path answers
    /// `already_committing` instead, since that handle is already reserved.
    /// An idle one has its Level-3 record locks released in the cancel call,
    /// since dropping the `Arc` frees none of them.
    pub(in crate::engine::db_handler) async fn cancel_tx(
        &self,
        session: &Session,
        tx_handle: u64,
    ) -> DbResponse {
        let (target_user_id, target, committing_row) = match self.tx_registry.peek(tx_handle) {
            TxParkedState::Open(tx) => (*tx.owner_user_id(), Some(tx), None),
            // A `Committing` reservation parks no `Arc`; its projection row is
            // the only ownership record. Retain its identity across auth.
            TxParkedState::Committing => match self.tx_registry.activity_row(tx_handle) {
                Some(row) => (*row.user_id(), None, Some(row)),
                None => return self.cancel_reply(CancelOutcome::AlreadyFinished),
            },
            TxParkedState::Gone => return self.cancel_reply(CancelOutcome::AlreadyFinished),
        };
        if target_user_id != session.user_id && !self.may_manage_other_users_work(session).await {
            return Self::cancel_permission_denied("transaction");
        }
        if target_user_id != session.user_id {
            #[cfg(test)]
            self.pause_cancel_after_authorization(CancelPauseTarget::Transaction, tx_handle)
                .await;
        }
        let outcome = match (target, committing_row) {
            (Some(tx), _) => self.cancel_parked_tx(tx_handle, tx).await,
            (None, Some(row)) => match (
                self.tx_registry.peek(tx_handle),
                self.tx_registry.activity_row(tx_handle),
            ) {
                (TxParkedState::Committing, Some(current)) if Arc::ptr_eq(&row, &current) => {
                    CancelOutcome::AlreadyCommitting
                }
                _ => CancelOutcome::AlreadyFinished,
            },
            (None, None) => CancelOutcome::AlreadyFinished,
        };
        self.audit_operator_cancel(session, "cancel_tx", tx_handle, target_user_id, &outcome);
        self.cancel_reply(outcome)
    }

    /// Remove `tx` (identity-checked) and release what it holds. The context
    /// lock is taken with a NON-BLOCKING attempt: a `TxExecute` holds it for
    /// the whole plan and the cancel never waits for that request.
    async fn cancel_parked_tx(&self, tx_handle: u64, tx: Arc<InteractiveTx>) -> CancelOutcome {
        let Ok(mut ctx) = tx.ctx().try_lock() else {
            // The lock is held by the owner's request (a `TxExecute` holds it
            // for the whole plan), or momentarily by the commit path reading
            // the context. Only the first case is a request the operator must
            // leave alone; a reserved handle is already committing. This is a
            // refusal-only re-read: no removal decision depends on it. A
            // transaction that is gone, or replaced by another one, is not a
            // request to leave alone: it answers `already_finished`.
            return match self.tx_registry.peek(tx_handle) {
                TxParkedState::Committing => CancelOutcome::AlreadyCommitting,
                TxParkedState::Open(current) if Arc::ptr_eq(&current, &tx) => {
                    CancelOutcome::NotCancellable
                }
                TxParkedState::Open(_) | TxParkedState::Gone => CancelOutcome::AlreadyFinished,
            };
        };
        match self.tx_registry.remove_identity(tx_handle, &tx) {
            TxParkedState::Open(tx) => match ctx.take() {
                Some(parked) => {
                    drop(ctx);
                    self.release_tx_locks(tx_handle, parked, &tx).await;
                    CancelOutcome::Cancelled
                }
                None => CancelOutcome::AlreadyFinished,
            },
            TxParkedState::Committing => CancelOutcome::AlreadyCommitting,
            TxParkedState::Gone => CancelOutcome::AlreadyFinished,
        }
    }

    /// The operator-cancel path's own synchronous release of the Level-3
    /// record locks the cancelled context still holds, kept so the locks are
    /// gone before the cancel reply returns. Every other end of an interactive
    /// transaction is covered by the registry's `Drop` release, and a repeated
    /// release of the same transaction is a harmless no-op because
    /// `MvccStore::release_locks` removes only holders whose `tx_version`
    /// matches.
    async fn release_tx_locks(&self, tx_handle: u64, parked: TxContext, tx: &InteractiveTx) {
        let release = tx.abort_release();
        // The cancel dispatch can be dropped with its caller. Keep the lock
        // release alive independently; dropping a JoinHandle detaches the
        // task, while normal dispatch still waits for completion before reply.
        let release = Arc::clone(release);
        if let Err(error) = tokio::spawn(async move { release(parked).await }).await {
            if error.is_panic() {
                tracing::error!(tx_handle, error = %error, "transaction lock-release task panicked");
            } else {
                tracing::warn!(tx_handle, error = %error, "transaction lock-release task was cancelled; accepted cancellation and removed handle remain in effect, but runtime shutdown may stop lock release");
            }
        }
    }

    /// Cancel another session's result cursor by its id. Ownership comes from
    /// the cursor registry's activity mirror; `not_cancellable` NEVER applies
    /// here — the only outcomes are `cancelled` and `already_finished`. An
    /// unknown cursor, or one the registry holds without activity metadata
    /// (never produced by the wire `CreateCursor` path) and therefore cannot
    /// be attributed to a user, answers `already_finished` before any
    /// permission check.
    ///
    /// The snapshot scan is an operator-driven O(N) read (no lock any
    /// lifecycle writer takes); N is bounded by the per-session cursor caps.
    pub(in crate::engine::db_handler) async fn cancel_cursor_admin(
        &self,
        session: &Session,
        cursor_id: CursorId,
    ) -> DbResponse {
        let Some(row) = self
            .cursor_registry
            .activity_snapshot()
            .into_iter()
            .find(|row| row.cursor_id == cursor_id.0)
        else {
            return self.cancel_reply(CancelOutcome::AlreadyFinished);
        };
        let target_user_id = row.meta.user_id;
        if target_user_id != session.user_id && !self.may_manage_other_users_work(session).await {
            return Self::cancel_permission_denied("cursor");
        }
        if target_user_id != session.user_id {
            #[cfg(test)]
            self.pause_cancel_after_authorization(CancelPauseTarget::Cursor, cursor_id.0)
                .await;
        }
        // `remove` frees the session slot and detaches the pinned snapshot;
        // dropping the returned `Arc` is what releases that snapshot.
        let outcome = match self
            .cursor_registry
            .remove_activity_identity(cursor_id.0, &row.meta)
        {
            Some(cursor) => {
                drop(cursor);
                CancelOutcome::Cancelled
            }
            None => CancelOutcome::AlreadyFinished,
        };
        self.audit_operator_cancel(
            session,
            "cancel_cursor_admin",
            cursor_id.0,
            target_user_id,
            &outcome,
        );
        self.cancel_reply(outcome)
    }
}
