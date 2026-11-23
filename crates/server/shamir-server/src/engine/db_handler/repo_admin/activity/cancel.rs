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
        let refusal = matches!(
            outcome,
            CancelOutcome::AlreadyCommitting | CancelOutcome::NotCancellable
        );
        if !refusal && target_user_id == session.user_id {
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
        let (target_user_id, target) = match self.tx_registry.peek(tx_handle) {
            TxParkedState::Open(tx) => (*tx.owner_user_id(), Some(tx)),
            // A `Committing` reservation parks no `Arc`; its projection row is
            // the only ownership record, and this branch only REFUSES.
            TxParkedState::Committing => match self.tx_registry.activity_row(tx_handle) {
                Some(row) => (*row.user_id(), None),
                None => return self.cancel_reply(CancelOutcome::AlreadyFinished),
            },
            TxParkedState::Gone => return self.cancel_reply(CancelOutcome::AlreadyFinished),
        };
        if target_user_id != session.user_id && !self.may_manage_other_users_work(session).await {
            return Self::cancel_permission_denied("transaction");
        }
        let outcome = match target {
            Some(tx) => self.cancel_parked_tx(tx_handle, tx).await,
            None => CancelOutcome::AlreadyCommitting,
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
            // refusal-only re-read: no removal decision depends on it.
            return match self.tx_registry.peek(tx_handle) {
                TxParkedState::Committing => CancelOutcome::AlreadyCommitting,
                _ => CancelOutcome::NotCancellable,
            };
        };
        match self.tx_registry.remove_identity(tx_handle, &tx) {
            TxParkedState::Open(tx) => match ctx.take() {
                Some(parked) => {
                    drop(ctx);
                    self.release_tx_locks(&parked, &tx).await;
                    CancelOutcome::Cancelled
                }
                None => CancelOutcome::AlreadyFinished,
            },
            TxParkedState::Committing => CancelOutcome::AlreadyCommitting,
            TxParkedState::Gone => CancelOutcome::AlreadyFinished,
        }
    }

    /// Release the Level-3 pessimistic record locks a parked transaction still
    /// holds. `TxContext` has no `Drop` that frees them, so every path that ends
    /// a transaction outside the commit pipeline must call the engine's explicit
    /// release before dropping the context (the drop itself runs the ordinary
    /// RAII rollback and snapshot release).
    async fn release_tx_locks(&self, parked: &TxContext, tx: &InteractiveTx) {
        let Some(db) = self.db.get_db(tx.db()) else {
            return;
        };
        let Some(repo) = db.get_repo(tx.repo()) else {
            return;
        };
        shamir_db::engine::tx::release_interactive_tx_locks(parked, repo.core()).await;
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

        // `remove` frees the session slot and detaches the pinned snapshot;
        // dropping the returned `Arc` is what releases that snapshot.
        let outcome = match self.cursor_registry.remove(cursor_id.0) {
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
